# V3-6 实机场产物(v3p6 波,1.3.0 线)

分支 `v3p6` @ **`5c29006`**(**终版产物重出**,含 570eaae
`fetch_devices` 401 就地轮换自愈——daemon 面,五端内嵌 daemon 全部重编;
替换 2026-11-12 `b6f7faf`/`da3dcb5` 基座旧包,全五端同一基线);
构建于 `.claude/worktrees/v3p6`,2026-11-13。二进制不入库(artifacts/
无 ignore 规则,按先例 untracked 不 add),本文件记档就绪面。

## Windows 冒烟包

- `Synshu_1.3.0-v3p6-x64-setup.exe`(NSIS,currentUser,6900702B)
  - sha256:`444f5ddc5e1b8a8bf5f5289a525c4163ea9d376a93dbb8c9e9104f2bae20901f`
    (见 `Synshu_1.3.0-v3p6-x64-setup.exe.sha256`)
  - 源:`ui/src-tauri/target/release/bundle/nsis/Synshu_1.3.0_x64-setup.exe`(tauri bundle)
  - 构建链:`MTDSS_UPDATE_URL=https://www.mtdss.top/dl/synshu` +
    `MTDSS_UPDATE_PUBKEY=keys/update-pub.hex 内容(64 hex,剥 CR)`
    → `cargo build --release -p mtdss-cli -p mtdss-daemon`
    → `cd ui && npx tauri build`
  - **双串验证(V3-5 教训,过)**:安装器内嵌 `mtdssd.exe`(7z 抽出)与
    `target/release/mtdssd.exe` sha256 逐字节一致(`9bb9373b…`;旧包内嵌
    `d199d346…`@b6f7faf 已变——**570eaae 401 自愈确认编入**),两串各 1
    命中:`www.mtdss.top/dl/synshu` + `e0342ba5c4e7…9ca16`(pubkey 全
    hex);引擎指纹 `armed 记忆命中`×5 + `回切:关闭中继半连`×1;`mtdssd
    --version` 1.3.0。运行时 env `MTDSS_UPDATE_URL` 可覆盖烤入 URL;
    pubkey 纯构建期。
  - 注:`keys/update-pub.hex` 为 CRLF 行尾,注入 env 前须剥 `\r`(否则
    daemon 判「非 hex/长度错」fail-closed)。

## Android APK

- `Synshu-v3p6-debug.apk`(debug,arm64-v8a,versionName 1.3.0 / versionCode 5,
  aapt badging 复核过:minSdk 29)
  - sha256:`f90a4c67bded33ce0b8d1f3cff1a9cdd4fffc217343d4ced1da742b1ad037051`
    (见 `Synshu-v3p6-debug.apk.sha256`)
  - 源:`android/app/build/outputs/apk/debug/app-debug.apk`(gradlew
    assembleDebug;引擎由 preBuild → cargoDaemonDebug 驱动:T1 锁定命令
    `cargo ndk -t arm64-v8a -P 29 -- build -p mtdss-daemon --release`,
    ANDROID_HOME 由 AGP SDK 目录显式注入;worktree 需 `android/local.properties`
    指 SDK——gitignore 不随仓,既有落点 `sdk.dir=C:\Users\Administrator\AppData\Local\Android\Sdk`)
  - **引擎指纹验证(过)**:APK 内 `lib/arm64-v8a/libmtdssd.so` 含 V3-6 指纹串
    (`armed 记忆命中`×5、`回切:关闭中继半连`×1)与 `1.3.0`;.so 与
    jniLibs 副本 sha256 一致(`24d1b816…`/18659456B),对旧包(b6f7faf 基座)
    逐字节不同(旧 `c154f2ee…`/18578912B)。570eaae 无新增运行时字符串
    (401 面字符串全在 cfg(test)),新鲜度以 mtime(本轮构建时刻
    2026-11-13 11:24)+ 新旧 .so 差异双证——**401 自愈确认编入**
  - 更新源未注入(仓内 gradle.properties 空值口径;检查更新呈「未配置更新源」,
    冒烟面无碍;实机更新链验证归正式 release 签名链)

## iOS TestFlight(已上传 ✅)

- Mac mini 构建盒通道(`docs/dev-mac-mini.md`):worktree `git push macmini v3p6`
  (`da3dcb5..5c29006`)→ `~/mtdss-ios` fetch + reset --hard origin/v3p6
  (Mac HEAD 核对 **`5c29006`** 一致)→ unlock 钥匙串(**同一链内**)→
  `MTDSS_DEVELOPMENT_TEAM=D4L57M3M67 MTDSS_ASC_USER=… MTDSS_ASC_PASSWORD=…
  MTDSS_BUILD_NUMBER=20261113.1 bash ios/build.sh device-sign`(归档+导出+
  ASC 直传一条链)。
- **结果(2026-11-13,终版)**:ARCHIVE SUCCEEDED → entitlements 双 target
  校验过(`group.top.mtdss.shared`,team D4L57M3M67)→ EXPORT SUCCEEDED →
  **UPLOAD SUCCEEDED**(Delivery UUID `987a11a6-9104-4fe1-80c9-7af1dc7fbcc2`);
  版本 1.3.0,build 号 **20261113.1**(env 显式指定);ASC 处理 10-30min
  (服务端面),TestFlight 群组照旧(取最新一枚即可,旧 20261112.x 两枚被取代)。
- 坑两枚(2026-11-12 实录,本轮沿用规避):①env 名是 `MTDSS_DEVELOPMENT_TEAM`
  (DEVELOPMENT,非 DEVELOPER——传错名=归档报 requires a development team);
  ②SSH 会话 codesign 报 `errSecInternalComponent` = 钥匙串回锁,
  **同一链内**先 `security unlock-keychain -p <Mac密码,见用户记忆
  mtdss-mac-mini-access,不入库> ~/Library/Keychains/login.keychain-db`
  再跑 build.sh。

## linux tar(2026-11-13 终版重出)

- `mtdss-1.3.0-v3p6-x86_64-linux.tar.gz`(9112072B,mtdss+mtdssd+README.txt,
  tar tzf 三件核过)
  - sha256:`78e3957007d95401227344c8ed23bb729f55e2e0369c23c81ab9159968423e53`
    (见 `mtdss-1.3.0-v3p6-x86_64-linux.tar.gz.sha256`;旧 `dc5abd69…`
    被覆盖,同名规则沿 V3-5 先例 `mtdss-1.2.1-v3p5b-…`)
  - 构建盒:WSL Ubuntu-22.04,独立 `CARGO_TARGET_DIR=/root/v3p6-target`;
    源码=worktree HEAD(5c29006)归档导出件(WSL 内零版本控制操作);
    配方随目录留档 `build-linux-v3p6.sh`(不入库)
  - 基线:**`5c29006`**;构建链=主仓 artifacts/build-linux.sh 口径平移:
    `MTDSS_SIGNALING_URL=https://www.mtdss.top` 构建期烤入 →
    `cargo build --release -p mtdss-cli -p mtdss-daemon` →
    `scripts/package/package-tar.sh`(x86_64-linux);更新通道按 CI tar
    裁定不注入(URL/PUBKEY 显式 unset 防宿主 env 泄漏)
  - **烤入验证(过)**:双二进制 `www.mtdss.top` 各 1 命中;
    `mtdss-signaling.invalid`/更新链 `/dl/synshu`/公钥前缀
    `e0342ba5c4e7` 全 0 命中;`mtdssd` 引擎指纹 armed ×12 +
    `回切:关闭中继半连`×1(`mtdss` cli 无中继串=预期,中继面全在
    daemon);`--version` 双 1.3.0
  - 新鲜度:二进制 mtime 本轮构建时刻(2026-11-13 11:16/11:17),源 stamp
    `5c29006`;daemon 随 570eaae 源变重编
  - 坑:归档导出件带 Windows 检出 CRLF,package-tar.sh/Cargo.toml 须先
    归一化(脚本内已含该步);`CARGO_TARGET_DIR` 显式 export(漏导则落
    src/target 默认位)

## macOS tar(2026-11-13 终版重出)

- `mtdss-1.3.0-v3p6-aarch64-macos.tar.gz`(7662996B,mtdss+mtdssd+README.txt,
  tar tzf 三件核过)
  - sha256:`6e24e97792d617b63d60e0283d70e6897dd27f86a3a8a9c0c016fdb3de7f38e2`
    (见 `mtdss-1.3.0-v3p6-aarch64-macos.tar.gz.sha256`;旧 `b63259e3…`
    被覆盖;Mac 侧 `shasum` 与 PC 侧 `sha256sum` 逐字节一致)
  - 构建盒:Mac mini(Apple Silicon)经 ssh 隧道
    (docs/dev-mac-mini.md);配方随目录留档 `build-macos-v3p6.sh`(不入库);
    本轮与 TestFlight 同链编排(先 tar 后归档,零凭据脚本 + 凭据仅进启动
    进程 env)
  - 基线:macmini bare 已推 `da3dcb5..5c29006`,`~/mtdss-ios` fetch +
    reset --hard origin/v3p6 后 HEAD 核对 **`5c29006`** 一致;构建链同
    linux:`MTDSS_SIGNALING_URL` 烤入 + `cargo build --release -p
    mtdss-cli -p mtdss-daemon`(target/release 原生 aarch64)→
    `scripts/package/package-tar.sh`(aarch64-macos);更新通道不注入
    (env -u 双保险;~/mtdss-env.sh 本身无 MTDSS_* 变量已核)
  - **烤入验证(过)**:`mtdss`/`mtdssd` domain 1/1、`.invalid`/更新链
    0/0;`mtdssd` fp_armed×12 + `关闭中继半连`×1;`--version` 双 1.3.0;
    PART1 日志显见 `Compiling mtdss-daemon`(570eaae 源变重编)
  - tar 构建链不需要钥匙串 unlock(无 codesign 环节;本轮 unlock 与 ASC
    仅服务 TestFlight 段)
