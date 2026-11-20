# Synshu 1.5.0 测试包(V4-1 预览波收官,2026-11-20)

> **本目录 README 入库(v4p1 分支);二进制+侧车留盘不入库**(主 checkout 同名目录)。
> 产物基座:v4p1@4868e6e(1.5.0 版本链 c0bc38c clippy 清面先行)。iOS TestFlight 直传挂控制器(本任务不代传)。

- **Win** `Synshu_1.5.0_x64-setup.exe`(8.28 MiB,NSIS currentUser)
  sha256 `5ba7e010560005b2573611323199a1750eb6d1cda96e619fb34226abbdeb8e4e`
  (更新链双 env 烤入 `MTDSS_UPDATE_URL=https://www.mtdss.top/dl/synshu` +
  `MTDSS_UPDATE_PUBKEY=e0342ba5…9ca16`(keys/update-pub.hex 剥 CR;VPS 现役 manifest
  同源——1.3.0→1.4.0 自动弹更新实证该 keypair 与现役签名者一致);双串验证过(V3-5 教训):
  7z 抽出内嵌 `mtdssd.exe` 与 `target/release/mtdssd.exe` sha256 逐字节一致
  (`699b7f30…`),URL 串与 PUBKEY hex 串在内嵌 mtdssd 各 1 命中,`1.5.0` ×37;
  VersionInfo.ProductVersion=1.5.0)
- **Android** `Synshu-1.5.0-debug.apk`(62.9 MB,debug,arm64-v8a)
  sha256 `160cce7fa721aed83b8cf6aed1d52a242f20ac17c85d8d3755a70646082f9df1`
  (aapt 复核 versionCode 7 / versionName 1.5.0 / minSdk 29;引擎指纹:APK 内
  `lib/arm64-v8a/libmtdssd.so` 含 `1.5.0` ×9;assets/previewer/previewer.js 与
  ui/previewer/dist 逐字节一致[5,337,707 B]=非陈物)
- **Linux x86_64** `mtdss-1.5.0-x86_64-linux.tar.gz`(9.7 MB)
  sha256 `b73b53550ce3d3c2b0bd16728376095cff1784956cfed8ef1246e78aa6dbb42f`
  (WSL 全量 release 构建[未烘焙更新串=tar 平台无自动更新,fail-closed 设计态];
  package-tar.sh 打包,产物名版本号自 Cargo.toml workspace version)
- **macOS aarch64** `mtdss-1.5.0-aarch64-macos.tar.gz`(8.1 MB)
  sha256 `bbe042c920b87a7d1308429b1db2d6ed69791f25fd2e50dcca9b4cffb9c91e75`
  (Mac mini 构建盒,`cargo build --release --target aarch64-apple-darwin` +
  package-tar.sh,同 CI build-macos job 口径)
- **iOS**:TestFlight 直传挂控制器(env 直传档,`ios/build.sh` upload;版本 1.5.0,
  build 号缺省日期制 20261120.x)。Mac mini 侧 `ios/build.sh app` BUILD SUCCEEDED
  (sim 壳预验,previewer bundle 资产在包)。

全量绿记录:Rust workspace Windows+WSL 双平台(unset 四 env;unix 门 e2e 全家)/
ui vitest 763+build+previewer/gradlew JVM 543+assembleDebug/Mac swift 全套 XCTest
passed(1.5.0 断言三面)/clippy -D warnings 双平台净;golden REGEN 零漂移。
实机场冒烟清单:`docs/smoke-v4p1.md`(spec §8 八条验收线编列)。
