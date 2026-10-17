#!/usr/bin/env bash
# matrix.sh — NAT 矩阵逐例执行。当前仅 full×full 冒烟一行(Task 9 扩到 14 例,
# 末尾再追加 chaos)。
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

run_case full full ok

bash "$HERE/teardown.sh" >/dev/null 2>&1 || true

if [ "$FAILED" -gt 0 ]; then
  echo "nat-matrix: ${FAILED} case(s) FAILED" >&2
  exit 1
fi
echo "nat-matrix: all cases passed"
