#!/usr/bin/env bash
# MTDSS iOS 构建脚本(v2-P5 T4;Mac mini 构建盒,先 source ~/mtdss-env.sh)。
#
# 用法:
#   ios/build.sh sim [udid]     全链:sim 档 = 引擎静态库 → xcodegen → 模拟器
#                               构建 → 安装 → 启动(短 home,防 sun_path 104 上限)
#   ios/build.sh app [udid]     只构建到 .app,不装不启
#   ios/build.sh device         真机构建骨架(T13 实装签名装载;本档只验编译)
#   ios/build.sh device-sign    T13 签名产线:引擎 device slice → xcodegen →
#                               xcodebuild archive(自动签名,双 target,App
#                               Group 随 provisioning 更新注册)→ 导出 App
#                               Store .ipa → 校验 entitlements → 凭据在场则
#                               altool 传 TestFlight
#   ios/build.sh upload         只重传已导出的 .ipa(凭据后到时补传用)
#
# 环境变量:
#   MTDSS_SIGNALING_URL        烤入引擎的信令缺省(缺省 https://www.mtdss.top,
#                              与 1.1.0 release 同口径,docs/runbook-1.1.0.md §域
#                              名烤入)
#   MTDSS_SIM_HOME             模拟器引擎 home(缺省 ~/.mtdss-ios-sim;UDS=
#                              {home}/daemon.sock,冒烟脚本对它说话)
#   MTDSS_SIM_KEEP=1           启动前不清 home(保留登录态/任务;缺省清 → 冒烟
#                              可重复)
#   MTDSS_BUILD_NUMBER         build 号(缺省日期制 $(date +%Y%m%d).1;对同一
#                              版本须单调递增)
#   MTDSS_DEVELOPMENT_TEAM     开发者 team ID(缺省从 Xcode IDEProvisioningTeams
#                              自探;首次归档自动注册 bundle ID/App Group)
#   MTDSS_ASC_USER             TestFlight 上传的 Apple ID(缺省从钥匙串
#                              MTDSS_ASC 条目的 acct 属性自取)
#   MTDSS_ASC_PASSWORD         App 专用密码明文(最高优先——Xcode 26 altool
#                              @keychain 查询在 Local Items 幻影条目下报
#                              AuthenticationFailure,直传为既定成功路径;
#                              2026-11-07 V3-3 收官裁定,runbook §2 同步)
#   MTDSS_ASC_KEY_ID /         App Store Connect API key 路径(备选:
#   MTDSS_ASC_ISSUER_ID        ~/mtdss-asc-key.p8 + 两 ID;凭据 (a) 缺时用)
#
# 退出码非 0 即失败;模拟器档成功时打印 IPC socket 路径与冒烟用法。

set -euo pipefail

IOS_DIR="$(cd "$(dirname "${BASH_SOURCE[0]}")" && pwd)"
REPO_ROOT="$(cd "$IOS_DIR/.." && pwd)"
APP_DIR="$IOS_DIR/app"
APP_BUNDLE_ID="top.mtdss.ios"

MODE="${1:-sim}"
# 引擎静态库目标档:sim/app=模拟器 slice(aarch64-apple-ios-sim;与模拟器
# 平台 load command 相容),device/device-sign=真机 slice(aarch64-apple-ios)。
# 两 slice 机器码同源,平台标记不同,不可混链(ld64 拒 iOS 物件进模拟器镜像)。
case "$MODE" in
  device|device-sign) RUST_TARGET="aarch64-apple-ios" ;;
  *)                  RUST_TARGET="aarch64-apple-ios-sim" ;;
esac
ENGINE_LIB_SRC="$REPO_ROOT/target/$RUST_TARGET/release/libios_bridge.a"
ENGINE_LIB_DST="$APP_DIR/Engine/libios_bridge.a"

# ---------- 共用:引擎静态库 + 工程生成 ----------

build_engine_and_project() {
  echo "== 引擎静态库(ios-bridge → $RUST_TARGET)=="
  MTDSS_SIGNALING_URL="${MTDSS_SIGNALING_URL:-https://www.mtdss.top}" \
    cargo build --manifest-path "$REPO_ROOT/Cargo.toml" \
      -p ios-bridge --target "$RUST_TARGET" --release
  mkdir -p "$APP_DIR/Engine"
  cp "$ENGINE_LIB_SRC" "$ENGINE_LIB_DST"

  echo "== xcodegen 生成工程 =="
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

# ---------- device-sign 档辅助(T13)----------

resolve_asc_team() { # team ID:env → Xcode 记录 → ASC 查询(凭据在场时;
                     # 个人账号通常仅一队,ProviderId=Team ID)
  local team="${MTDSS_DEVELOPMENT_TEAM:-}"
  if [ -z "$team" ]; then
    team="$(defaults read com.apple.dt.Xcode IDEProvisioningTeams 2>/dev/null \
      | sed -n 's/.*teamID = "\([A-Z0-9]*\)".*/\1/p' | head -1)"
  fi
  if [ -z "$team" ]; then
    local user="${MTDSS_ASC_USER:-}"
    if [ -z "$user" ] && security find-generic-password -s MTDSS_ASC >/dev/null 2>&1; then
      user="$(security find-generic-password -s MTDSS_ASC 2>/dev/null \
        | sed -n 's/.*"acct"<blob>="\(.*\)"/\1/p')"
    fi
    if [ -n "$user" ]; then
      team="$(xcrun altool --list-providers -u "$user" -p "@keychain:MTDSS_ASC" 2>/dev/null \
        | grep -oE "(ProviderId|Team ID): ?[A-Za-z0-9]+" | grep -oE "[A-Za-z0-9]+$" | head -1 || true)"
      if [ -n "$team" ]; then
        echo "team 自探经 ASC(ProviderId): $team" >&2
      fi
    fi
  fi
  echo "$team"
}

verify_entitlements() { # $1 = .app/.appex 路径;打 entitlements 并钉 App Group
  local path="$1" ents
  echo "== entitlements 校验: $path =="
  ents="$(codesign -d --entitlements - --xml "$path" 2>/dev/null \
    || codesign -d --entitlements :- "$path" 2>/dev/null || true)"
  echo "$ents"
  echo "$ents" | grep -q "group.top.mtdss.shared" \
    || { echo "缺 App Group entitlement(group.top.mtdss.shared)→ 失败" >&2; return 1; }
}

write_export_options() { # $1 = 输出 plist;$2 = team ID(可空——归档已钉 team
                         # 时 exportArchive 缺省沿用归档同一 team)
  local team_line=""
  if [ -n "${2:-}" ]; then
    team_line="  <key>teamID</key><string>$2</string>"
  fi
  cat > "$1" <<EOF
<?xml version="1.0" encoding="UTF-8"?>
<!DOCTYPE plist PUBLIC "-//Apple//DTD PLIST 1.0//EN" "http://www.apple.com/DTDs/PropertyList-1.0.dtd">
<plist version="1.0">
<dict>
  <key>method</key><string>app-store-connect</string>
  <key>uploadSymbols</key><true/>
  <key>signingStyle</key><string>automatic</string>
$team_line
</dict>
</plist>
EOF
}

find_exported_ipa() { # 最新导出的 .ipa(build/export 下唯一定期重导,取新;
                      # || true 防无产物时 pipefail 使调用方赋值炸 set -e)
  ls -t "$APP_DIR"/build/export/*.ipa 2>/dev/null | head -1 || true
}

asc_upload() { # $1 = .ipa;凭据次序:(a) MTDSS_ASC_PASSWORD 直传 (b) 钥匙串
               # MTDSS_ASC (c) ASC API key。返回 3 = 无凭据(非失败)。
  local ipa="$1" p8="$HOME/mtdss-asc-key.p8" have_kc=0
  if [ -n "${MTDSS_SKIP_UPLOAD:-}" ]; then
    # 分权闸(R5/J2 起):实现者只出 ipa,TestFlight 上传归控制器(其会话
    # 注 env 凭据跑 build.sh upload)。设 MTDSS_SKIP_UPLOAD=1 即按无凭据
    # 语义跳过(return 3 → device-sign 打印「上传待凭据」收口)。
    echo "上传跳过:MTDSS_SKIP_UPLOAD=1(实现者出 ipa;上传归控制器)" >&2
    return 3
  fi
  if [ -n "${MTDSS_ASC_PASSWORD:-}" ] && [ -n "${MTDSS_ASC_USER:-}" ]; then
    echo "== TestFlight 上传(altool 直传,Apple ID: $MTDSS_ASC_USER)=="
    xcrun altool --upload-app -f "$ipa" -t ios \
      -u "$MTDSS_ASC_USER" -p "$MTDSS_ASC_PASSWORD"
    return $?
  fi
  if security find-generic-password -s MTDSS_ASC >/dev/null 2>&1; then
    have_kc=1
  fi
  if [ "$have_kc" = 1 ] || [ -n "${MTDSS_ASC_USER:-}" ]; then
    local user="${MTDSS_ASC_USER:-}"
    if [ -z "$user" ]; then
      user="$(security find-generic-password -s MTDSS_ASC 2>/dev/null \
        | sed -n 's/.*"acct"<blob>="\(.*\)"/\1/p')"
    fi
    if [ -z "$user" ]; then
      echo "钥匙串 MTDSS_ASC 在场但无 acct 属性 → 设 MTDSS_ASC_USER 后重试 upload 档" >&2
      return 2
    fi
    echo "== TestFlight 上传(altool,Apple ID: $user)=="
    xcrun altool --upload-app -f "$ipa" -t ios -u "$user" -p "@keychain:MTDSS_ASC"
  elif [ -f "$p8" ] && [ -n "${MTDSS_ASC_KEY_ID:-}" ] && [ -n "${MTDSS_ASC_ISSUER_ID:-}" ]; then
    echo "== TestFlight 上传(altool,ASC API key $MTDSS_ASC_KEY_ID)=="
    mkdir -p "$HOME/private_keys"
    cp "$p8" "$HOME/private_keys/AuthKey_$MTDSS_ASC_KEY_ID.p8"
    xcrun altool --upload-app -f "$ipa" -t ios \
      --apiKey "$MTDSS_ASC_KEY_ID" --apiIssuer "$MTDSS_ASC_ISSUER_ID"
  else
    echo "上传跳过:无凭据(钥匙串 MTDSS_ASC 与 ~/mtdss-asc-key.p8 均缺)——" \
         "archive+export 已完成,凭据到位后跑 ios/build.sh upload 补传" >&2
    return 3
  fi
}

archive_and_export() {
  local team bn="${MTDSS_BUILD_NUMBER:-$(date +%Y%m%d).1}"
  team="$(resolve_asc_team)"
  echo "== [3/5] xcodebuild archive(Release,自动签名;build $bn;team ${team:-<自动>})=="
  local team_arg=""
  if [ -n "$team" ]; then
    team_arg="DEVELOPMENT_TEAM=$team"
  fi
  xcodebuild -project "$APP_DIR/MTDSS.xcodeproj" \
    -scheme MTDSS -configuration Release \
    -destination "generic/platform=iOS" \
    -derivedDataPath "$APP_DIR/build/dd-archive" \
    -archivePath "$APP_DIR/build/MTDSS.xcarchive" \
    ${team_arg:+"$team_arg"} \
    CURRENT_PROJECT_VERSION="$bn" \
    -allowProvisioningUpdates -allowProvisioningDeviceRegistration \
    archive

  echo "== [4/5] entitlements + 导出 =="
  local app="$APP_DIR/build/MTDSS.xcarchive/Products/Applications/MTDSS.app"
  verify_entitlements "$app"
  if [ ! -d "$app/PlugIns/MTDSSShare.appex" ]; then
    echo "归档缺分享扩展 appex → 失败" >&2
    exit 1
  fi
  verify_entitlements "$app/PlugIns/MTDSSShare.appex"

  mkdir -p "$APP_DIR/build/export"
  rm -rf "$APP_DIR"/build/export/*.ipa
  local opts="$APP_DIR/build/exportOptions.plist"
  write_export_options "$opts" "$team"
  xcodebuild -exportArchive \
    -archivePath "$APP_DIR/build/MTDSS.xcarchive" \
    -exportOptionsPlist "$opts" \
    -exportPath "$APP_DIR/build/export" \
    -allowProvisioningUpdates

  local ipa rc=0
  ipa="$(find_exported_ipa)"
  if [ -z "$ipa" ]; then
    echo "导出未产 .ipa → 失败" >&2
    exit 1
  fi
  echo "ipa: $ipa ($(du -h "$ipa" | cut -f1))"
  echo "== [5/5] TestFlight 上传 =="
  asc_upload "$ipa" || rc=$?
  if [ "$rc" = 3 ]; then
    echo "device-sign 完成(归档+导出;上传待凭据)"
  elif [ "$rc" != 0 ]; then
    exit "$rc"
  fi
}


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
    echo "device 编译通过(产物不装载;签名归 device-sign 档)"
    ;;
  device-sign)
    build_engine_and_project
    archive_and_export
    ;;
  upload)
    ipa="$(find_exported_ipa)"
    if [ -z "$ipa" ]; then
      echo "没有已导出的 .ipa(先跑 ios/build.sh device-sign)" >&2
      exit 1
    fi
    echo "ipa: $ipa"
    asc_upload "$ipa" || exit $?
    echo "上传完成:ASC/TestFlight 处理中(App Store Connect → 我的 App → " \
         "TestFlight → iOS builds,处理 10-30 分钟)"
    ;;
  *)
    sed -n '2,26p' "${BASH_SOURCE[0]}" >&2
    exit 2
    ;;
esac
