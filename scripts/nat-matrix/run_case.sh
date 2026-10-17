#!/usr/bin/env bash
# run_case.sh <typeA> <typeB> <expect:ok|fail>
#
# 单用例:teardown → setup(两侧 NAT 类型) → 起信令/STUN → 双侧注册(dev
# 验证码回环)→ b 侧 recv(后台)→ presence 轮询 → a 侧 send → 按预期断言。
# exit 0 = 行为与预期一致;非 0 = 不一致或环境故障(失败先倒两侧完整日志)。
#
# expect=ok  : send exit 0 + 双侧 sha256 清单一致 + 两侧日志各 grep 到
#              srflx=[203.0.113.2 / srflx=[203.0.113.6。
# expect=fail: send exit 非 0 + 日志含 symmetric + 耗时 ≤ 30s。
# fail 例两侧子进程注入小重试预算(plan Task 9:MAX_RECONNECTS=1、
# BACKOFF_MS=500;打洞预算保持默认 10s——NAT 时序是测点)。
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

nm_require_env
NM_PIDS=()
RECV_PID=""

CASE_DIR="$(mktemp -d "${TMPDIR:-/tmp}/mtdss-nm.XXXXXX")"
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
  for p in ${NM_PIDS[@]+"${NM_PIDS[@]}"}; do kill "$p" 2>/dev/null || true; done
  for p in ${NM_PIDS[@]+"${NM_PIDS[@]}"}; do wait "$p" 2>/dev/null || true; done
  if [ "$rc" -ne 0 ]; then
    nm_log "FAIL($TYPE_A x $TYPE_B expect=$EXPECT) — 转储完整日志:"
    for f in "$CASE_DIR/signaling.log" "$CASE_DIR/stun.log" "$LOG_B" "$LOG_A"; do
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
nm_stun_probe "$NM_NS_PUB" "$NM_STUN_PORT" || nm_die "STUN 探针 pub:$NM_STUN_PORT 不可达"
nm_stun_probe "$NM_NS_A" "$NM_STUN_PORT" || nm_die "STUN 探针 a:$NM_STUN_PORT 不可达"
nm_stun_probe "$NM_NS_A" "$NM_STUN_ALT" || nm_die "STUN 探针 a:$NM_STUN_ALT 不可达"
nm_stun_probe "$NM_NS_B" "$NM_STUN_PORT" || nm_die "STUN 探针 b:$NM_STUN_PORT 不可达"
nm_stun_probe "$NM_NS_B" "$NM_STUN_ALT" || nm_die "STUN 探针 b:$NM_STUN_ALT 不可达"
nm_log "stun probes ok (pub/a/b x $NM_STUN_PORT,$NM_STUN_ALT)"

# ③ 双侧注册(同账号两台设备;身份种子各自落在 case 临时 home)
nm_register "$NM_NS_A" "$H_A" dev@test mtdss-a || nm_die "注册 a 失败"
nm_register "$NM_NS_B" "$H_B" dev@test mtdss-b || nm_die "注册 b 失败"
DEV_B="$(nm_json_str "$H_B/session.json" device_id)"
[ -n "$DEV_B" ] || nm_die "拿不到 b 的 device_id"

# ④ b 侧接收(后台,日志落文件)
mkdir -p "$OUT_B"
RECV_ENV=(env MTDSS_HOME="$H_B" RUST_LOG="${NM_RUST_LOG:-info}")
if [ "$EXPECT" = fail ]; then
  RECV_ENV+=(MTDSS_TEST_MAX_RECONNECTS=1 MTDSS_TEST_BACKOFF_MS=500)
fi
ip netns exec "$NM_NS_B" "${RECV_ENV[@]}" \
  "$MTDSS_BIN" recv --out "$OUT_B" --ca "$NM_CA_PEM" >"$LOG_B" 2>&1 &
RECV_PID=$!
NM_PIDS+=("$RECV_PID")

# presence 就绪(plan:轮询 devices 在线;上限 20s,超时按环境故障处理)
nm_wait_online "$DEV_B" "$H_A" || nm_die "b($DEV_B) 未在信令目录上线"

# ⑤ a 侧发送(带硬超时兜底防挂死;正常成功远小于该值)
SEND_ENV=(env MTDSS_HOME="$H_A" RUST_LOG="${NM_RUST_LOG:-info}")
if [ "$EXPECT" = fail ]; then
  SEND_ENV+=(MTDSS_TEST_MAX_RECONNECTS=1 MTDSS_TEST_BACKOFF_MS=500)
fi
T0="$(date +%s)"
set +e
timeout "${NM_SEND_TIMEOUT_S:-180}" ip netns exec "$NM_NS_A" "${SEND_ENV[@]}" \
  "$MTDSS_BIN" send "$PAYLOAD_DIR"/f1.bin "$PAYLOAD_DIR"/f2.bin \
  "$PAYLOAD_DIR"/f3.bin "$PAYLOAD_DIR"/f4.bin \
  --to "$DEV_B" --ca "$NM_CA_PEM" >"$LOG_A" 2>&1
SEND_RC=$?
set -e
T1="$(date +%s)"
ELAPSED=$((T1 - T0))

# ok 例:收侧一轮完成即自退;给它留收尾时间(fail 例收侧常驻等窗,cleanup 负责杀)
if [ "$EXPECT" = ok ]; then
  WAITED=0
  while kill -0 "$RECV_PID" 2>/dev/null && [ "$WAITED" -lt 60 ]; do
    sleep 1
    WAITED=$((WAITED + 1))
  done

  # ⑥ 断言(exit 0 = 退出码 / srflx 候选 / sha256 清单三关全过)
  if [ "$SEND_RC" -ne 0 ]; then
    nm_log "send 退出码 $SEND_RC(预期 0,耗时 ${ELAPSED}s)"
    exit 1
  fi
  grep -q "srflx=\[$NM_GWA_EXT_IP" "$LOG_A" ||
    { nm_log "a 侧日志缺 srflx=[$NM_GWA_EXT_IP(Task 5 候选行)"; exit 1; }
  grep -q "srflx=\[$NM_GWB_EXT_IP" "$LOG_B" ||
    { nm_log "b 侧日志缺 srflx=[$NM_GWB_EXT_IP(Task 5 候选行)"; exit 1; }
  (
    cd "$OUT_B" &&
      find . -maxdepth 1 -type f -name '*.bin' -printf '%f\n' |
      LC_ALL=C sort | xargs -r sha256sum
  ) | LC_ALL=C sort >"$CASE_DIR/manifest.dst"
  if ! diff -u "$CASE_DIR/manifest.src" "$CASE_DIR/manifest.dst"; then
    nm_log "双侧 sha256 清单不一致"
    exit 1
  fi
  nm_log "PASS $TYPE_A x $TYPE_B expect=ok (${ELAPSED}s)"
  exit 0
fi

# ⑥' fail 例断言:exit 非 0 + 日志含 symmetric + 耗时 ≤ 30s
if [ "$SEND_RC" -eq 0 ]; then
  nm_log "send 退出码 0(预期非 0,耗时 ${ELAPSED}s)"
  exit 1
fi
if [ "$ELAPSED" -gt 30 ]; then
  nm_log "耗时 ${ELAPSED}s > 30s"
  exit 1
fi
if ! cat "$LOG_A" "$LOG_B" 2>/dev/null | grep -q "symmetric"; then
  nm_log "两侧日志均无 symmetric 标记"
  exit 1
fi
nm_log "PASS $TYPE_A x $TYPE_B expect=fail (send_rc=$SEND_RC, ${ELAPSED}s)"
exit 0
