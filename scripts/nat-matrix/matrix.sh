#!/usr/bin/env bash
# matrix.sh — NAT 矩阵逐例执行(plan §4.2:6 纯锥组合 ×1 + 4 含对称组合
# ×2 = 14 例)+ auto-spawn 门禁 1 例(M3a Task 8:NM_AUTO_SPAWN_A=1,a 侧
# daemon 由 CLI send 自动拉起)+ 末尾 netem 混沌 4 例(Task 10:loss/delay/
# blackout/permfail,详见 chaos.sh;断言只按各自硬超时,宁慢勿抖)。
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

bash "$HERE/teardown.sh" >/dev/null 2>&1 || true

if [ "$FAILED" -gt 0 ]; then
  echo "nat-matrix: ${FAILED} case(s) FAILED" >&2
  exit 1
fi
echo "nat-matrix: all cases passed"
