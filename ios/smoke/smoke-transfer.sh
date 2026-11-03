#!/usr/bin/env bash
# 传输链冒烟(v2-P5 T6):对模拟器内引擎驱动 remote_copy(转发 = 调度
# A→B,数据不经操作端;dir 给 B 端绝对收件目录——桌面 unix 对端无 App
# 发布应答,收件箱哨兵仅 Android/iOS 语义 + Windows 桌面回退,见 task-6
# 报告注记)。
#
# 用法: ios/smoke/smoke-transfer.sh <daemon.sock> <from_id> <remote_path> <to_id> <to_dir>
# 依赖: nc -U(macOS 自带即可,读半工见 EOF 即断,应答先行)。
set -euo pipefail

SOCK="${1:?usage: smoke-transfer.sh <daemon.sock> <from_id> <remote_path> <to_id> <to_dir>}"
FROM="$2"; RPATH="$3"; TO="$4"; TODIR="$5"
WAIT="${MTDSS_SMOKE_WAIT:-90}"

fail() { echo "FAIL: $*" >&2; exit 1; }
pass() { echo "PASS: $*"; }

rpc() { # 一调用一连接(id 恒 1,与 golden 锚同构;smoke-login.sh 同口径;
        # 只取应答行——引擎会在连接上先推 event 帧,事件里 "error":null
        # 不得误判成错误帧)
  local frame resp
  frame="{\"jsonrpc\":\"2.0\",\"id\":1,\"method\":\"$1\",\"params\":$2}"
  resp="$(printf '%s\n' "$frame" | nc -U -w 15 "$SOCK" | grep "\"id\":1" | tail -1)" || fail "$1: 连接/写失败"
  case "$resp" in
    *'"error":{"'*|*'"error":{'*) fail "$1: 收到 error 帧: $resp" ;;
    *'"result":'*) printf '%s\n' "$resp" ;;
    *) fail "$1: 应答非 result 帧: $resp" ;;
  esac
}

# ① 发起转发(remote_copy 契约形态;dir = B 端收件目录)
resp="$(rpc transfer.start "{\"cut\":false,\"dir\":\"$TODIR\",\"from\":\"$FROM\",\"kind\":\"remote_copy\",\"overwrite\":[],\"paths\":[\"$RPATH\"],\"skip\":[],\"to\":\"$TO\"}")"
JOB="$(printf '%s' "$resp" | grep -o '"job_id":"[^"]*"' | head -1 | cut -d'"' -f4)"
[ -n "$JOB" ] || fail "未得 job_id: $resp"
pass "① 转发已发起 job_id=$JOB (from=$FROM to=$TO dir=$TODIR)"

# ② 轮询 transfer.list 至该作业终态(id 唯一;按含 id 的片段抽 state)
i=0
while :; do
  list="$(rpc transfer.list '{}')"
  state="$(printf '%s' "$list" | tr '{' '\n' | grep "\"id\":\"$JOB\"" | grep -o '"state":"[^"]*"' | head -1 | cut -d'"' -f4 || true)"
  case "$state" in
    done) pass "② 转发终态 done"; exit 0 ;;
    failed|cancelled) fail "② 转发终态 $state" ;;
  esac
  i=$((i + 1))
  [ "$i" -gt "$WAIT" ] && fail "② 超时($WAIT x2s)仍未终态: $state / $list"
  sleep 2
done
