# V3-6 实机场产物(v3p6 波,1.3.0 线)

分支 `v3p6` @ `0e20e2b`(chore(v3p6): 版本 1.3.0);构建于
`.claude/worktrees/v3p6`,2026-11-12。二进制不入库(artifacts/ gitignore),
本文件记档就绪面。

## Windows 冒烟包

- `Synshu_1.3.0-v3p6-x64-setup.exe`(NSIS,currentUser)
  - sha256:见 `Synshu_1.3.0-v3p6-x64-setup.exe.sha256`
  - 源:`ui/src-tauri/target/release/bundle/nsis/Synshu_1.3.0_x64-setup.exe`(tauri bundle)
  - 构建链:`MTDSS_UPDATE_URL=https://www.mtdss.top/dl/synshu` +
    `MTDSS_UPDATE_PUBKEY=keys/update-pub.hex 内容(64 hex,剥 CR)`
    → `cargo build --release -p mtdss-cli -p mtdss-daemon`
    → `cd ui && npx tauri build`
  - **双串验证(V3-5 教训,过)**:安装器内嵌 `mtdssd.exe`(7z 抽出)与
    `target/release/mtdssd.exe` sha256 一致(`964bee9d…`),两串各 1 命中:
    `www.mtdss.top/dl/synshu` + `e0342ba5c4e7…9ca16`(pubkey 全 hex)。
    运行时 env `MTDSS_UPDATE_URL` 可覆盖烤入 URL;pubkey 纯构建期。
  - 注:`keys/update-pub.hex` 为 CRLF 行尾,注入 env 前须剥 `\r`(否则
    daemon 判「非 hex/长度错」fail-closed)。

## Android APK

- `Synshu-v3p6-debug.apk`(debug,arm64-v8a,versionName 1.3.0 / versionCode 5,
  aapt badging 复核过:minSdk 29)
  - sha256:见 `Synshu-v3p6-debug.apk.sha256`
  - 源:`android/app/build/outputs/apk/debug/app-debug.apk`(gradlew assembleDebug;
    引擎由 preBuild → cargoDaemonDebug 驱动:T1 锁定命令
    `cargo ndk -t arm64-v8a -P 29 -- build -p mtdss-daemon --release`,
    ANDROID_HOME 由 AGP SDK 目录显式注入;worktree 需 `android/local.properties`
    指 SDK——gitignore 不随仓,本波落 `sdk.dir=C:\Users\Administrator\AppData\Local\Android\Sdk`)
  - **引擎指纹验证(过)**:APK 内 `lib/arm64-v8a/libmtdssd.so` 含 V3-6 指纹串
    (`armed 记忆命中`×5、`回切:关闭中继半连`×1)与 `1.3.0`,即本波引擎非陈物
  - 更新源未注入(仓内 gradle.properties 空值口径;检查更新呈「未配置更新源」,
    冒烟面无碍;实机更新链验证归正式 release 签名链)

## iOS TestFlight(已上传 ✅)

- Mac mini 构建盒通道(`docs/dev-mac-mini.md`):worktree `git push macmini v3p6`
  → `~/mtdss-ios` fetch + reset --hard origin/v3p6(`0e20e2b`)→ unlock 钥匙串
  → `MTDSS_DEVELOPMENT_TEAM=D4L57M3M67 MTDSS_ASC_USER=… MTDSS_ASC_PASSWORD=…
  bash ios/build.sh device-sign`(归档+导出+ASC 直传一条链)。
- **结果(2026-11-12)**:ARCHIVE SUCCEEDED → EXPORT SUCCEEDED →
  **UPLOAD SUCCEEDED**(Delivery UUID `220e13fd-6306-4a32-abe9-60e0d4f8f776`,
  16.8MB/2.1s);版本 1.3.0,build 号缺省日期制 **20261112.1**;ASC 处理 10-30min
  (服务端面),TestFlight 群组照旧。entitlements 双 target 校验过
  (`group.top.mtdss.shared`,team D4L57M3M67)。
- 坑两枚(2026-11-12 实录):①env 名是 `MTDSS_DEVELOPMENT_TEAM`
  (DEVELOPMENT,非 DEVELOPER——传错名=归档报 requires a development team);
  ②SSH 会话 codesign 报 `errSecInternalComponent` = 钥匙串又锁,
  **同一 ssh 会话内**先 `security unlock-keychain -p <Mac密码,见用户记忆
  mtdss-mac-mini-access,不入库> ~/Library/Keychains/login.keychain-db`
  再跑 build.sh(跨会话 unlock 不保真)。

## linux tar

随需未出(v3p6 引擎面无 linux 专属增量;需要时沿 CI release.yml build-linux
job 口径 `cargo build --release -p mtdss-cli -p mtdss-daemon` +
`scripts/package/package-tar.sh`,tar 平台不注入更新公钥=更新 fail-closed 设计态)。
