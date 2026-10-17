#!/usr/bin/env bash
# payload.sh — 生成发送载荷:4 文件 ×3MiB(/dev/urandom),合计 ~12MiB,
# 并把 sha256 清单打到 stdout(排序稳定,供收发两侧比对)。
# 用法: payload.sh <dir>
set -euo pipefail
dir="${1:?usage: payload.sh <dir>}"
mkdir -p "$dir"
i=1
while [ "$i" -le 4 ]; do
  dd if=/dev/urandom of="$dir/f$i.bin" bs=1M count=3 status=none
  i=$((i + 1))
done
(
  cd "$dir" &&
    sha256sum -- f1.bin f2.bin f3.bin f4.bin | LC_ALL=C sort
)
