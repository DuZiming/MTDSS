# V3-6 实机场产物(v3p6 波,1.3.0 线)

分支 `v3p6` @ **`b6f7faf`**(fix(v3p6): 类型图标淡染规则 dark 排除;含
e66be65 引擎终审修复 + 8b48d66/b6f7faf Windows 主题裁定——**产物重出**,
替换 2026-11-12 早间 `0e20e2b`/`3d5c5bf` 基座旧包,全三端同一基线);
构建于 `.claude/worktrees/v3p6`,2026-11-12。二进制不入库(artifacts/
gitignore),本文件记档就绪面。

## Windows 冒烟包

- `Synshu_1.3.0-v3p6-x64-setup.exe`(NSIS,currentUser,6.87MB/6870413B)
  - sha256:`9852dc44c03e4c0c899778aae8cf87b138f450f1a9bf9f01890e30b3cf67cb27`
    (见 `Synshu_1.3.0-v3p6-x64-setup.exe.sha256`)
  - 源:`ui/src-tauri/target/release/bundle/nsis/Synshu_1.3.0_x64-setup.exe`(tauri bundle)
  - 构建链:`MTDSS_UPDATE_URL=https://www.mtdss.top/dl/synshu` +
    `MTDSS_UPDATE_PUBKEY=keys/update-pub.hex 内容(64 hex,剥 CR)`
    → `cargo build --release -p mtdss-cli -p mtdss-daemon`
    → `cd ui && npx tauri build`
  - **双串验证(V3-5 教训,过)**:安装器内嵌 `mtdssd.exe`(7z 抽出)与
    `target/release/mtdssd.exe` sha256 逐字节一致(`d199d346…`,b6f7faf
    新引擎),两串各 1 命中:`www.mtdss.top/dl/synshu` +
    `e0342ba5c4e7…9ca16`(pubkey 全 hex);引擎指纹 `armed 记忆命中`×5
    在场。运行时 env `MTDSS_UPDATE_URL` 可覆盖烤入 URL;pubkey 纯构建期。
  - 注:`keys/update-pub.hex` 为 CRLF 行尾,注入 env 前须剥 `\r`(否则
    daemon 判「非 hex/长度错」fail-closed)。

## Android APK

- `Synshu-v3p6-debug.apk`(debug,arm64-v8a,versionName 1.3.0 / versionCode 5,
  aapt badging 复核过:minSdk 29)
  - sha256:`dad3abbed7048664db55ea882014e4a28ece6a0de6b842f5405fbf279aa99294`
    (见 `Synshu-v3p6-debug.apk.sha256`)
  - 源:`android/app/build/outputs/apk/debug/app-debug.apk`(gradlew
    assembleDebug;引擎由 preBuild → cargoDaemonDebug 驱动:T1 锁定命令
    `cargo ndk -t arm64-v8a -P 29 -- build -p mtdss-daemon --release`,
    ANDROID_HOME 由 AGP SDK 目录显式注入;worktree 需 `android/local.properties`
    指 SDK——gitignore 不随仓,本波落 `sdk.dir=C:\Users\Administrator\AppData\Local\Android\Sdk`)
  - **引擎指纹验证(过)**:APK 内 `lib/arm64-v8a/libmtdssd.so` 含 V3-6 指纹串
    (`armed 记忆命中`×5、`回切:关闭中继半连`×1)与 `1.3.0`;且 .so 与旧包
    (0e20e2b 基座)逐字节不同(`c154f2ee…`/18578912B 对旧
    `7125a265…`/18574272B)——e66be65 引擎修复确认编入,e66be65 无新增
    字符串(纯逻辑改动),新鲜度以 mtime(构建时刻 22:04)+ 新旧 .so 差异
    双证
  - 更新源未注入(仓内 gradle.properties 空值口径;检查更新呈「未配置更新源」,
    冒烟面无碍;实机更新链验证归正式 release 签名链)

## iOS TestFlight(已上传 ✅)

- Mac mini 构建盒通道(`docs/dev-mac-mini.md`):worktree `git push macmini v3p6`
  → `~/mtdss-ios` fetch + reset --hard origin/v3p6(Mac HEAD 核对
  **`b6f7faf`** 一致)→ unlock 钥匙串(**同一 ssh 会话内**)→
  `MTDSS_DEVELOPMENT_TEAM=D4L57M3M67 MTDSS_ASC_USER=… MTDSS_ASC_PASSWORD=…
  MTDSS_BUILD_NUMBER=20261112.2 bash ios/build.sh device-sign`(归档+导出+
  ASC 直传一条链)。
- **结果(2026-11-12,重出)**:ARCHIVE SUCCEEDED → EXPORT SUCCEEDED →
  **UPLOAD SUCCEEDED**(Delivery UUID `31792b66-cce8-45a7-9842-36ea2a7058bf`,
  16.8MB);版本 1.3.0,build 号 **20261112.2**(同日第二传,env 显式指定
  保单调递增);ASC 处理 10-30min(服务端面),TestFlight 群组照旧。
  entitlements 双 target 校验过(`group.top.mtdss.shared`,team D4L57M3M67)。
- 坑两枚(2026-11-12 实录):①env 名是 `MTDSS_DEVELOPMENT_TEAM`
  (DEVELOPMENT,非 DEVELOPER——传错名=归档报 requires a development team);
  ②SSH 会话 codesign 报 `errSecInternalComponent` = 钥匙串又锁,
  **同一 ssh 会话内**先 `security unlock-keychain -p <Mac密码,见用户记忆
  mtdss-mac-mini-access,不入库> ~/Library/Keychains/login.keychain-db`
  再跑 build.sh(跨会话 unlock 不保真)。

## linux tar(2026-11-12 补齐)

- `mtdss-1.3.0-v3p6-x86_64-linux.tar.gz`(9073517B,mtdss+mtdssd+README.txt,
  tar tzf 三件核过)
  - sha256:`dc5abd69bbe6b663291e9780ebedb3854498adef1ba293ef489947f700ee481f`
    (见 `mtdss-1.3.0-v3p6-x86_64-linux.tar.gz.sha256`;命名沿 V3-5 先例
    `mtdss-1.2.1-v3p5b-…`)
  - 构建盒:WSL Ubuntu-22.04,独立 `CARGO_TARGET_DIR=/root/v3p6-target`;
    源码=worktree HEAD 归档导出件(WSL 内零版本控制操作);配方随目录留档
    `build-linux-v3p6.sh`(不入库)
  - 基线:**`da3dcb5`**(v3p6 PR 头;对二进制与 b6f7faf 同码——da3dcb5 仅
    README 增量);构建链=主仓 artifacts/build-linux.sh 口径平移:
    `MTDSS_SIGNALING_URL=https://www.mtdss.top` 构建期烤入 →
    `cargo build --release -p mtdss-cli -p mtdss-daemon` →
    `scripts/package/package-tar.sh`(x86_64-linux);更新通道按 CI tar
    裁定不注入(URL/PUBKEY 显式 unset 防宿主 env 泄漏)
  - **烤入验证(过)**:双二进制 `www.mtdss.top` 各 1 命中(strings 交叉
    复核一致);`mtdss-signaling.invalid`/更新链 `/dl/synshu`/公钥前缀
    `e0342ba5c4e7` 全 0 命中;`mtdssd` 引擎指纹 `armed 记忆命中`×5 +
    `回切:关闭中继半连`×1(`mtdss` cli 无中继串=预期,中继面全在
    daemon);`--version` 双 1.3.0
  - 坑:归档导出件带 Windows 检出 CRLF,package-tar.sh/Cargo.toml 须先
    归一化(脚本内已含该步);`CARGO_TARGET_DIR` 显式 export(漏导则落
    src/target 默认位)

## macOS tar(2026-11-12 补齐)

- `mtdss-1.3.0-v3p6-aarch64-macos.tar.gz`(7620107B,mtdss+mtdssd+README.txt,
  tar tzf 三件核过)
  - sha256:`b63259e38742bf354ff5e8f0a003ba48943f61bd07a636f05efc6d5a0e12cf7f`
    (见 `mtdss-1.3.0-v3p6-aarch64-macos.tar.gz.sha256`;Mac 侧 `shasum` 与
    PC 侧 `sha256sum` 逐字节一致)
  - 构建盒:Mac mini(Apple Silicon,macOS 26.3)经 ssh 隧道
    (docs/dev-mac-mini.md);配方随目录留档 `build-macos-v3p6.sh`(不入库)
  - 基线:macmini bare 已推 `b6f7faf..da3dcb5`,`~/mtdss-ios` fetch +
    reset --hard origin/v3p6 后 HEAD 核对 **`da3dcb5`** 一致;构建链同
    linux:`MTDSS_SIGNALING_URL` 烤入 + `cargo build --release -p
    mtdss-cli -p mtdss-daemon`(target/release 原生 aarch64)→
    `scripts/package/package-tar.sh`(aarch64-macos);更新通道不注入
    (env -u 双保险;~/mtdss-env.sh 本身无 MTDSS_* 变量已核)
  - **烤入验证(过)**:`mtdss`/`mtdssd` domain 1/1、`.invalid`/更新链
    0/0;`mtdssd` fp_armed×12 + `关闭中继半连`×1;`--version` 双 1.3.0
  - tar 构建链不需要钥匙串 unlock(无 codesign 环节)
