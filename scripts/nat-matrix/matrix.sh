#!/usr/bin/env bash
# matrix.sh — NAT 矩阵逐例执行(plan §4.2:6 纯锥组合 ×1 + 4 含对称组合
# ×2 = 14 例)+ auto-spawn 门禁 1 例(M3a Task 8:NM_AUTO_SPAWN_A=1,a 侧
# daemon 由 CLI send 自动拉起)+ 中继兜底 1 例(v2-P1c Task 8:NM_RELAY=1,
# symmetric×symmetric ok——直连穷尽自动走服务端中继)+ ctrl 中继远端浏览
# 1 例(v2-P1c 第二刀:NM_RELAY=1 NM_CTRL=1,同型对称对——send 前先经
# ctrl 通道远端浏览,兜底锚行钉死确经中继)+ 末尾 netem 混沌 4 例
# (Task 10:loss/delay/blackout/permfail,详见 chaos.sh;断言只按各自硬超时,
# 宁慢勿抖)。
# 含对称组合 ×2 = 同序对全新身份再跑一次:互拨模型预期与两侧设备 pubkey
# 键序无关(run_case 的 key-order 日志记录每跑的实际键序),双跑回归两种
# 键序下的两侧角色。
# fail 例由 run_case 注入小重试预算(MTDSS_TEST_MAX_RECONNECTS=1、
# MTDSS_TEST_BACKOFF_MS=500;打洞预算保持默认 10s——NAT 行为时序是测点)。
# 每例独立清场重建;单例失败不中断后续,末尾聚合退出码;最后统一 teardown。
# 用法(CI 与本机 Linux 复跑相同): sudo bash scripts/nat-matrix/matrix.sh
set -uo pipefail
HERE="$(cd "$(dirname "${BASH_SOURCE[0]}")" && pwd)"

FAILED=0
run_case() { # <typeA> <typeB> <expect>
  echo ""
  echo "=== nat-matrix case: $1 x $2 expect=$3 ==="
  if bash "$HERE/run_case.sh" "$1" "$2" "$3"; then
    echo "=== case PASS: $1 x $2 expect=$3 ==="
  else
    echo "=== case FAIL: $1 x $2 expect=$3 ===" >&2
    FAILED=$((FAILED + 1))
  fi
}

# 6 纯锥组合 ×1(默认调参)
run_case full full ok
run_case full restricted ok
run_case full port_restricted ok
run_case restricted restricted ok
run_case restricted port_restricted ok
run_case port_restricted port_restricted ok

# 4 含对称组合 ×2(全新身份再跑一次;预期与键序无关)
run_case full symmetric ok
run_case full symmetric ok
run_case restricted symmetric ok
run_case restricted symmetric ok
run_case port_restricted symmetric fail
run_case port_restricted symmetric fail
run_case symmetric symmetric fail
run_case symmetric symmetric fail

# chaos 4 例(Task 10):netem 损伤与断链场景,拓扑恒 port_restricted ×
# port_restricted;每例独立清场重建,与矩阵同生命周期(chaos.sh 内定稿)
run_chaos() { # <case>
  echo ""
  echo "=== nat-chaos case: $1 ==="
  if bash "$HERE/chaos.sh" "$1"; then
    echo "=== chaos PASS: $1 ==="
  else
    echo "=== chaos FAIL: $1 ===" >&2
    FAILED=$((FAILED + 1))
  fi
}
run_chaos loss10
run_chaos delay200
run_chaos blackout3
run_chaos permfail

# auto-spawn 门禁 1 例(M3a Task 8):a 侧不预起 daemon,CLI send 自动拉起
# ——CLI 自动拉起行为唯一的真实二进制覆盖(语义为 full×full ok)
run_auto_spawn() {
  echo ""
  echo "=== nat-matrix case (auto-spawn): full x full expect=ok NM_AUTO_SPAWN_A=1 ==="
  if NM_AUTO_SPAWN_A=1 bash "$HERE/run_case.sh" full full ok; then
    echo "=== case PASS: auto-spawn full x full ok ==="
  else
    echo "=== case FAIL: auto-spawn full x full ok ===" >&2
    FAILED=$((FAILED + 1))
  fi
}
run_auto_spawn

# 中继兜底 1 例(v2-P1c Task 8):对称×对称直连必穷尽,信令开通中继(NM_RELAY=1
# 门控:signaling 加 --relay-port/--relay-addr,dev@test 开通中继 + 限速
# 8192kbps,lib.sh::nm_start_servers 内生效)后传输面自动走服务端中继,预期
# 仍为 ok 全链——客户端打洞穷尽兜底路径唯一的真实二进制覆盖;run_case 的 ok
# 断言追加「中继兜底已建立」锚行(确经中继,而非直连侥幸成功)。
run_relay() {
  echo ""
  echo "=== nat-matrix case (relay): symmetric x symmetric expect=ok NM_RELAY=1 ==="
  if NM_RELAY=1 bash "$HERE/run_case.sh" symmetric symmetric ok; then
    echo "=== case PASS: relay symmetric x symmetric ok ==="
  else
    echo "=== case FAIL: relay symmetric x symmetric ok ===" >&2
    FAILED=$((FAILED + 1))
  fi
}
run_relay

# ctrl 中继远端浏览 1 例(v2-P1c 第二刀):中继兜底用例叠加 NM_CTRL=1——
# presence 后、send 前先经 ctrl 通道远端浏览(`mtdss ls`),对称×对称直连
# 必穷尽,此刻「中继兜底已建立」锚行只能来自 ctrl(run_case 断言)——ctrl
# 兜底路径唯一的真实二进制覆盖;浏览步之后再跑 ok 全链。
run_ctrl_relay() {
  echo ""
  echo "=== nat-matrix case (ctrl-relay): symmetric x symmetric expect=ok NM_RELAY=1 NM_CTRL=1 ==="
  if NM_RELAY=1 NM_CTRL=1 bash "$HERE/run_case.sh" symmetric symmetric ok; then
    echo "=== case PASS: ctrl-relay symmetric x symmetric ok ==="
  else
    echo "=== case FAIL: ctrl-relay symmetric x symmetric ok ===" >&2
    FAILED=$((FAILED + 1))
  fi
}
run_ctrl_relay

bash "$HERE/teardown.sh" >/dev/null 2>&1 || true

if [ "$FAILED" -gt 0 ]; then
  echo "nat-matrix: ${FAILED} case(s) FAILED" >&2
  exit 1
fi
echo "nat-matrix: all cases passed"
