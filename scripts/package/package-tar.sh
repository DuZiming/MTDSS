#!/usr/bin/env bash
# MTDSS Linux/macOS tar 发布件打包:把已构建的 mtdss / mtdssd 二进制与 README.txt
# 打成 mtdss-<ver>-<platform>.tar.gz(版本号取仓库根 Cargo.toml 的 workspace version)。
# 用法: package-tar.sh <产物目录> <x86_64-linux|aarch64-macos> [输出目录]
#   产物目录:须已含 `cargo build --release -p mtdss-cli -p mtdss-daemon` 的两个二进制
#   (macOS 交叉形态为 target/aarch64-apple-darwin/release/,由调用方传入对应目录;
#    CI release workflow 与本地用法一致:先构建,后打包)。
# 兼容性:CI(ubuntu/macos runner)与本地通用,保持 POSIX-sh 语法(无 bashism)。
set -eu

usage() {
  echo "usage: $0 <artifact-dir> <x86_64-linux|aarch64-macos> [out-dir]" >&2
  exit 2
}

[ $# -ge 2 ] && [ $# -le 3 ] || usage
artifact_dir=$1
platform=$2
out_dir=${3:-$1}

case $platform in
  x86_64-linux | aarch64-macos) ;;
  *) usage ;;
esac

script_dir=$(CDPATH= cd -- "$(dirname -- "$0")" && pwd) || exit 1
repo_root=$(CDPATH= cd -- "$script_dir/../.." && pwd) || exit 1

# 版本号:根 Cargo.toml 为虚拟 workspace(无 [package]),唯一 ^version = 行即 workspace.package
ver=$(sed -n 's/^version = "\(.*\)"$/\1/p' "$repo_root/Cargo.toml" | head -n 1)
[ -n "$ver" ] || {
  echo "error: cannot read workspace version from $repo_root/Cargo.toml" >&2
  exit 1
}

[ -d "$artifact_dir" ] || { echo "error: artifact dir not found: $artifact_dir" >&2; exit 1; }

stage=$(mktemp -d)
trap 'rm -rf "$stage"' EXIT INT TERM

for bin_name in mtdss mtdssd; do
  if [ ! -f "$artifact_dir/$bin_name" ]; then
    echo "error: $artifact_dir/$bin_name missing; run: cargo build --release -p mtdss-cli -p mtdss-daemon" >&2
    exit 1
  fi
  cp "$artifact_dir/$bin_name" "$stage/$bin_name"
  chmod 755 "$stage/$bin_name"
done

cat >"$stage/README.txt" <<'EOF'
MTDSS 命令行版(Linux/macOS tar 包)

解压即用:./mtdss --help 查看客户端用法;./mtdssd 为后台常驻进程(建议配合 --no-tray --log-file 运行)。注意:tar 包平台无自动更新能力,升级需手动下载新包替换。
EOF

mkdir -p "$out_dir"
out="$out_dir/mtdss-$ver-$platform.tar.gz"
tar -czf "$out" -C "$stage" mtdss mtdssd README.txt
echo "$out"
