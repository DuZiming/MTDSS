#!/usr/bin/env bash
# MTDSS iOS 构建脚本(v2-P5 T4;Mac mini 构建盒,先 source ~/mtdss-env.sh)。
#
# 用法:
#   ios/build.sh sim [udid]   全链:sim 档 = 引擎静态库 → xcodegen → 模拟器
#                             构建 → 安装 → 启动(短 home,防 sun_path 104 上限)
#   ios/build.sh app [udid]   只构建到 .app,不装不启
#   ios/build.sh device       真机构建骨架(T13 实装签名装载;本档只验编译)
#
# 环境变量:
#   MTDSS_SIGNALING_URL  烤入引擎的信令缺省(缺省 https://www.mtdss.top,
#                        与 1.1.0 release 同口径,docs/runbook-1.1.0.md §域 名烤入)
#   MTDSS_SIM_HOME       模拟器引擎 home(缺省 ~/.mtdss-ios-sim;UDS=
#                        {home}/daemon.sock,冒烟脚本对它说话)
#   MTDSS_SIM_KEEP=1     启动前不清 home(保留登录态/任务;缺省清 → 冒烟可重复)
#
# 退出码非 0 即失败;模拟器档成功时打印 IPC socket 路径与冒烟用法。

set -euo pipefail

IOS_DIR="$(cd "$(dirname "${BASH_SOURCE[0]}")" && pwd)"
REPO_ROOT="$(cd "$IOS_DIR/.." && pwd)"
APP_DIR="$IOS_DIR/app"
APP_BUNDLE_ID="top.mtdss.ios"

MODE="${1:-sim}"
# 引擎静态库目标档:sim/app=模拟器 slice(aarch64-apple-ios-sim;与模拟器
# 平台 load command 相容),device=真机 slice(aarch64-apple-ios)。两 slice
# 机器码同源,平台标记不同,不可混链(ld64 拒 iOS 物件进模拟器镜像)。
case "$MODE" in
  device) RUST_TARGET="aarch64-apple-ios" ;;
  *)      RUST_TARGET="aarch64-apple-ios-sim" ;;
esac
ENGINE_LIB_SRC="$REPO_ROOT/target/$RUST_TARGET/release/libios_bridge.a"
ENGINE_LIB_DST="$APP_DIR/Engine/libios_bridge.a"

# ---------- 共用:引擎静态库 + 工程生成 ----------

build_engine_and_project() {
  echo "== [1/3] 引擎静态库(ios-bridge → $RUST_TARGET)=="
  MTDSS_SIGNALING_URL="${MTDSS_SIGNALING_URL:-https://www.mtdss.top}" \
    cargo build --manifest-path "$REPO_ROOT/Cargo.toml" \
      -p ios-bridge --target "$RUST_TARGET" --release
  mkdir -p "$APP_DIR/Engine"
  cp "$ENGINE_LIB_SRC" "$ENGINE_LIB_DST"

  echo "== [2/3] xcodegen 生成工程 =="
  (cd "$APP_DIR" && xcodegen generate)
}

build_app() { # $1 = xcodebuild 目的地串;$2 = 附加 setting(可空)
  echo "== [3/3] xcodebuild 构建($1)=="
  xcodebuild -project "$APP_DIR/MTDSS.xcodeproj" \
    -scheme MTDSS -configuration Debug \
    -destination "$1" \
    -derivedDataPath "$APP_DIR/build/dd" \
    ${2:+$2} build
  echo "产物: $APP_DIR/build/dd/Build/Products/Debug-iphonesimulator/MTDSS.app"
}

# ---------- sim 档辅助 ----------

resolve_sim_udid() { # $1 = 可选显式 udid
  if [ -n "${1:-}" ]; then
    echo "$1"
    return
  fi
  xcrun simctl list devices available \
    | sed -n 's/.*iPhone 17 Pro Max (\([A-F0-9-]*\)).*/\1/p' | head -1
}

sim_install_and_launch() { # $1 = udid
  local udid="$1"
  local sim_home="${MTDSS_SIM_HOME:-$HOME/.mtdss-ios-sim}"
  local app="$APP_DIR/build/dd/Build/Products/Debug-iphonesimulator/MTDSS.app"

  echo "== 模拟器:boot + 安装 + 启动 =="
  xcrun simctl boot "$udid" 2>/dev/null || true
  xcrun simctl bootstatus "$udid" -b
  xcrun simctl install "$udid" "$app"

  if [ "${MTDSS_SIM_KEEP:-0}" != "1" ]; then
    rm -rf "$sim_home"
  fi
  mkdir -p "$sim_home"

  # 短 home 经环境变量注入(DEBUG 档 App 侧读取;真机不设走 Documents)。
  # 引擎线程异步起:UDS 出现由冒烟脚本/调用方轮询。
  SIMCTL_CHILD_MTDSS_HOME="$sim_home" xcrun simctl launch "$udid" "$APP_BUNDLE_ID"

  echo ""
  echo "sim 档完成:udid=$udid  引擎home=$sim_home"
  echo "IPC socket(轮询至出现): $sim_home/daemon.sock"
  echo "登录冒烟: ios/smoke/smoke-login.sh $sim_home/daemon.sock [email]"
}

case "$MODE" in
  app)
    build_engine_and_project
    # generic 模拟器目的地默认双 slice(x86_64 模拟器无引擎库)→ 钉 arm64
    build_app "generic/platform=iOS Simulator" "ARCHS=arm64"
    ;;
  sim)
    build_engine_and_project
    udid="$(resolve_sim_udid "${2:-}")"
    [ -n "$udid" ] || { echo "找不到可用 iPhone 模拟器(可显式传 udid)" >&2; exit 1; }
    build_app "id=$udid" ""
    sim_install_and_launch "$udid"
    ;;
  device)
    echo "device 档:T13 签名装载(本档只验编译,CODE_SIGNING_ALLOWED=NO)"
    build_engine_and_project
    xcodebuild -project "$APP_DIR/MTDSS.xcodeproj" \
      -scheme MTDSS -configuration Debug \
      -destination "generic/platform=iOS" \
      -derivedDataPath "$APP_DIR/build/dd-device" \
      CODE_SIGNING_ALLOWED=NO build
    echo "device 编译通过(产物不装载;签名归 T13)"
    ;;
  *)
    sed -n '2,12p' "${BASH_SOURCE[0]}" >&2
    exit 2
    ;;
esac
