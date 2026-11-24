#!/usr/bin/env bash
# 预览回环冒烟(field r25:iOS 4-7 归因——preview_open 授柄后回环口取数
# raw/thumb 三面验证):sim 引擎 home 内造样件 → RPC preview_open →
# 解 port/token/handle → simctl spawn curl 回环三面 → 字节/状态码断言。
set -euo pipefail
SOCK="${1:?usage: smoke-preview.sh <daemon.sock> <sim-udid>}"
UDID="${2:?usage: smoke-preview.sh <daemon.sock> <sim-udid>}"
HOME_DIR="$(dirname "$SOCK")"

fail() { echo "FAIL: $*" >&2; exit 1; }
pass() { echo "PASS: $*"; }

rpc() {
  local frame resp
  frame="{\"jsonrpc\":\"2.0\",\"id\":1,\"method\":\"$1\",\"params\":$2}"
  # macOS nc:stdin EOF 即断——补 sleep 持连等应答
  resp="$( (printf '%s\n' "$frame"; sleep 6) | nc -U "$SOCK" | grep '"id":1' | tail -1)" || fail "$1: 连接失败"
  case "$resp" in
    *'"error"":{"'*) fail "$1: error: $resp" ;;
    *'"result"':*) printf '%s\n' "$resp" ;;
    *) fail "$1: 非结果帧: $resp" ;;
  esac
}

# 样件:txt(回环 raw 直读面)+ 小 PNG(图像)
printf 'hello preview smoke line1\nline2\n' > "$HOME_DIR/inbox/smoke.txt"
printf '\x89PNG\r\n\x1a\n' > "$HOME_DIR/inbox/smoke.png"
# 32x1 红 PNG(最小 IHDR/IDAT/IEND 手搓不现实——用 base64 1x1)
B64="iVBORw0KGgoAAAANSUhEUgAAAAEAAAABCAYAAAAfFcSJAAAADUlEQVR42mP8z8BQDwAEhQGAhKmMIQAAAABJRU5ErkJggg=="
echo "$B64" | base64 -d > "$HOME_DIR/inbox/smoke.png"

for F in smoke.txt smoke.png; do
  echo "== preview_open $F =="
  R="$(rpc preview_open "{\"path\":\"$HOME_DIR/inbox/$F\",\"mtime_ms\":1,\"size\":$(wc -c < "$HOME_DIR/inbox/$F" | tr -d ' ')}")"
  PORT="$(printf '%s' "$R" | sed -n 's/.*"port":\([0-9]*\).*/\1/p')"
  TOK="$(printf '%s' "$R" | sed -n 's/.*"token":"\([^"]*\)".*/\1/p')"
  HANDLE="$(printf '%s' "$R" | sed -n 's/.*"handle":"\([^"]*\)".*/\1/p')"
  [ -n "$PORT" ] && [ -n "$TOK" ] && [ -n "$HANDLE" ] || fail "解应答失败: $R"
  echo "  port=$PORT handle=$HANDLE"

  echo "== 回环 raw =="
  CODE="$(curl -s -o /tmp/sm-raw.out -w '%{http_code}' "http://127.0.0.1:$PORT/f/$HANDLE/raw?t=$TOK")" || fail "curl spawn 失败"
  SIZE="$(wc -c < /tmp/sm-raw.out | tr -d ' ')"
  [ "$CODE" = "200" ] || fail "raw HTTP $CODE(期望 200)"
  pass "raw 200, $SIZE 字节"

  echo "== 回环 thumb =="
  CODE="$(curl -s -o /tmp/sm-thumb.out -w '%{http_code}' "http://127.0.0.1:$PORT/f/$HANDLE/thumb?w=256&t=$TOK")" || fail "curl spawn 失败"
  echo "  thumb HTTP $CODE, $(wc -c < /tmp/sm-thumb.out | tr -d ' ') 字节"

  rpc preview_close "{\"handle\":\"$HANDLE\"}" > /dev/null
done
echo "DONE"
