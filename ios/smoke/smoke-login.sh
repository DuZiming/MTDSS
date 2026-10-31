#!/usr/bin/env bash
# 登录冒烟(v2-P5 T4 brief Step 2-4):对模拟器内引擎(进程内 UDS,NDJSON
# JSON-RPC)执行 hello → login.request_code(dev 模式服务器回显验证码)→
# login.verify → hello(登录态)→ devices(设备列表可见)。
#
# 用法: ios/smoke/smoke-login.sh <daemon.sock> [email]   (email 缺省 kk@test.com)
#
# 口径:服务器/CA 用烤入缺省 https://www.mtdss.top(spec §4.6,不自定义);
# 每调用一条独立连接(id 恒 1,与 golden 逐字节锚同构;pending login 状态在
# daemon 侧,跨连接成立 —— crates/daemon/tests/login.rs 同款流程)。
# 依赖:nc -U(Unix socket)。

set -euo pipefail

SOCK="${1:?usage: smoke-login.sh <daemon.sock> [email]}"
EMAIL="${2:-kk@test.com}"
SERVER="https://www.mtdss.top"
NAME="iOS 模拟器"
WAIT="${MTDSS_SMOKE_WAIT:-20}"   # 引擎线程异步起:等 socket 出现的上界(秒)

# ---------- 工具 ----------

fail() { echo "FAIL: $*" >&2; exit 1; }
pass() { echo "PASS: $*"; }

# 等 socket 文件出现(引擎线程就绪后 bind 产文件)
wait_socket() {
  local i=0
  while [ ! -S "$SOCK" ]; do
    i=$((i + 1))
    [ "$i" -gt $((WAIT * 5)) ] && fail "socket 未出现($WAIT s): $SOCK —— App 已启动?MTDSS_HOME 是否一致?"
    sleep 0.2
  done
}

# rpc <method> <params-json> → stdout=result 原文(整体应答帧进 stderr 留档)
rpc() {
  local method="$1" params="$2"
  local frame resp
  frame="{\"jsonrpc\":\"2.0\",\"id\":1,\"method\":\"$method\",\"params\":$params}"
  resp="$(printf '%s\n' "$frame" | nc -U -N -w 10 "$SOCK")" || fail "$method: 连接/写失败"
  printf '%s\n' "$resp" >&2   # 线上原帧留档(stderr)
  case "$resp" in
    *'"error":'*) fail "$method: 收到 error 帧: $resp" ;;
    *'"result":'*) printf '%s\n' "$resp" ;;
    *) fail "$method: 应答非 result 帧: $resp" ;;
  esac
}

# ---------- 冒烟 ----------

[ -e "$SOCK" ] || wait_socket

# ① 引擎在线:hello 通且 logged_in 键在(新 home 未登录应为 false;保留 home
#    重跑则 true,不硬断言值,只验通话)
hello1="$(rpc hello '{}')"
echo "$hello1" | grep -q '"logged_in":' || fail "hello 缺 logged_in: $hello1"
pass "① hello 通(引擎在线)"

# ② 请求验证码(dev 模式服务器回显六位码)
rc="$(rpc login.request_code "{\"server\":\"$SERVER\",\"email\":\"$EMAIL\"}")"
CODE="$(printf '%s' "$rc" | grep -o '"code":"[0-9]\{6\}"' | head -1 | cut -d'"' -f4)"
[ -n "$CODE" ] || fail "login.request_code 未回显六位码: $rc"
pass "② 拿到 dev 验证码 $CODE"

# ③ 真登录
vf="$(rpc login.verify "{\"email\":\"$EMAIL\",\"code\":\"$CODE\",\"name\":\"$NAME\"}")"
DEVICE_ID="$(printf '%s' "$vf" | grep -o '"device_id":"[0-9a-f-]*"' | head -1 | cut -d'"' -f4)"
[ -n "$DEVICE_ID" ] || fail "login.verify 未得 device_id: $vf"
pass "③ 登录成功 device_id=$DEVICE_ID"

# ④ 登录态翻转:hello logged_in=true 且 device_id 回显
hello2="$(rpc hello '{}')"
echo "$hello2" | grep -q '"logged_in":true' || fail "登录后 hello 仍未登录: $hello2"
printf '%s' "$hello2" | grep -q "\"device_id\":\"$DEVICE_ID\"" || fail "hello 未回显 device_id: $hello2"
pass "④ hello 登录态=true,device_id 回显一致"

# ⑤ 设备列表可见(账号下至少本机一台)
dv="$(rpc devices '{}')"
echo "$dv" | grep -q '"devices":\[' || fail "devices 缺数组: $dv"
case "$dv" in *'"devices":[]'*) fail "设备列表为空: $dv" ;; esac
N=$(printf '%s' "$dv" | grep -o '"device_id"' | wc -l | tr -d ' ')
pass "⑤ 设备列表 $N 台"

echo "SMOKE OK: 引擎在线 → 真登录($EMAIL @ 产线)→ 设备列表可见"
