# MTDSS Android 客户端:构建 / 签名 / 发布

android/(独立 Gradle 工程,与 cargo workspace 平级)的本地构建、release 签名、
16KB 对齐检查与直发渠道(VPS `/dl`)发布步骤。工具链与流程口径见 spec
`docs/superpowers/specs/2026-10-25-mtdss-v2p3-android-design.md` §7;实机验收
清单见 `docs/smoke-p3.md`。

## 1. 本地构建

前置(与 CI android job 同面):JDK 21、Android SDK(platform 37 / build-tools
36.0.0 / NDK 30.0.16248370)、rustup target `aarch64-linux-android`、
cargo-ndk(本机 4.1.2)。Gradle 发行与 Maven 依赖走国内镜像(见
`android/settings.gradle.kts` 头注释),开箱即用。

```powershell
# android/ 目录下(JAVA_HOME 指 Android Studio 自带 JBR 亦可,同代 21)
./gradlew testDebugUnitTest   # JVM 单测(IPC 契约 golden + 各 VM/逻辑面)
./gradlew assembleDebug       # debug APK(AGP 自动生成 debug keystore 签名)
```

引擎(mtdssd)交叉编译由 `preBuild → cargoDaemonDebug` 自动驱动,无需手工
预跑;锁定命令(T1 裁定,`-P 29` 大写**不可省**——缺省 API 21 的目标缺
`getifaddrs`,链接失败):

```
cargo ndk -t arm64-v8a -P 29 -- build -p mtdss-daemon --release
```

产物:

| 变体 | 路径 | 说明 |
| --- | --- | --- |
| debug | `android/app/build/outputs/apk/debug/app-debug.apk` | 开发/实机调试装 |
| release | `android/app/build/outputs/apk/release/app-release-unsigned.apk` | 签名属性未注入时的缺省产物(fail-visible) |
| release(已签名) | `android/app/build/outputs/apk/release/app-release.apk` | 四项签名属性齐备时(见 §3) |

## 2. 更新源注入(MTDSS_DL_BASE / MTDSS_UPDATE_PUBKEY)

应用内「设置 ▸ 检查更新」(spec §6)在构建期注入两项 Gradle 属性(镜像 M4
daemon 的 `MTDSS_UPDATE_URL` / `MTDSS_UPDATE_PUBKEY` 构建期注入口径;App 侧
入口 = `BuildConfig`,经 `android/gradle.properties` 注入,提交形态为**空值**):

| 属性 | 值 | 缺省(空)的语义 |
| --- | --- | --- |
| `MTDSS_DL_BASE` | 更新分发基址,如 `https://<域名或IP>/dl`(与服务器侧 GH vars `MTDSS_DL_BASE` 同源;**域名备案前不得硬编码 IP 入仓**) | 检查更新呈「未配置更新源」(fail-closed,按钮无效) |
| `MTDSS_UPDATE_PUBKEY` | 验签公钥 hex(`keys/update-pub.hex` 同一枚,全平台共用同一 ed25519 信任链) | 同上 |

注入途径(优先级从高到低):命令行 `-P` > `android/gradle.properties`(仓内
提交空值)> `~/.gradle/gradle.properties`。本地覆盖示例:

```powershell
$pub = (Get-Content ..\keys\update-pub.hex -Raw).Trim()
./gradlew assembleDebug "-PMTDSS_DL_BASE=https://<host>/dl" "-PMTDSS_UPDATE_PUBKEY=$pub"
```

验证:产物内 `BuildConfig.MTDSS_DL_BASE` 应为注入值(源码见
`android/app/build/generated/source/buildConfig/`)。CI android job 不注入任何
更新源属性——构建不依赖 secrets,APK 即未配置态。

## 3. release 签名(keystore 用户离线保管)

APK 签名密钥与 M4 的 ed25519 更新签名密钥**同规格管理**(spec §7):私钥
用户离线保管,不入仓、不进 CI(CI 只 assembleDebug)。四项签名属性齐备才挂
`signingConfig`;缺任一 `assembleRelease` 产未签名 APK(fail-visible,不会
静默用 debug 键)。

1. 离线机生成(有效期 30 年,RSA 2048;`keytool` 任意 JDK 自带):

   ```
   keytool -genkeypair -v -keystore mtdss-release.keystore -alias mtdss ^
     -keyalg RSA -keysize 2048 -validity 10950
   ```

2. keystore 文件离线保管(备份!),注入四项属性(命令行 `-P` 或
   `~/.gradle/gradle.properties`,**不写入仓内文件**):

   | 属性 | 值 |
   | --- | --- |
   | `MTDSS_RELEASE_STORE_FILE` | keystore 路径(相对 android/ 或绝对) |
   | `MTDSS_RELEASE_STORE_PASSWORD` | keystore 口令 |
   | `MTDSS_RELEASE_KEY_ALIAS` | `mtdss` |
   | `MTDSS_RELEASE_KEY_PASSWORD` | 密钥口令 |

3. `./gradlew assembleRelease` → `app-release.apk`。

## 4. 16KB page 对齐检查

16KB 内核页设备(Android 15+ 的部分新硬件;上架前置)要求 ELF 加载段
16KB 对齐。本项目两条面都已实测满足(2026-10-26,NDK r30 / build-tools 36.0.0
/Rust stable 1.99 工具链):

1. **引擎 ELF(mtdssd 的真实门,exec 形态)**——`useLegacyPackaging = true`
   下 .so 压缩存 APK、安装期解包,运行时由内核按 ELF 程序头加载,
   `llvm-readelf -l libmtdssd.so` 全部 LOAD 段 `Align 0x4000`(= 16384):

   ```
   <NDK>/toolchains/llvm/prebuilt/windows-x86_64/bin/llvm-readelf.exe -l ^
     android/app/src/main/jniLibs/arm64-v8a/libmtdssd.so
   ```

   NDK r27+ 的 lld 对 arm64 缺省 `max-page-size=16384`,Rust 侧无需额外旗标。

2. **APK zip 对齐(上架面,切非压缩打包后才是硬门)**——当前压缩存档形态
   没有未压缩 .so 条目,检查通过属预期;将来若切 `useLegacyPackaging=false`
   (商店上架常形),此检查升级为硬门:

   ```
   <SDK>/build-tools/36.0.0/zipalign.exe -c -P 16 -v 4 <apk>
   ```

   (注:`-P` 与 `-p` 互斥;`<align>` 位参数 `4` 不可省,省了只打印用法。)

## 5. 发布到直发渠道(VPS /dl)

更新分发根 = `/var/www/mtdss/dl/`(Caddy `/dl/*` 静态目录,布局与
`scripts/deploy/README.md`「更新发布目录规约」同源)。安卓新增平级目录:

```
/var/www/mtdss/dl/
├── manifest.json                  # Windows 最新版(release workflow 维护,既有)
├── win/<ver>/…                    # 既有
└── android/
    ├── manifest.json              # Android 最新版签名清单(App 检查更新拉这里)
    └── <ver>/                     # 如 android/0.1.1/,按版本沉淀
        ├── MTDSS_<ver>_arm64.apk
        └── manifest.json          # 版本钉扎位(手动发布时一并放,供回滚重装)
```

App 侧检查 URL = `{MTDSS_DL_BASE}/android/manifest.json`(直发渠道安卓最新
位;Windows 客户端互不干扰)。清单的平台键为 **`android-arm64`**(对应 daemon
侧 `windows-x86_64` 键形),验签/复核语义与 M4 逐字节同源(
`crates/core/src/update.rs` ↔ `android/.../update/UpdateCheck.kt`,golden 字符串
在 UpdateCheckTest 逐字节锁定)。

发布步骤(手工;与 Windows 发布共用 `mtdss-sign`,密钥对即现有
update-priv.hex / update-pub.hex):

1. `./gradlew assembleRelease`(签名属性注入,见 §3)→ 取 `app-release.apk`,
   更名 `MTDSS_<ver>_arm64.apk`(`versionName` 须严格 `X.Y.Z`)。
2. 生成清单。**注意**:`mtdss-sign manifest` 生成的资产 URL 硬编码 `/win/`
   段(v1 已知债),安卓发布用「生成 → 改 url → 签名」绕开,或直接手写:

   ```
   mtdss-sign manifest --version <ver> --url-base https://<host>/dl/android ^
     --notes-file <notes.txt> --file MTDSS_<ver>_arm64.apk ^
     --platform android-arm64 --out unsigned.json
   # 把 unsigned.json 内 asset url 的 /win/<ver>/ 改为 /android/<ver>/(sha256/
   # size/version 不动,sign 按 canonical 域重签,改 url 不影响验签有效性)
   mtdss-sign sign --manifest unsigned.json --key update-priv.hex --out manifest.json
   mtdss-sign verify --manifest manifest.json --pubkey update-pub.hex
   ```

3. 上传:`manifest.json` → `/dl/android/manifest.json`(新版钉扎位同放
   `/dl/android/<ver>/manifest.json`),APK → `/dl/android/<ver>/`。
4. 旧客户端实测:已装 App 点「检查更新」→ 发现新版 → 下载 → 拉起安装器
   (验收判定见 `docs/smoke-p3.md` F3)。

发布提醒(沿 `docs/smoke-m4.md` §3 同款):VPS 拓扑在域名备案前为纯 IP +
自签 CA,`MTDSS_DL_BASE` 指向 `https://<IP>/dl` 时,App 侧信任该自签证书需
注意——`HttpURLConnection` 走系统信任库,自签 CA 需导入设备用户证书库,或
演练期暂以 `http://`(Caddy 按需加 80 明文口)过渡;正式形态以域名 + 公网
CA 为准。
