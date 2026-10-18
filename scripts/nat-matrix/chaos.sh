#!/usr/bin/env bash
# chaos.sh <loss10|delay200|blackout3|permfail>
#
# netem 混沌单例(Task 10):拓扑恒 port_restricted × port_restricted(混沌最
# 敏感的常见真实形态),在真实二进制对传路径上注入损伤。生命周期同
# run_case.sh:teardown → setup → 服务 → 探针 → daemon → 登录 → 注入 →
# 收发 → 断言;EXIT trap 失败转储完整日志 + teardown。时间断言只有各自硬
# 超时(宁慢勿抖,Task 11 校准只放宽不收紧):
#   loss10    : 两侧 gw-ext netem loss 10%         — 送达 + 清单一致    (120s)
#   delay200  : 两侧 gw-ext netem delay 200ms 固定 — 送达 + 清单一致    (180s)
#               (±50ms 抖动会使突发重排→cubic 锁死初始窗口,见 ⑤ 注释)
#   blackout3 : 传输在途断 gwA-ext 3s 后恢复(IDLE=1s/KEEPALIVE=500ms 注入
#               两侧 daemon)→ 掉线检测 → 退避重连 → 续传完成,终态清单一致
#                                                                    (120s)
#   permfail  : 同 blackout3 但不恢复;发送 daemon MAX_RECONNECTS=2/
#               BACKOFF=300ms → send 非 0 带逐文件报告(3 轮);收侧 daemon
#               常驻不退出(无窗口语义),一轮失败留痕于日志                (120s)
#
# 断链落点修正:plan 定稿 "send 启动 2s 后断链",但实测单轮传输 0–1s 即完成,
# 固定 2s 必落在传输结束后(用例空转)。改为轮询收侧 .mtdss/*.part——清单协商
# 完成后传输在途时出现,断链确定落在 QUIC 已建立的传输中;载荷保持 4×3MiB
# 不变,并以 daemon 日志 "send attempt 0 failed" 断言第一轮确被击中(落点不空转)。
set -euo pipefail
HERE="$(cd "$(dirname "${BASH_SOURCE[0]}")" && pwd)"
# shellcheck source=lib.sh
source "$HERE/lib.sh"

CASE="${1:?usage: chaos.sh <loss10|delay200|blackout3|permfail>}"
case "$CASE" in
  loss10 | delay200 | blackout3 | permfail) ;;
  *) nm_die "未知 chaos 用例: $CASE" ;;
esac

# 各例硬超时(plan 定稿初值)
case "$CASE" in
  delay200) SEND_TIMEOUT_S=180 ;;
  *) SEND_TIMEOUT_S=120 ;;
esac

nm_require_env
command -v tc >/dev/null 2>&1 || nm_die "缺命令: tc(netem 注入需要)"

NM_PIDS=()
SEND_PID=""

CASE_DIR="$(mktemp -d "${TMPDIR:-/tmp}/mtdss-chaos.XXXXXX")"
H_A="$CASE_DIR/home_a"
H_B="$CASE_DIR/home_b"
OUT_B="$CASE_DIR/out_b"
PAYLOAD_DIR="$CASE_DIR/payload"
TLS_DIR="$CASE_DIR/tls"
NM_CA_PEM="$TLS_DIR/ca.crt"
LOG_A="$CASE_DIR/daemon_a.log"      # a 侧 daemon 日志
LOG_B="$CASE_DIR/daemon_b.log"      # b 侧 daemon 日志
CLI_OUT="$CASE_DIR/send_cli.log"    # a 侧 CLI 输出(进度/失败报告)

cleanup() {
  local rc=$1 p
  # 先收子进程(daemon/信令/STUN/send/采样),不留孤儿(同 run_case.sh)
  for p in ${NM_PIDS[@]+"${NM_PIDS[@]}"}; do kill "$p" 2>/dev/null || true; done
  for p in ${NM_PIDS[@]+"${NM_PIDS[@]}"}; do wait "$p" 2>/dev/null || true; done
  if [ "$rc" -ne 0 ]; then
    nm_log "FAIL(chaos/$CASE) — 转储完整日志:"
    for f in "$CASE_DIR/signaling.log" "$CASE_DIR/stun.log" "$LOG_B" "$LOG_B.stdio" "$LOG_A" "$LOG_A.stdio" "$CLI_OUT" "$CASE_DIR/rate.log"; do
      if [ -f "$f" ]; then
        echo "===== $f ====="
        cat "$f"
      fi
    done
    nm_dump_netns_state
    # qdisc 统计(netem 丢包/延迟计数),归因用
    if nm_ns_exists "$NM_NS_GWA"; then
      echo "===== qdisc 统计 gwA/gwA-ext ====="
      nexec "$NM_NS_GWA" tc -s qdisc show dev gwA-ext 2>/dev/null || true
    fi
    if nm_ns_exists "$NM_NS_GWB"; then
      echo "===== qdisc 统计 gwB/gwB-ext ====="
      nexec "$NM_NS_GWB" tc -s qdisc show dev gwB-ext 2>/dev/null || true
    fi
    if [ -d "$OUT_B" ]; then
      echo "===== $OUT_B(已落盘) ====="
      find "$OUT_B" -maxdepth 2 -type f -printf '%s\t%p\n' 2>/dev/null | LC_ALL=C sort -k2 || true
      echo "===== $OUT_B/.mtdss 记录(到位块位图) ====="
      cat "$OUT_B"/.mtdss/*.json 2>/dev/null || true
    fi
  fi
  bash "$HERE/teardown.sh" >/dev/null 2>&1 || true
  rm -rf "$CASE_DIR"
  exit "$rc"
}
trap 'cleanup $?' EXIT

# ① 清场重建(混沌拓扑恒 port_restricted × port_restricted)
bash "$HERE/teardown.sh" >/dev/null 2>&1 || true
NM_TYPE_A=port_restricted NM_TYPE_B=port_restricted bash "$HERE/setup.sh"

# ② 载荷 + 测试网 TLS + 服务
bash "$HERE/payload.sh" "$PAYLOAD_DIR" >"$CASE_DIR/manifest.src"
nm_gen_tls "$TLS_DIR" || nm_die "生成测试网 TLS 失败"
nm_start_servers "$CASE_DIR"
nm_wait_http_ready || nm_die "信令服务未就绪"

# ②' STUN 可达探针(注入之前钉死通路,探针走干净网络,同 run_case)
nm_stun_probe "$NM_NS_PUB" "$NM_STUN_PORT" || nm_die "STUN 探针 pub:$NM_STUN_PORT 不可达"
nm_stun_probe "$NM_NS_A" "$NM_STUN_PORT" || nm_die "STUN 探针 a:$NM_STUN_PORT 不可达"
nm_stun_probe "$NM_NS_A" "$NM_STUN_ALT" || nm_die "STUN 探针 a:$NM_STUN_ALT 不可达"
nm_stun_probe "$NM_NS_B" "$NM_STUN_PORT" || nm_die "STUN 探针 b:$NM_STUN_PORT 不可达"
nm_stun_probe "$NM_NS_B" "$NM_STUN_ALT" || nm_die "STUN 探针 b:$NM_STUN_ALT 不可达"
nm_log "stun probes ok (pub/a/b x $NM_STUN_PORT,$NM_STUN_ALT)"

# ③ 双侧 daemon + 登录(同账号两台设备;daemon 常驻即收端;MTDSS_TEST_*
# 注入面移到 daemon 启动环境——编排选项在 daemon 内一次性读取,起前定好)
DAEMON_ENV_A=()
DAEMON_ENV_B=()
case "$CASE" in
  blackout3 | permfail)
    # 掉线快速暴露:1s 空闲超时 + 500ms keep-alive(两侧同注入)
    DAEMON_ENV_A+=(MTDSS_TEST_IDLE_TIMEOUT_MS=1000 MTDSS_TEST_KEEPALIVE_MS=500)
    DAEMON_ENV_B+=(MTDSS_TEST_IDLE_TIMEOUT_MS=1000 MTDSS_TEST_KEEPALIVE_MS=500)
    ;;
esac
case "$CASE" in
  permfail)
    # 永久故障:发送侧小重试预算(总尝试 = 2+1 = 3 轮)。收侧 daemon 常驻
    # 等来件,无接收窗口语义(旧 RECV_WINDOW_MS 注入随之失效,不再传)
    DAEMON_ENV_A+=(MTDSS_TEST_MAX_RECONNECTS=2 MTDSS_TEST_BACKOFF_MS=300)
    ;;
esac
nm_start_daemon "$NM_NS_A" "$H_A" "$LOG_A" "$H_A/inbox" ${DAEMON_ENV_A[@]+"${DAEMON_ENV_A[@]}"}
nm_start_daemon "$NM_NS_B" "$H_B" "$LOG_B" "$OUT_B" ${DAEMON_ENV_B[@]+"${DAEMON_ENV_B[@]}"}
nm_register "$NM_NS_A" "$H_A" dev@test mtdss-a || nm_die "登录 a 失败"
nm_register "$NM_NS_B" "$H_B" dev@test mtdss-b || nm_die "登录 b 失败"
DEV_B="$(nm_json_str "$H_B/session.json" device_id)"
[ -n "$DEV_B" ] || nm_die "拿不到 b 的 device_id"

# ④ 损伤注入(netem 作用于 netns 网卡;探针已走干净网络)
apply_netem() { # <ns> <if> <netem 参数...>(replace 幂等,重复跑不因 qdisc 已存在而失败)
  nexec "$1" tc qdisc replace dev "$2" root netem "${@:3}"
}
case "$CASE" in
  loss10)
    apply_netem "$NM_NS_GWA" gwA-ext loss 10%
    apply_netem "$NM_NS_GWB" gwB-ext loss 10%
    nm_log "netem 注入: loss 10% (gwA-ext + gwB-ext)"
    ;;
  delay200)
    # 注入定稿为固定 200ms(不带 ±50ms 抖动),机制性依据如下:
    # netem 的抖动是"每包独立抽值 + tfifo 按到期时间排序"——只要抖动幅度
    # 超过包间间隔(quinn 以 cwnd 突发发包,间隔≈0),突发内必然重排;QUIC
    # 的丢包判定是 3 包重排容限,重排即被记为丢包,cubic 每次事件 ×0.7,
    # 而 400ms RTT 下 CA 恢复只有 ~1 MSS/RTT——丢包率与恢复速度互相咬死,
    # 吞吐被锁死在 ~初始窗口(实测 CI 三跑 ~50KB/s,180s 只送完 83%,
    # run 39013491209/39014880905/39016181767;本地最小复现同样锁定,
    # 而"固定 200ms 无抖动"9.2s 送完 12MiB)。correlation 75/95/99% 都救不回
    # (突发间隔≈0,任何独立分量都会反序)。固定延迟下 tfifo 严格保序,
    # 用例回到"慢但不挂"的本意:200ms 单向 ×2 腿,RTT≈400ms,传输数秒完成。
    # limit 抬高防突发尾丢(GSO super-skb 计数)。
    # 注:曾试过关 WAN 腿网卡 offload(GSO super-skb 整体进延迟队列,commit
    # 985c103),固定延迟下 tfifo 保序、无重排可谈,实测无差别,已移除。
    apply_netem "$NM_NS_GWA" gwA-ext limit 10000 delay 200ms
    apply_netem "$NM_NS_GWB" gwB-ext limit 10000 delay 200ms
    nm_log "netem 注入: delay 200ms 固定 (gwA-ext + gwB-ext;±50ms 抖动因重排锁定吞吐不可用,见上)"
    ;;
  blackout3 | permfail)
    # 断链落点前提是"传输在途足够久":12MiB 在 veth 上 ~0.1s 就能送完,
    # .part 触发即断链是对抽(run 39014880905 断在传输中 PASS;39020493998
    # 断在完成后——收端已收满,重连一律 "target offline",send 耗尽
    # 重试 FAIL)。先垫 50ms 延迟把传输窗口拉到秒级,断链确定落在传输中,
    # IDLE=1s 的掉线检测路径才真正被走到。
    apply_netem "$NM_NS_GWA" gwA-ext limit 10000 delay 50ms
    apply_netem "$NM_NS_GWB" gwB-ext limit 10000 delay 50ms
    nm_log "netem 注入: delay 50ms (垫宽传输窗口,稳定断链落点)"
    ;;
esac

# ⑤ presence 就绪(常驻 daemon 已在等来件;CLI devices 读 daemon 快照)
nm_wait_online "$DEV_B" "$H_A" || nm_die "b($DEV_B) 未在信令目录上线"

# ⑥ a 侧发送(CLI → 本侧 daemon RPC 长调用;后台 + timeout 硬超时兜底,
# 断链注入发生在运行中)
T0="$(date +%s)"
timeout "$SEND_TIMEOUT_S" ip netns exec "$NM_NS_A" \
  env MTDSS_HOME="$H_A" RUST_LOG="${NM_RUST_LOG:-info}" \
  "$MTDSS_BIN" send "$PAYLOAD_DIR"/f1.bin "$PAYLOAD_DIR"/f2.bin \
  "$PAYLOAD_DIR"/f3.bin "$PAYLOAD_DIR"/f4.bin \
  --to "$DEV_B" >"$CLI_OUT" 2>&1 &
SEND_PID=$!
NM_PIDS+=("$SEND_PID")

# ⑥' 速率采样:每 2s 记录 gwA-ext qdisc 累计发送字节,给出传输速率时间线
# (失败归因与 Task 11 校准用)。采样进程随 send 退出自然结束。
(
  T=0
  while kill -0 "$SEND_PID" 2>/dev/null; do
    sleep 2
    T=$((T + 2))
    B="$(nexec "$NM_NS_GWA" tc -s qdisc show dev gwA-ext 2>/dev/null | sed -n 's/.*Sent \([0-9]*\) bytes.*/\1/p' | head -n1)"
    printf 'T+%ss sent=%s\n' "$T" "${B:-NA}" >>"$CASE_DIR/rate.log"
  done
) &
NM_PIDS+=("$!")

# ⑦ 断链注入(blackout3/permfail):等收侧 .part 出现(传输在途、QUIC 已建立)
case "$CASE" in
  blackout3 | permfail)
    WAITED_MS=0
    while ! find "$OUT_B/.mtdss" -maxdepth 1 -name '*.part' -print -quit 2>/dev/null | grep -q .; do
      [ "$WAITED_MS" -lt 15000 ] ||
        nm_die "15s 内未观察到收侧 .part(传输未在途,断链落点不成立)"
      kill -0 "$SEND_PID" 2>/dev/null ||
        nm_die "send 先于断链注入退出(异常,查上方日志)"
      sleep 0.05
      WAITED_MS=$((WAITED_MS + 50))
    done
    nm_log "传输在途(${WAITED_MS}ms):断链 gwA-ext"
    ip netns exec "$NM_NS_GWA" ip link set gwA-ext down
    if [ "$CASE" = blackout3 ]; then
      sleep 3
      ip netns exec "$NM_NS_GWA" ip link set gwA-ext up
      # link down 会清掉经该接口的路由(含 default),up 不自动恢复——重建
      ip netns exec "$NM_NS_GWA" ip route replace default via "$NM_PUB_IP_A"
      nm_log "链路恢复(default route 重建),等待退避重连与续传"
    else
      nm_log "链路保持 down(permfail)"
    fi
    ;;
esac

# ⑧ 等 send 退出(timeout 顶格时退出码 124 = 硬超时触发)
set +e
wait "$SEND_PID"
SEND_RC=$?
set -e
T1="$(date +%s)"
ELAPSED=$((T1 - T0))

# ⑨ 断言(按用例分派)
case "$CASE" in
  loss10 | delay200 | blackout3)
    # ok 语义:send exit 0(+blackout 断言第一轮确被击中,daemon 日志)+
    # OUT_B 落盘齐(60s 上限)+ b 侧「接收完成」终局行;时间断言 = 硬超时
    # (上方 timeout),不收紧。收端是常驻 daemon,不再等「recv 自退」。
    if [ "$SEND_RC" -ne 0 ]; then
      nm_log "send 退出码 $SEND_RC(预期 0,耗时 ${ELAPSED}s)"
      exit 1
    fi
    if [ "$CASE" = blackout3 ]; then
      grep -q "send attempt 0 failed" "$LOG_A" || {
        nm_log "a 侧 daemon 日志无 \"send attempt 0 failed\":断链未击中第一轮传输(落点空转)"
        exit 1
      }
    fi
    WAITED=0
    while :; do
      nm_manifest_snapshot "$OUT_B" "$CASE_DIR/manifest.dst"
      if cmp -s "$CASE_DIR/manifest.src" "$CASE_DIR/manifest.dst"; then
        break
      fi
      if [ "$WAITED" -ge 60 ]; then
        diff -u "$CASE_DIR/manifest.src" "$CASE_DIR/manifest.dst" || true
        nm_log "60s 内 b 侧落盘不齐"
        exit 1
      fi
      sleep 1
      WAITED=$((WAITED + 1))
    done
    # 终局行有界轮询:接收方 EOF 收尾(≤2s)可晚于清单轮询通过那一刻,
    # 一次性 grep 属 TOCTOU 偶发误红(nm_wait_log 注释详述)。
    nm_wait_log "$LOG_B" "接收完成" 30 ||
      { nm_log "30s 内 b 侧日志未出现「接收完成」终局行"; exit 1; }
    if [ "$CASE" = delay200 ]; then
      # 速率曲线随 PASS 一起打出来(Task 11 校准与回归归因)
      while IFS= read -r line; do nm_log "rate: $line"; done <"$CASE_DIR/rate.log"
    fi
    nm_log "PASS chaos/$CASE (send=${ELAPSED}s)"
    exit 0
    ;;
  permfail)
    # fail 语义:send 非 0 带逐文件报告(3 轮预算耗尽,f4 必在未完成栏;
    # 报告全文随 RPC 错误回 CLI,落在 CLI_OUT),收侧 daemon 一轮失败留痕
    # 于 LOG_B(常驻接收不退出,无窗口语义)
    if [ "$SEND_RC" -eq 0 ]; then
      nm_log "send 退出码 0(预期非 0,耗时 ${ELAPSED}s)"
      exit 1
    fi
    grep -q "发送失败" "$CLI_OUT" || {
      nm_log "CLI 输出无终局失败报告"
      exit 1
    }
    grep -q "共尝试 3 轮" "$CLI_OUT" || {
      nm_log "报告轮次不符(预期 MAX_RECONNECTS=2 → 3 轮)"
      exit 1
    }
    grep -q "未完成" "$CLI_OUT" || {
      nm_log "报告缺 未完成 分栏"
      exit 1
    }
    grep -q "f4.bin" "$CLI_OUT" || {
      nm_log "报告未列出 f4.bin(逐文件清单缺失)"
      exit 1
    }
    grep -q "常驻接收轮失败" "$LOG_B" || {
      nm_log "b 侧 daemon 日志无接收轮失败留痕"
      exit 1
    }
    nm_log "PASS chaos/permfail (send_rc=$SEND_RC, ${ELAPSED}s)"
    exit 0
    ;;
esac
