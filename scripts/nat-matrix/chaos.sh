#!/usr/bin/env bash
# chaos.sh <loss10|delay200|blackout3|permfail>
#
# netem 混沌单例(Task 10):拓扑恒 port_restricted × port_restricted(混沌最
# 敏感的常见真实形态),在真实二进制对传路径上注入损伤。生命周期同
# run_case.sh:teardown → setup → 服务 → 探针 → 注册 → 注入 → 收发 → 断言;
# EXIT trap 失败转储完整日志 + teardown。时间断言只有各自硬超时(宁慢勿抖,
# Task 11 校准只放宽不收紧):
#   loss10    : 两侧 gw-ext netem loss 10%         — 送达 + 清单一致    (120s)
#   delay200  : 两侧 gw-ext netem delay 200ms 50ms — 送达 + 清单一致    (180s)
#   blackout3 : 传输在途断 gwA-ext 3s 后恢复(子进程 IDLE=1s/KEEPALIVE=500ms)
#               → 掉线检测 → 退避重连 → 续传完成,终态清单一致          (120s)
#   permfail  : 同 blackout3 但不恢复;子进程 MAX_RECONNECTS=2/BACKOFF=300ms,
#               收侧窗口 8s → send 非 0 带逐文件报告(3 轮),
#               recv "接收窗口超时"                                      (120s)
#
# 断链落点修正:plan 定稿 "send 启动 2s 后断链",但实测单轮传输 0–1s 即完成,
# 固定 2s 必落在传输结束后(用例空转)。改为轮询收侧 .mtdss/*.part——清单协商
# 完成后传输在途时出现,断链确定落在 QUIC 已建立的传输中;载荷保持 4×3MiB
# 不变,并以 send 日志 "send attempt 0 failed" 断言第一轮确被击中(落点不空转)。
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
RECV_PID=""
SEND_PID=""

CASE_DIR="$(mktemp -d "${TMPDIR:-/tmp}/mtdss-chaos.XXXXXX")"
H_A="$CASE_DIR/home_a"
H_B="$CASE_DIR/home_b"
OUT_B="$CASE_DIR/out_b"
PAYLOAD_DIR="$CASE_DIR/payload"
TLS_DIR="$CASE_DIR/tls"
NM_CA_PEM="$TLS_DIR/ca.crt"
LOG_A="$CASE_DIR/send_a.log"
LOG_B="$CASE_DIR/recv_b.log"

cleanup() {
  local rc=$1 p
  if [ "$rc" -ne 0 ]; then
    nm_log "FAIL(chaos/$CASE) — 转储完整日志:"
    for f in "$CASE_DIR/signaling.log" "$CASE_DIR/stun.log" "$LOG_B" "$LOG_A" "$CASE_DIR/rate.log"; do
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

# ③ 双侧注册(同账号两台设备)
nm_register "$NM_NS_A" "$H_A" dev@test mtdss-a || nm_die "注册 a 失败"
nm_register "$NM_NS_B" "$H_B" dev@test mtdss-b || nm_die "注册 b 失败"
DEV_B="$(nm_json_str "$H_B/session.json" device_id)"
[ -n "$DEV_B" ] || nm_die "拿不到 b 的 device_id"

# ④ 子进程环境按用例注入(Task 5 测试面;loss/delay 用默认)
SEND_ENV=(env MTDSS_HOME="$H_A" RUST_LOG="${NM_RUST_LOG:-info}")
RECV_ENV=(env MTDSS_HOME="$H_B" RUST_LOG="${NM_RUST_LOG:-info}")
case "$CASE" in
  blackout3 | permfail)
    # 掉线快速暴露:1s 空闲超时 + 500ms keep-alive(两侧同注入)
    SEND_ENV+=(MTDSS_TEST_IDLE_TIMEOUT_MS=1000 MTDSS_TEST_KEEPALIVE_MS=500)
    RECV_ENV+=(MTDSS_TEST_IDLE_TIMEOUT_MS=1000 MTDSS_TEST_KEEPALIVE_MS=500)
    ;;
esac
case "$CASE" in
  permfail)
    # 永久故障:小重试预算(总尝试 = 2+1 = 3 轮)+ 收侧 8s 接收窗口
    SEND_ENV+=(MTDSS_TEST_MAX_RECONNECTS=2 MTDSS_TEST_BACKOFF_MS=300)
    RECV_ENV+=(MTDSS_TEST_MAX_RECONNECTS=2 MTDSS_TEST_BACKOFF_MS=300 MTDSS_TEST_RECV_WINDOW_MS=8000)
    ;;
esac

# ⑤ 损伤注入
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
    # 机制性配套(plan 注入只定 delay 参数本身):netem 对 GSO super-skb 是
    # 整体入延迟队列、整体出队——64KB 一次突发,ACK 时钟被切成巨块,与
    # 高 RTT 叠加后实测吞吐塌到 ~50KB/s(run 39013491209/39014880905,
    # netem dropped=0、无重传,排除丢包/拥塞塌陷)。先关四条 WAN 腿网卡的
    # offload 让 netem 看到 MTU 级 skb,再注入。
    command -v ethtool >/dev/null 2>&1 || nm_die "缺命令: ethtool(关 WAN 腿 offload 需要)"
    for spec in "$NM_NS_GWA:gwA-ext" "$NM_NS_GWB:gwB-ext" "$NM_NS_PUB:pub-a" "$NM_NS_PUB:pub-b"; do
      nexec "${spec%%:*}" ethtool -K "${spec##*:}" gro off gso off tso off >/dev/null 2>&1 || true
    done
    # 抖动带相关性:缺省每包独立抽抖动会重排突发、超 QUIC 3 包重排容限,
    # 连发虚假丢包;75% 相关保序而幅度仍为 200ms±50ms。limit 抬高防突发
    # 尾部丢包。
    apply_netem "$NM_NS_GWA" gwA-ext limit 10000 delay 200ms 50ms 75%
    apply_netem "$NM_NS_GWB" gwB-ext limit 10000 delay 200ms 50ms 75%
    nm_log "netem 注入: offloads off + delay 200ms 50ms 75% (gwA-ext + gwB-ext)"
    ;;
esac

# ⑥ b 侧接收(后台)+ presence 就绪
mkdir -p "$OUT_B"
ip netns exec "$NM_NS_B" "${RECV_ENV[@]}" \
  "$MTDSS_BIN" recv --out "$OUT_B" --ca "$NM_CA_PEM" >"$LOG_B" 2>&1 &
RECV_PID=$!
NM_PIDS+=("$RECV_PID")
nm_wait_online "$DEV_B" "$H_A" || nm_die "b($DEV_B) 未在信令目录上线"

# ⑦ a 侧发送(后台 + timeout 硬超时兜底;断链注入发生在运行中)
T0="$(date +%s)"
timeout "$SEND_TIMEOUT_S" ip netns exec "$NM_NS_A" "${SEND_ENV[@]}" \
  "$MTDSS_BIN" send "$PAYLOAD_DIR"/f1.bin "$PAYLOAD_DIR"/f2.bin \
  "$PAYLOAD_DIR"/f3.bin "$PAYLOAD_DIR"/f4.bin \
  --to "$DEV_B" --ca "$NM_CA_PEM" >"$LOG_A" 2>&1 &
SEND_PID=$!
NM_PIDS+=("$SEND_PID")

# ⑦' 速率采样:每 2s 记录 gwA-ext qdisc 累计发送字节,给出传输速率时间线
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

# ⑧ 断链注入(blackout3/permfail):等收侧 .part 出现(传输在途、QUIC 已建立)
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

# ⑨ 等 send 退出(timeout 顶格时退出码 124 = 硬超时触发)
set +e
wait "$SEND_PID"
SEND_RC=$?
set -e
T1="$(date +%s)"
ELAPSED=$((T1 - T0))

# ⑩ 断言(按用例分派)
case "$CASE" in
  loss10 | delay200 | blackout3)
    # ok 语义:send exit 0(+blackout 断言第一轮确被击中)+ recv 自退 0 +
    # 双侧 sha256 清单一致;时间断言 = 硬超时(上方 timeout),不收紧
    if [ "$SEND_RC" -ne 0 ]; then
      nm_log "send 退出码 $SEND_RC(预期 0,耗时 ${ELAPSED}s)"
      exit 1
    fi
    if [ "$CASE" = blackout3 ]; then
      grep -q "send attempt 0 failed" "$LOG_A" || {
        nm_log "send 日志无 \"send attempt 0 failed\":断链未击中第一轮传输(落点空转)"
        exit 1
      }
    fi
    WAITED=0
    while kill -0 "$RECV_PID" 2>/dev/null && [ "$WAITED" -lt 60 ]; do
      sleep 1
      WAITED=$((WAITED + 1))
    done
    if kill -0 "$RECV_PID" 2>/dev/null; then
      nm_log "recv 未在 60s 内自退(预期一轮完成即退)"
      exit 1
    fi
    set +e
    wait "$RECV_PID"
    RECV_RC=$?
    set -e
    if [ "$RECV_RC" -ne 0 ]; then
      nm_log "recv 退出码 $RECV_RC(预期 0)"
      exit 1
    fi
    (
      cd "$OUT_B" &&
        find . -maxdepth 1 -type f -name '*.bin' -printf '%f\n' |
        LC_ALL=C sort | xargs -r sha256sum
    ) | LC_ALL=C sort >"$CASE_DIR/manifest.dst"
    if ! diff -u "$CASE_DIR/manifest.src" "$CASE_DIR/manifest.dst"; then
      nm_log "双侧 sha256 清单不一致"
      exit 1
    fi
    if [ "$CASE" = delay200 ]; then
      # 速率曲线随 PASS 一起打出来(Task 11 校准与回归归因)
      while IFS= read -r line; do nm_log "rate: $line"; done <"$CASE_DIR/rate.log"
    fi
    nm_log "PASS chaos/$CASE (send=${ELAPSED}s)"
    exit 0
    ;;
  permfail)
    # fail 语义:send 非 0 带逐文件报告(3 轮预算耗尽,f4 必在未完成栏),
    # recv 非 0 且日志含 "接收窗口超时"
    if [ "$SEND_RC" -eq 0 ]; then
      nm_log "send 退出码 0(预期非 0,耗时 ${ELAPSED}s)"
      exit 1
    fi
    grep -q "发送失败" "$LOG_A" || {
      nm_log "send 日志无终局失败报告"
      exit 1
    }
    grep -q "共尝试 3 轮" "$LOG_A" || {
      nm_log "send 报告轮次不符(预期 MAX_RECONNECTS=2 → 3 轮)"
      exit 1
    }
    grep -q "未完成" "$LOG_A" || {
      nm_log "send 报告缺 未完成 分栏"
      exit 1
    }
    grep -q "f4.bin" "$LOG_A" || {
      nm_log "send 报告未列出 f4.bin(逐文件清单缺失)"
      exit 1
    }
    WAITED=0
    while kill -0 "$RECV_PID" 2>/dev/null && [ "$WAITED" -lt 30 ]; do
      sleep 1
      WAITED=$((WAITED + 1))
    done
    if kill -0 "$RECV_PID" 2>/dev/null; then
      nm_log "recv 未按预期超窗退出(8s 窗口早应到期)"
      exit 1
    fi
    set +e
    wait "$RECV_PID"
    RECV_RC=$?
    set -e
    if [ "$RECV_RC" -eq 0 ]; then
      nm_log "recv 退出码 0(预期非 0:8s 窗口内对端未回)"
      exit 1
    fi
    grep -q "接收窗口超时" "$LOG_B" || {
      nm_log "recv 日志无 接收窗口超时"
      exit 1
    }
    nm_log "PASS chaos/$CASE (send_rc=$SEND_RC, recv_rc=$RECV_RC, ${ELAPSED}s)"
    exit 0
    ;;
esac
