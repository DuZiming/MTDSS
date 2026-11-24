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

## r14(2026-11-23,四发现收口)

- **Win** `Synshu-1.5.0-r14-x64-setup.exe`(9.48 MiB)
  sha256 `ab78bdccb91c3d582f8e6809e60b70a168c0fbfbc35467d27bd78dec78c424fe`
  (验证同 r13;previewer- 资产引用 ×3=主窗 css 链接入包钉面)
- **①小地图**:`--hidden` 起步 + applyTransform 按 fit 切换——整图可见
  (小图 100%/fit 态)恒隐藏(全图框无信息量),部分可见才亮相(蓝框=
  真实可见区;100% 下大图框小属实);thumb 渐进段不再出小地图,消除
  「整图框→小框」跳变。
- **②主窗封面布局**:根因=主窗未载 previewer styles.css(cover 元素零
  样式:靠上溢出/与元数据叠字)→ main.tsx 补 import(弹窗各自引入同
  源);ini「16 进制」=UTF-16LE 被按 UTF-8 误读 → 文本封面 BOM 嗅探
  (FF FE→utf-16le / FE FF→utf-16be / EF BB BF→剥 BOM,头 U+FEFF 剥离)。
- **③图标类封面对齐主界面**:audio/不支持/超门 office(>25MiB)不进
  cover,面板直渲染主界面三视图统一 EntryIcon(FileList 同款 theme 变量
  SVG,76px 居中)——文件夹图标同源(面板不对目录开,零成本自带)。
- **④视频桌面直连兜底**:playlist 404/410(桌面 v1 无转码后端)→
  `<video>` 直连 raw(Range 渐进播,H.264/AAC 等兼容编码原生可看);
  编解码不支持才落失败面(HEVC 引导词此境属实);重试钮重启走 hls
  预检。移动端转码在位不经此臂。
- 测试:previewer 120/120(BOM 2 例+视频直连 2 例)、ui 781/781(面板
  图标门 3 例)。

## r15(2026-11-23,六发现收口)

- **Win** `Synshu-1.5.0-r15-x64-setup.exe`(9.48 MiB)
  sha256 `f322ca474bdf4d78fd9a629503a3ee78d5c8d67fa7c173bec51ffa9112c4fb04`
  (验证同 r14:侧车逐字节/双串/1.5.0×37/previewer- ×3)
- **①小地图框「过小」真根因**:`.mtdss-preview__img` 的 CSS max-w/h +
  flex-shrink 把 100% 的大图**偷偷压回容器**(所见=全图,缩放数学=23%
  可见,框与画面背离)——撤钳制+`flex:none`,100%=真原大(溢出可中键
  拖),蓝框恒=真实可见区。
- **②文本查看器三件套**:右侧进度轨(视口位置+点击/拖拽跳转,无滚动
  自隐);md 左侧目录(h1-h3 收集/缩进/点击跳转/滚动跟随高亮,≥2 标题
  才出,续读重渲染随刷);**Ctrl+F 文档内搜索**(防抖高亮 mark、Enter/
  Shift+Enter 上下处、计数 n/m、Esc 关闭清高亮;txt/代码/md 通用)。
- **③视频未播放即撑满**:media 元素恒 width/height 100% + object-fit
  contain(信箱化;海报帧同面;音频条 height:auto 回退)。
- **④面板工具栏**:撤 ⤓ 下载与 ↗ 用其他应用打开(用户裁定);四钮
  图标化=主壳线性图标族(IcBack/IcForward/新 IcPopout/IcX)。
- **⑤‹› 光标跟随**:步进换源同时单选列表行+锚点+scrollIntoView
  (previewStep,data-row-path 命中)。
- **⑥文件夹进面板**:目录行单击换源→信息面(文件夹图标[EntryIcon]
  +文件夹名/大小—/格式文件夹/三时间),零 preview_open 零 cover,⧉ 隐藏;
  dir/favdir/fav 三路行点击均接。
- 测试:previewer 123/123(三件套 3 例)、ui 784/784(面板两测翻新+
  文件夹模式+图标合同清单 IcPopout)。

## r16(2026-11-23,三发现收口:弹窗陈缓存根因)

- **Win** `Synshu-1.5.0-r16-x64-setup.exe`(9.47 MiB)
  sha256 `60159f909e96fa55cd6c7af1b15b7ce41c1434495a3558f450d44b3fc25ea64a`(验证:侧车逐字节/双串/1.5.0×37/nonce 串入包/三件套
  css ×3;包内 exe 双弹+X 销毁端到端过)
- **①②真根因=WebView2 对 tauri.localhost 入口页的启发式缓存**:升级安装
  不清用户数据,旧版 previewer.html(URL 恒定)+旧 hash JS 整体命中陈缓
  存 → 弹窗跑旧代码(三件套「没做出来」的现场形态)。Chrome 干净服务
  复现证当前构建三件套全正常(md 目录 4 项/进度轨按内容高度)。修=弹窗
  页 URL 带每次运行 nonce(config 预建窗首发强制导航一次,运行时建窗
  直接带参)——入口 URL 变即缓存必 miss。
- **③‹› 双灰根因=步进域只含文件**:混合目录点过文件夹(r15 #6)后面板
  源=文件夹,不在纯文件域 → idx=-1 双灰。修=步进域=当前列表全部行
  (含目录,踩到=文件夹信息面,可穿行)。
- **面板 md 封面渲染格式**(顺带 #2 前半):md 封面走 markdown-it+
  DOMPurify 与弹窗同管线(text.ts renderMarkdownHtml 公开门),不再裸
  源文。
- 测试:previewer 123/123(cover 切 jsdom 系 DOMPurify 环境注记)、
  ui 784/784。

## r17(2026-11-24,三发现收口)

- **Win** `Synshu-1.5.0-r17-x64-setup.exe`(9.48 MiB)
  sha256 `845d4c8c518c3346fa4582038c57c017680257f9fd1f7ef22f179595146a60c9`
  (验证:侧车逐字节/双串/1.5.0×37/nonce 串;Chrome 实渲截图过目:TOC+
  缩微 minimap+视口浮层+md 块面无叠错、滚动同步比例正确)
- **①快捷目录根分组 ‹› 恒灰**:fav 根位此前不在步进域——接入(favRows
  渲染行上抛现值,行形统一 {device,path,entry} 逐行携设备跨源步进;选择
  键=fav 位复合键 device\0path,data-row-path 命中同步)。
- **③文本 minimap 重做(VS Code 式)**:整档缩微克隆(按宽缩放,行宽∝
  行长)+ 半透明视口浮层 + 点击/拖拽跳转;替 r15 细轨。修自锁缺陷
  (--hidden 起步下 clientHeight=0,先量后显永不放行→先判滚动先显后量)。
- **③目录扩 txt**:启发式大纲(#前缀/第N[章节篇部回]/Chapter N/N. 标题,
  ≤200 行窗;≥2 项才出;跳转=行高估算,pre 不折行恒行高)。
- **②md 块面补全**:弹窗正文与面板封面共享完整 markdown 样式(h1-h4 分
  级+底线、列表/引用/表格/代码块/分隔线/段落边距),不再「堆在一起」;
  面板档 11px 弹窗档 13px。
- 测试:previewer 124/124(tmap/txt 启发式新钉面)、ui 785/785。

## r18(2026-11-24,做减法)

- **Win** `Synshu-1.5.0-r18-x64-setup.exe`(9.48 MiB)
  sha256 `17a944fd7447bb0740e38e6f82953d61cbebb15bcf9f0f318d49e39d2b2df14b`
  (验证:侧车逐字节/双串/1.5.0×37/nonce 串)
- **用户裁定:撤目录与文本 minimap**——文本/md 打开正常为要。三件套仅
  留 Ctrl+F 文档内搜索(零界面 footprint)。md 渲染保留(markdown-it+
  消毒,块面样式 r17 版);**现场若仍渲染异常 → 与 office 文件预览问题
  合并归 office 流式/渲染票处理**。
- 测试:previewer 121/121、ui 782/782(撤三例)。

## r19(2026-11-24,平铺视图两裁定)

- **Win** `Synshu-1.5.0-r19-x64-setup.exe`(9.47 MiB)
  sha256 `3c36fed1692a8071d3f02873d4491961e0005546c7b5b90e29dd01eb1702b449`
  (验证:侧车逐字节/双串/1.5.0×37/nonce 串)
- **①图标按档真实像素渲染**:类型图标(文件夹/文件页形)此前 16px 属性
  被 CSS 放大数倍(66-104px)——笔画亚像素走样=「平铺放大后模糊」;
  EntryIcon/EntryThumb 增 size 通道,平铺按档渲染(54/46·78/66·104/88,
  与 CSS 三档变量同值),面板大图标 76px 同治。
- **②撤方块底**:tile-slot 常态去底色/描边(选中高亮 .tile.selected 与
  悬停面保留);缩略图裁切域留,可视边界 82%→94%。
- 测试:ui 782/782。
