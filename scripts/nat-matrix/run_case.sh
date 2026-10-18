#!/usr/bin/env bash
# run_case.sh <typeA> <typeB> <expect:ok|fail>
#
# 单用例:teardown → setup(两侧 NAT 类型) → 起信令/STUN → 双侧起 daemon
# (常驻接收即收端)→ 双侧 CLI 登录(dev 验证码回环)→ presence 轮询 →
# a 侧 CLI send → 按预期断言。
# exit 0 = 行为与预期一致;非 0 = 不一致或环境故障(失败先倒两侧完整日志)。
#
# expect=ok  : send exit 0 + 双侧 daemon 日志各 grep 到
#              srflx=[203.0.113.2 / srflx=[203.0.113.6 + OUT_B 落盘清单一致
#              + LOG_B 出现「接收完成」终局行。
# expect=fail: send exit 非 0 + 日志含 "appears symmetric"(对称侧 punch_err
#              诊断,Task 9 评审收紧) + 耗时 ≤ 70s(Task 11 校准)。
# fail 例两侧 daemon 注入小重试预算(plan Task 9:MAX_RECONNECTS=1、
# BACKOFF_MS=500;打洞预算保持默认 10s——NAT 时序是测点)。
#
# NM_AUTO_SPAWN_A=1:auto-spawn 门禁模式。a 侧只临时起 daemon 供登录,登录
# 后即收掉——send 阶段 a 侧无 daemon,由 `mtdss send` 的 connect_or_spawn
# 自动拉起(CLI 自动拉起行为唯一的真实二进制覆盖;预期仍为 ok 全链)。
set -euo pipefail
HERE="$(cd "$(dirname "${BASH_SOURCE[0]}")" && pwd)"
# shellcheck source=lib.sh
source "$HERE/lib.sh"

TYPE_A="$(nm_norm_type "${1:?usage: run_case.sh <typeA> <typeB> <expect:ok|fail>}")" ||
  nm_die "bad typeA: $1"
TYPE_B="$(nm_norm_type "${2:?usage: run_case.sh <typeA> <typeB> <expect:ok|fail>}")" ||
  nm_die "bad typeB: $2"
EXPECT="${3:?usage: run_case.sh <typeA> <typeB> <expect:ok|fail>}"
case "$EXPECT" in
  ok | fail) ;;
  *) nm_die "expect 必须是 ok|fail,得: $EXPECT" ;;
esac
NM_AUTO_SPAWN="${NM_AUTO_SPAWN_A:-0}"
if [ "$NM_AUTO_SPAWN" = 1 ] && [ "$EXPECT" != ok ]; then
  nm_die "auto-spawn 门禁只配 ok 例(该模式不注入重试预算,daemon 由 CLI 环境拉起)"
fi

nm_require_env
NM_PIDS=()
DAEMON_A_PID=""

CASE_DIR="$(mktemp -d "${TMPDIR:-/tmp}/mtdss-nm.XXXXXX")"
H_A="$CASE_DIR/home_a"
H_B="$CASE_DIR/home_b"
OUT_B="$CASE_DIR/out_b"
PAYLOAD_DIR="$CASE_DIR/payload"
TLS_DIR="$CASE_DIR/tls"
NM_CA_PEM="$TLS_DIR/ca.crt"
LOG_A="$CASE_DIR/daemon_a.log" # a 侧 daemon 日志(auto-spawn 模式仅登录阶段日志)
LOG_B="$CASE_DIR/daemon_b.log" # b 侧 daemon 日志
CLI_OUT_A="$CASE_DIR/send_cli.log" # a 侧 CLI 输出(进度/失败报告/auto-spawn 模式的 daemon 日志)
CAP_FILE="$CASE_DIR/tcpdump_gwb.txt"
CAP_PID=""

cleanup() {
  local rc=$1 p
  # auto-spawn 模式:CLI 拉起的 a 侧 daemon 不在 NM_PIDS,按 ns 收尾防孤儿
  if [ "$NM_AUTO_SPAWN" = 1 ]; then
    nm_kill_ns_procs "$NM_NS_A"
  fi
  for p in ${NM_PIDS[@]+"${NM_PIDS[@]}"}; do kill "$p" 2>/dev/null || true; done
  for p in ${NM_PIDS[@]+"${NM_PIDS[@]}"}; do wait "$p" 2>/dev/null || true; done
  if [ "$rc" -ne 0 ]; then
    nm_log "FAIL($TYPE_A x $TYPE_B expect=$EXPECT) — 转储完整日志:"
    for f in "$CASE_DIR/signaling.log" "$CASE_DIR/stun.log" "$LOG_B" "$LOG_B.stdio" "$LOG_A" "$LOG_A.stdio" "$CLI_OUT_A" "$CAP_FILE"; do
      if [ -f "$f" ]; then
        echo "===== $f ====="
        cat "$f"
      fi
    done
    nm_dump_netns_state
  fi
  bash "$HERE/teardown.sh" >/dev/null 2>&1 || true
  rm -rf "$CASE_DIR"
  exit "$rc"
}
trap 'cleanup $?' EXIT

# ① 清场重建(带两侧 NAT 类型)
bash "$HERE/teardown.sh" >/dev/null 2>&1 || true
NM_TYPE_A="$TYPE_A" NM_TYPE_B="$TYPE_B" bash "$HERE/setup.sh"

# ② 载荷 + 测试网 TLS + 服务
bash "$HERE/payload.sh" "$PAYLOAD_DIR" >"$CASE_DIR/manifest.src"
nm_gen_tls "$TLS_DIR" || nm_die "生成测试网 TLS 失败"
nm_start_servers "$CASE_DIR"
nm_wait_http_ready || nm_die "信令服务未就绪"

# ②' STUN 可达探针(pub 本机 + 双侧,双端口):absorb 首包冷路径,并在
# gather 之前把网络通路钉死——探针失败即环境故障,不等 2s 超时才暴露。
# 探针期间在 gwB-ext 抓线上包(tcpdump 可用时):SNAT 是否保端口、回应
# 目的端口是什么,一屏可查——2026-10-17 CI 曾定位到 b 侧回应全被 DNAT
# (conntrack miss)+ b 侧 ICMP port-unreachable,需线上证据定因。
if command -v tcpdump >/dev/null 2>&1; then
  ip netns exec "$NM_NS_GWB" timeout 40 tcpdump -ni gwB-ext -l -c 200 udp >"$CAP_FILE" 2>&1 &
  CAP_PID=$!
  NM_PIDS+=("$CAP_PID")
fi
nm_stun_probe "$NM_NS_PUB" "$NM_STUN_PORT" || nm_die "STUN 探针 pub:$NM_STUN_PORT 不可达"
nm_stun_probe "$NM_NS_A" "$NM_STUN_PORT" || nm_die "STUN 探针 a:$NM_STUN_PORT 不可达"
nm_stun_probe "$NM_NS_A" "$NM_STUN_ALT" || nm_die "STUN 探针 a:$NM_STUN_ALT 不可达"
nm_stun_probe "$NM_NS_B" "$NM_STUN_PORT" || nm_die "STUN 探针 b:$NM_STUN_PORT 不可达"
nm_stun_probe "$NM_NS_B" "$NM_STUN_ALT" || nm_die "STUN 探针 b:$NM_STUN_ALT 不可达"
if [ -n "$CAP_PID" ]; then
  kill "$CAP_PID" 2>/dev/null || true
  wait "$CAP_PID" 2>/dev/null || true
  CAP_PID=""
fi
nm_log "stun probes ok (pub/a/b x $NM_STUN_PORT,$NM_STUN_ALT)"

# ③ 双侧 daemon + 登录(同账号两台设备;身份种子与会话各自落在 case 临时
# home;MTDSS_TEST_* 注入面移到 daemon 启动环境——编排选项在 daemon 内读取)
DAEMON_ENV_A=()
DAEMON_ENV_B=()
if [ "$EXPECT" = fail ]; then
  DAEMON_ENV_A+=(MTDSS_TEST_MAX_RECONNECTS=1 MTDSS_TEST_BACKOFF_MS=500)
  DAEMON_ENV_B+=(MTDSS_TEST_MAX_RECONNECTS=1 MTDSS_TEST_BACKOFF_MS=500)
fi
nm_start_daemon "$NM_NS_A" "$H_A" "$LOG_A" "$H_A/inbox" ${DAEMON_ENV_A[@]+"${DAEMON_ENV_A[@]}"}
DAEMON_A_PID="$NM_LAST_PID"
nm_start_daemon "$NM_NS_B" "$H_B" "$LOG_B" "$OUT_B" ${DAEMON_ENV_B[@]+"${DAEMON_ENV_B[@]}"}

nm_register "$NM_NS_A" "$H_A" dev@test mtdss-a || nm_die "登录 a 失败"
nm_register "$NM_NS_B" "$H_B" dev@test mtdss-b || nm_die "登录 b 失败"
DEV_A="$(nm_json_str "$H_A/session.json" device_id)"
[ -n "$DEV_A" ] || nm_die "拿不到 a 的 device_id"
DEV_B="$(nm_json_str "$H_B/session.json" device_id)"
[ -n "$DEV_B" ] || nm_die "拿不到 b 的 device_id"

# ③' 键序日志(plan §4.2:互拨预期与键序无关;含对称双跑回归两种键序,
# 这里把两侧 pubkey hex 与字典序较小方打进日志供归因)。纯日志,不门控。
nm_log_key_order "$DEV_A" "$H_A" "$DEV_B" "$H_B" || true

# ③'' auto-spawn 门禁:登录需要 daemon 在场(登录经 IPC),登录后即收掉
# a 侧 daemon——send 阶段 a 侧无 daemon,由 CLI send 的 connect_or_spawn
# 自动拉起(同 home 同 MTDSS_HOME,锁/管道口径一致)。
if [ "$NM_AUTO_SPAWN" = 1 ]; then
  nm_log "auto-spawn gate: 收掉 a 侧 daemon($DAEMON_A_PID),send 由 CLI 自动拉起"
  kill "$DAEMON_A_PID" 2>/dev/null || true
  wait "$DAEMON_A_PID" 2>/dev/null || true
fi

# ④ presence 就绪(plan:轮询 a 侧 mtdss devices;上限 20s,超时按环境故障
# 处理)。auto-spawn 模式 a 侧 daemon 不在场,无 presence 可等——CLI send
# 的自动拉起即本例测点,直接进 ⑤。
if [ "$NM_AUTO_SPAWN" = 1 ]; then
  nm_log "auto-spawn gate: 跳过 a 侧 presence 等待"
else
  nm_wait_online "$DEV_B" "$H_A" || nm_die "b($DEV_B) 未在信令目录上线"
fi

# ⑤ a 侧发送(CLI → 本侧 daemon 的 RPC 长调用;CLI 硬超时兜底防挂死,
# 正常成功远小于该值;进度与失败报告落 CLI_OUT_A)
T0="$(date +%s)"
set +e
timeout "${NM_SEND_TIMEOUT_S:-180}" ip netns exec "$NM_NS_A" \
  env MTDSS_HOME="$H_A" RUST_LOG="${NM_RUST_LOG:-info}" \
  "$MTDSS_BIN" send "$PAYLOAD_DIR"/f1.bin "$PAYLOAD_DIR"/f2.bin \
  "$PAYLOAD_DIR"/f3.bin "$PAYLOAD_DIR"/f4.bin \
  --to "$DEV_B" >"$CLI_OUT_A" 2>&1
SEND_RC=$?
set -e
T1="$(date +%s)"
ELAPSED=$((T1 - T0))

# ok 例:收端是常驻 daemon,不再等「recv 进程自退」;改为轮询 OUT_B 落盘齐
# (60s 上限,复用旧等待形状)
if [ "$EXPECT" = ok ]; then
  if [ "$SEND_RC" -ne 0 ]; then
    nm_log "send 退出码 $SEND_RC(预期 0,耗时 ${ELAPSED}s)"
    exit 1
  fi

  # ⑥ 断言(srflx 候选 / 落盘清单 / 接收完成终局行 全过)
  # a 侧候选行:常规在 LOG_A;auto-spawn 模式 daemon 由 CLI 拉起、无
  # --log-file,日志随继承的 stderr 落在 CLI_OUT_A。
  if ! grep -q "srflx=\[$NM_GWA_EXT_IP" "$LOG_A" 2>/dev/null; then
    grep -q "srflx=\[$NM_GWA_EXT_IP" "$CLI_OUT_A" ||
      { nm_log "a 侧缺 srflx=[$NM_GWA_EXT_IP(daemon 候选行)"; exit 1; }
  fi
  grep -q "srflx=\[$NM_GWB_EXT_IP" "$LOG_B" ||
    { nm_log "b 侧日志缺 srflx=[$NM_GWB_EXT_IP(daemon 候选行)"; exit 1; }
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
  nm_log "PASS $TYPE_A x $TYPE_B expect=ok (${ELAPSED}s)"
  exit 0
fi

# ⑥' fail 例断言:exit 非 0 + 对称侧诊断 + 耗时 ≤ 70s。
# 70s 来源(Task 11 校准,只放宽):三次全绿 CI nat-matrix job 的 12 个
# fail 例 send 耗时样本(39010737459-a1 / 39022881071-a2 / 39024437668-a3)
# 为 {11,11,20,20,21,21,21,21,21,21,21,21}s,中位 21s,3× 中位 = 63s,
# 按 10s 粒度向上取整;旧值 30s 只有 1.4× 中位,慢机抖动即误杀。
# 「appears symmetric」在 daemon 日志(send attempt/常驻接收轮失败 的 warn
# 行,punch_err 全链在内)与 CLI 输出(RPC 错误原文)都有,一并 grep。
if [ "$SEND_RC" -eq 0 ]; then
  nm_log "send 退出码 0(预期非 0,耗时 ${ELAPSED}s)"
  exit 1
fi
if [ "$ELAPSED" -gt 70 ]; then
  nm_log "耗时 ${ELAPSED}s > 70s"
  exit 1
fi
if ! cat "$LOG_A" "$LOG_B" "$CLI_OUT_A" 2>/dev/null | grep -q "appears symmetric"; then
  nm_log "日志均无 \"local NAT appears symmetric\" 诊断(对称侧 punch_err 未触发)"
  exit 1
fi
nm_log "PASS $TYPE_A x $TYPE_B expect=fail (send_rc=$SEND_RC, ${ELAPSED}s)"
exit 0
