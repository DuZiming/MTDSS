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

## r11(2026-11-23,弹窗样式 CSP 根因修)

- **Win** `Synshu-1.5.0-r11-x64-setup.exe`(9.47 MiB,NSIS currentUser)
  sha256 `664eb9c2bcce5071b52bb3810a91ee0c327e8b865e60b815278474f7452c20c2`
  (双 env 烤入同 1.5.0 基线:URL/PUBKEY 内嵌 mtdssd 各 1 命中;侧车 sha256
  与 target/release 逐字节一致;`1.5.0`×37;heif 三 DLL 随包)
- **根因与修**:弹窗「内容错乱/无布局」=tauri 服务面 CSP 资产改写剥离
  style-src 'unsafe-inline',previewer 运行时注入 `<style>` 恒 sheet=null
  (r10d-r10f 自复现钩子+WinRT OCR 活体探针钉面);修=样式改构建期静态
  css(`@mtdss/previewer/styles.css`),桌面 previewer.html `<link>` 同源
  加载,Android/iOS 资产目录同步入库+gradle/build.sh 校验扩 css 面。
  包内 exe 端到端验证:弹窗错误态居中 muted 渲染(旧态=左上黑字堆叠)。
- 主窗 devtools 自开随诊断撤;MTDSS_POPUP_TEST 归因钩子保留至波尾清场。

## r12(2026-11-23,弹窗交互六裁定)

- **Win** `Synshu-1.5.0-r12-x64-setup.exe`(9.48 MiB)
  sha256 `ef7fa20a914011cd7577633c723742e9aceb8fd9adc663997e4fdef9f1dd3a5b`
  (验证同 r11:侧车逐字节/双串/1.5.0×37/三 DLL;capabilities 修复与新 css
  面入包确认)
- **六裁定落地**:①小地图视口矩形钳到图面(fit 态蓝框=全图恒居中,不再
  飞出图外)②③Esc/X=真销毁+「适配」退役→恢复 100% 图标钮(内联 SVG,
  点击倍率回 100% 平移归零)④**多开**:每弹窗独立窗(标签池
  preview-popout…-8,全忙逐最旧;首发走 config 预建窗 show 面,后续运行时
  创建=自愈同款已证路径)⑤面板常开换源补 fav 位(FavView onFileClick,
  dir 位三视图既有)⑥面板布局:缩略图弹性生长为主内容+整体垂直居中
- **关窗链根因二连(实测定形)**:capabilities glob `preview-popout-*` 不匹
  配裸标签 `preview-popout`(plugin:window|close 被 ACL 拒)→ 补裸标签;
  close() 与 CloseRequested 事件互喂死循环(invoke OK 而窗不亡)→ 页面侧
  恒 preventDefault + destroy() 强销毁(allow-destroy 入册)。X(WM_CLOSE)
  双弹窗销毁已包内端到端验证。
- 测试:previewer 18/18(image 新增 3:钳制/部分可见/恢复钮)+ui 767/767
  (previewer-main 关窗语义重写)。

## r13(2026-11-23,倍率基准+面板静态封面)

- **Win** `Synshu-1.5.0-r13-x64-setup.exe`(9.48 MiB)
  sha256 `62580931015c8b0327a23be9ff4fe54f479f907e3c842ca483a21b935d14b7d8`
  (验证同 r12:侧车逐字节/双串/1.5.0×37;previewer.js/css 双端资产同步刷新)
- **①初始倍率=加载后的原大 100%**:setSrc onLoad 不再 fitView——fitScale
  仍算(滚轮下探底线/双击切换基准),zoom 恒以 1 起步平移归零;thumb→raw
  替换面各归各重置,倍率标签全程 100% 基准不再闪「先 100 后适配」。大图
  100% 溢出可中键拖,双击切 fit。钉面:image.test 大图 scale=1+双击往返。
- **④面板静态封面**(previewer 新 mountCover,面板 ⧉ 弹窗分工不变):
  image/video=服务端 thumb 海报帧 <img>(失败=图标);pdf=pdf.js 首页
  canvas(URL 直连 http Range 只拉首页字节);text/md=Range 头 4KiB 前 24
  行;docx/xlsx/pptx=完整挂载裁剪静态呈现(尺寸门 ≤25MiB,超门/失败=图
  标,office 流式票落地后可撤);audio=音符图标;unsupported=文件图标。
  Panel thumb 槽改 cover 宿主,载荷 buildPreviewRequest 单源。测试:
  cover.test 7 例 + panel 断言翻新;包 117/117、ui 774/774。
