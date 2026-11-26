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

## r20(2026-11-24,撤方块底实落)

- **Win** `Synshu-1.5.0-r20-x64-setup.exe`(9.48 MiB)
  sha256 `f20cf8f257b2912c06100dcf9c07f8afa7884eae8258ed2434b4f60b5725886f`
- r19 的撤底改**未实落**(脚本替换未命中误报 ok,包内 tile-slot 底色/
  描边原样——用户截图实锤);本轮 Edit 实改+三级验证(src grep 零
  background → dist css 无串 → 包),缩略图边界 94% 同落。

## r21(2026-11-24,四裁定)

- **Win** `Synshu-1.5.0-r21-x64-setup.exe`(9.48 MiB)
  sha256 `4eb6836e50e8bcb3aa12bc067bc28cbe23013c3e07906e8cd33772df17890480`
- **①撤平铺左上选择圈**(tile-check;Ctrl+点行/框选已足,悬停快捷下载钮
  保留)。顺带修 FileTiles 行点击回传裸 path 潜在错键(fav 平铺位应传
  行键——被新测试揪出)。
- **②框选补 fav 根分组位**(dir 位既有):FavView 宿主 onMouseDown 上抛,
  startBand 泛化矩形源;fav 行复合键与选择集同键形。
- **③右键刷新确认在位**(空白菜单已有「刷新」→reloadTick 硬重拉;此轮
  F5 同源)。
- **④浏览器快捷键拦截**:F5=**刷新当前列表**(capture 相位 preventDefault,
  整页 reload 重置界面被拦——「按 F5 跳回设备页」根因);F7(光标浏览
  弹窗)/F12(devtools)拦下;**devtools cargo feature 撤除**(WebView2 层
  F12 恒可开,JS 拦不住;撤后 AreDevToolsEnabled=false 静默)。
- 测试:ui 781/781(选择圈退役+行键语义翻新)。

## r21b(2026-11-24,Linux/macOS tar 刷新到 r21 码基)

- **Linux x86_64** `mtdss-1.5.0-r21-x86_64-linux.tar.gz`(9.7 MiB,WSL
  Ubuntu-22.04 原生构建 v4p1@a635e81)
  sha256 `5dcc4ebb…`(侧车)
- **macOS aarch64** `mtdss-1.5.0-r21-aarch64-macos.tar.gz`(8.2 MiB,Mac
  mini 原生构建同码基;mac 裸仓同步 a635e81,全新克隆 ~/mtdss-build-v4p1
  构建后已清,~/mtdss-ios 既有树未动)
- 烤入口径沿 tar 既有裁定:仅 MTDSS_SIGNALING_URL=https://www.mtdss.top
  (双二进制 grep 各 1 命中),更新通道不注入(fail-closed 属预期);r2-r21
  daemon 侧全部修复在包(锁重构/中继预览缝/CORS 三面/atime·ctime 链/
  预览 15s 预算/404·410 面/视频直连兜底配合端)。heic 面 Windows 专属
  (FFI DLL),tar 两平台不含=HEIC 预览 fail-closed。
- iOS/mac 既有工作克隆 ~/mtdss-ios 仍在旧码基——iOS 构建前需 reset 到
  v4p1(既有既定流程,届时执行)。

## r22(2026-11-24,Linux/macOS 缩略图补课+图片初始倍率复裁定)

- **Win** `Synshu-1.5.0-r22-x64-setup.exe`(9.5 MiB,ee3c40b3…)
- **Linux** `mtdss-1.5.0-r22-x86_64-linux.tar.gz`(10.2 MiB,5b303082…)
- **macOS** `mtdss-1.5.0-r22-aarch64-macos.tar.gz`(8.6 MiB,432bda75…)
  (tar 双二进制信令串各 1 命中;win 包双串/1.5.0×37/侧车逐字节)
- **①非 Windows 桌面缩略图补课**:根因=thumb 生成分发 Windows=Shell
  COM、非 Windows=空字节(V4-3 才补的设计空档)——Linux/macOS 对端被
  浏览时 /thumb 恒空→右侧栏封面/平铺缩略图全回退图标。修=image 纯
  Rust 后端(解码+等比缩放+JPEG 编码,jpeg/png/gif/bmp/webp;巨图
  96MiB 输入护栏;仅非 Windows 非 mobile 依赖图,Windows 零新增)。
  视频/PDF 抽帧 unix 仍回退图标(无 Shell COM 等价物,V4-3 票)。
- **②图片初始倍率=min(100%, 适配)**(r13「真原大」复裁定):小图
  100% 原大,大图整图入窗不溢出;100% 原大=双击/恢复钮。测试翻新
  (fit 起步+双击往返)。
- 测试:unix thumb 11/11(PNG→JPEG 真解码钉面;独立 home 防账本串
  味)、win thumb 14/14、previewer 121/121。

## r23(2026-11-24,unix 视频缩略图 ffmpeg 抽帧)

- **Linux** `mtdss-1.5.0-r23-x86_64-linux.tar.gz`(10.2 MiB,1e4946ce…)
- **macOS** `mtdss-1.5.0-r23-aarch64-macos.tar.gz`(8.6 MiB,106e66e9…)
  (信令串各 1/更新面 0=既有口径;ffmpeg 串入包验证)
- **unix 视频封面**:ffmpeg 抽帧后端——1s 海报帧(短片 0s 兜底)→等比
  缩到 w→mjpeg pipe 直读(零临时文件);12s 看门狗防病态容器;缺席
  ffmpeg = 回退类型图标(**现场装 ffmpeg 即生效,无需重装 daemon**;
  mac mini 已 brew install)。unix thumb 测试 12/12(lavfi 造片真抽帧)。
- **修 r22 cfg 漏洞**:not(windows) 误纳 android/ios(image 依赖不在
  mobile 依赖图,安卓编译必红)→三臂分发:win=Shell/桌面 unix=image
  +ffmpeg/mobile=空。

## r23b(2026-11-24,iOS TestFlight 20261124.1)

- **iOS** TestFlight **20261124.1** 上传成功(Delivery UUID
  `c7cca6cf-8746-497d-ab33-ad860b305e00`,22.3MB/4.4MB/s;SDK 26.5 警告
  =2027-04 起 App Store 需 27 SDK,仅备案)
- 码基 v4p1@958aa1e(r23);ipa 内嵌 previewer.js/css 为当前版(恢复钮
  串/封面样式标记验证;Windows 专属 nonce 串正确不在 iOS 包)
- **签名链三修(mac 通道手册级)**:①codesign SSH 会话
  errSecInternalComponent=GUI 钥匙串域拒绝——**`launchctl asuser 501`
  到 GUI 会话上下文执行即通**(console 登录态的钥匙串在 GUI 域解锁);
  ②`set-key-partition-list` 须 `-k <密码>` 旗标(`-p` 是用法错误静默
  不生效);③env 变量名 `MTDSS_DEVELOPMENT_TEAM`(拼错 → team 落空 →
  altool @keychain 探针在 GUI 上下文等授权对话框=永挂)。

## r24(2026-11-24,iOS 六裁定 + Windows 视图全局)

- **Win** `Synshu-1.5.0-r24-x64-setup.exe`(9.5 MiB,52884926…;侧车逐字节
  /双串/1.5.0×37)
- **①左滑返回**:非图片容器横向滑(70pt 阈)dismiss;图片容器横向手势
  归图库滑切(不冲突)。
- **②视频格式面**:iOS 原生 AVPlayer 直连 raw——H.264 MP4/HEVC(MP4 容器)
  应可播;**MKV 容器 AVFoundation 不支持**(桌面 MF 转码票落地前 PC 源
  MKV 恒不可播,信息面板「格式」行可甄别容器)。
- **③文档容器真根因**:previewer.css 未入资产臂(404 落代理臂)=全 UI
  未样式化(桌面 r14 同族);补资产臂 + preview.html 模块加载改动态
  import catch(此前静态 import 失败恒挂「加载中…」无诊断,现在错误词面
  直出)。双端宿主页(android/iOS)同源更新。
- **④图片容器减法**:撤底部操作条(下载/转发/全屏/信息收编「···」菜
  单,含全屏切换项)与 ‹› 箭头;**横向滑切**(scale=1 时左滑下一张右滑
  上一张,放大态拖拽不冲突);页点保留。
- **⑤信息面板**:alert → sheet(名称/大小/格式(容器词面+扩展名)/
  分辨率(图片 onMeta 回填)/路径/设备 + 修改·创建·访问时间,yyyy-MM-dd
  HH:mm,0/缺=「—」);DirEntry 补 atime/ctime 解码通道(旧对端缺省安全)。
- **⑥Windows 视图全局**:viewmode:global 单键(弃每目录 {device}:{path}
  键;旧键成孤儿不再读);测试翻新,ui 779/779。
- **iOS TestFlight 20261124.2 上传成功**(UUID d5e1f315,22.3MB/7MB/s;
  SDK 警告同前批)。

## r25(2026-11-24,批量:移动端缩略图根因+交互四项)

- **Win** `Synshu-1.5.0-r25-x64-setup.exe`(9.5 MiB,63e811c4…;侧车
  逐字节/双串/1.5.0×37——首轮 env 丢失重建,r2 纪律复执)
- **iOS** TestFlight **20261124.3**(UUID 3c19d65d)
- **缩略图根因修(模拟器实锤)**:r23 三臂分发把 mobile 排除 → 移动端
  /thumb 恒空字节(200+0B)= PC 浏览手机/手机自览 inbox 缩略图全无;
  image 后端放开到全非 Windows(照片 vpath 走 0x0B 桥不受影响)。
  sim 验证 PNG thumb 0→2421B;unix thumb 12/12、win thumb 14/14。
- **移动端交互**:①前台恒亮屏(isIdleTimerDisabled 随 scenePhase)
  ②图片点按=纯图模式(标题/页点藏,再点回;··· 菜单全屏项在)
  ③左缘右滑返回(28pt 边缘带,任意容器;与中央滑切分流)vs 中央
  横滑=图库切换 ④滑切闪屏根因=换图 preview_open 往返+全链重载 →
  邻图句柄预取仓(开图即预取前后邻,切图命中=瞬切)。
- **冒烟工具**:ios/smoke/smoke-preview.sh 入库(preview_open→回环
  raw/thumb 断言;mac nc 需持连等应答,sim localhost=宿主 loopback)。

## r26(2026-11-25,缩略图启动预热——用户架构提案落地)

- **Win** `Synshu-1.5.0-r26-x64-setup.exe`(9.5 MiB,f8c92377…)
- **Linux** `mtdss-1.5.0-r26-x86_64-linux.tar.gz`(10.2 MiB;预热日志词面
  3 命中=入包;信令 1/更新 0 口径同前)
- **iOS** TestFlight **20261124.4**(上传 No errors)
- **架构问答(现状)**:触发=查看端按需请求→源端生成;落地=源端
  thumb-tmp 512MiB 磁盘 LRU(键 path|mtime|size|宽);复用=同键读盘,
  跨终端生效。缺的正是预热——本修补上:
  **daemon 就绪即后台低优先级预扫**(移动=全部 fs 授权根+照片图库
  桥递归;桌面=收件箱),逐项走与在线请求完全相同的路由化生成
  (thumb_routed 抽单点:服务桥/照片桥/平台后端)落地缓存;
  串行+80ms 呼吸(源端无感)+10 分钟预算断点续扫(缓存只增,下次
  启动接着扫);键宽统一 256(面板/平铺 m 档/移动浏览行最常见面);
  MTDSS_THUMB_PREWARM=0 关停。
- 测试:unix preview 107/107、win 102/102(prewarm 全链:两 PNG 落
  缓存、txt 不落、候选扩展名矩阵)。

## r26b(2026-11-25,预热 24h 定时增量制)

- **Win** `Synshu-1.5.0-r26b-x64-setup.exe`(df76ca69…)
- **Linux** `mtdss-1.5.0-r26b-x86_64-linux.tar.gz`(bc74cffd…;预热词面
  3 命中)
- **iOS** TestFlight **20261125.1**
- **节奏制(用户裁定)**:批 8 项/轮 → 轮睡 30s → 再批;项间 80ms
  呼吸;**串行恒定,零并发**(≈960 项/时 ≈ 2.3 万项/日);全量扫完转
  **10 分钟增量巡检**(重收集候选,新文件/mtime 变更键自然 miss 重
  生成);**24h 窗自 daemon 启动计时**,到点收工,下次启动重起窗;
  daemon 进程退出 = runtime 收摊 = 任务自然终结。
- 测试改单趟直驱(不进常驻主环):unix 107/107 + win 102/102。

## r26c(2026-11-25,按需触发面生成并发帽)

- **Win** `Synshu-1.5.0-r26c-x64-setup.exe`(8045bb48…)
- **Linux** `mtdss-1.5.0-r26c-x86_64-linux.tar.gz`(41f9ebd9…)
- **iOS** TestFlight **20261125.2**
- **现状问答(按需触发)**:查看端每个可见缩略图独立发请求 → 源端
  **恒并行、零间隔**——N 个平铺缩略图同时缓存 miss = N 路并发解码,
  手机端全尺寸 HEIC 解码直接卡(上次相册卡顿根因)。**修=三生成点
  (本机 keyed/App 桥解码/照片导出回退)共用 Semaphore(2) 仅钳生成
  段**:最多 2 路并发解码,其余排队等令牌(信号量 FIFO);**缓存命中
  读盘不设帽不排队**(热路径零损——预热完成后基本全走此面);预热与
  在线同帽(全局重活上限一致)。env MTDSS_THUMB_GEN_CONCURRENCY 可
  调 1-8。win thumb 14/14 + unix preview 107/107。

## r26d(2026-11-25,相册卡顿双根修+截断帽)

- **iOS** TestFlight **20261125.3**(Kit Swift 430/430)
- **①相册卡顿真双根**(并发帽外另两层):a)**每请求全相册重放**——
  缩略图名→photoId 解析每次 fetch+enumerate 整相册(O(n²),大相册
  即卡顿波)→ assets 15s TTL 缓存(Seek/PhotoServe 全调用方受益);
  b)**全尺寸解码**——照片臂 openSeekable(requestContentEditingInput
  渲染)+ UIImage 全图解码再降采样 → **PHImageManager 系统缩略图快径**
  (Photos 自用缓存面直出 JPEG,aspectFit;协议 thumbData 缺省 nil =
  全量路不变,快径不命中才落 fd+编码器)。
- **②列表截断帽**:照片列表 200 → **10_000**(对齐 fs LIST_ENTRY_CAP;
  测试夹具按帽+1 生成)。相册 <1 万项不再「仅显示部分」。

## r26e(2026-11-25,四裁定:相册断连根因+撤预热+混合页平铺+槽底撤)

- **Win** `Synshu-1.5.0-r26e-x64-setup.exe`(3ac5fe05…)
- **Linux** `mtdss-1.5.0-r26e-x86_64-linux.tar.gz`(e0e02ede…;prewarm
  词面 0=已撤验讫)
- **iOS** TestFlight **20261125.4**
- **①相册「路径不存在:App照片桥拒绝 oversized(160864)」根因**:列表帽
  提到 10_000 后照片列表 JSON 达 160KB,超 fdpass 帧 64KiB 帽被整帧拒
  → 列表整体 NOT_FOUND。修=帧帽 **64KiB→8MiB 双侧**(Rust MAX_PAYLOAD_
  BYTES + Swift maxPayloadBytes 同步;本地 UDS 面安全)。
- **②撤预热与生成并发帽(用户裁定:即时并发)**:prewarm 模块整撤
  (spawn+模块+测试);Semaphore(2) 生成帽撤(三处);**保留** PHImage-
  Manager 系统缩略图快径 + assets 15s TTL 缓存 + 平铺/列表懒加载(可视
  即取,滚动到哪请求到哪——浏览页与混合页平铺均 LazyVGrid/LazyVStack
  天然懒)。
- **③快捷混合页列表/平铺切换**:BrowseTileItem 泛化(entry+闭包,选择/
  翻选/进目录/开预览全注入);混合页按成员设备分池缩略图(PreviewHandle-
  Pool open 缝携 device);「···」菜单切换项,仓持久与浏览页共享。
- **④iOS 平铺磁贴槽底色撤**(systemGray6 底删,内容面即裸)。

## r26f(2026-11-25,缩略图链路三修——吞吐+卡顿+I/O)

- **Win** `Synshu-1.5.0-r26f-x64-setup.exe`(38645e4f…;jpeg_b64 串 9 命中)
- **Linux** `mtdss-1.5.0-r26f-x86_64-linux.tar.gz`(4b468efa…)
- **iOS** TestFlight **20261125.5**
- **三修**(用户裁定;分析→实施全链):
  - **①吞吐**:Responder 并发化(ConcurrentGate actor 钳 3;per-event
    MainActor Task 在 PHImageManager await 点交错=真并发;此前串行 for
    await 是最大瓶颈)
  - **②卡顿**:名→photoId O(1) 字典查替 O(n) 重放(PhotoNameIndex
    15s TTL;0x06 清单同序 uniqueName 单点——此前每请求全相册字符串
    重算压主线程=「手机卡但其他 app 不卡」根因)
  - **③I/O**:0x0B 内联 JPEG(Swift deliver base64 直置 JSON + Rust
    parse_thumb_reply 解码;临时文件三跳写→读→删全撤;帧帽 8MiB 恒
    适配;legacy path 兼容回退保留)
- win 101/101 + unix 105/105

## r26g(2026-11-25,缩略图四修——冷缩略图提速 + 手机卡顿根因面)

- **iOS** TestFlight **20261125.6**(Win/Linux 沿 r26f 包——本波纯 Swift,
  引擎面零改动,无需重发)
- **四修**(用户裁定「四个一起做」;分析→实施全链):
  - **①顺序反转**(主菜):照片臂快径**前置**——answerThumb 先纯解析
    resolvePhoto(名→photoId,O(1) 字典 + 15s TTL)→ PHImageManager 系统
    缩略图 → 内联交付,**零 openSeekable / 零 requestContentEditingInput**
    (此前 openThumbSource 无条件先跑 requestContentEditingInput 全尺寸渲
    染,90%+ 场景成本白付——「冷缩略图慢 + 手机卡」最大剩余根因);快径
    不中(罕见)落全量路,其内部二次解析走缓存零重复成本;ThumbSource
    撤 photoId 凭据(快径面已前置)。
  - **②deliveryMode .opportunistic**:嵌入缩略图优先(highQualityFormat
    强全图解码是冷缩略图慢的单项最大成本;同步请求恰一回调 = 降级产物
    即嵌入缩略图,缩略图场景正解)
  - **③PHAsset 对象缓存**:NSCache 600 帽 cachedAsset——thumbData/
    openSeekable/thumbnail 三面共享,每请求 fetchAssets(withLocalIdentifiers:)
    往返免重复(16 万库高频波显著)
  - **④并发闸 3→5**:硬件 HEIC 解码器共享资源上界 ~6,5 = 安全余量
- **收尾三修**(swift test 2 红根因 + 快径同患面):
  - displayName 恢复 raw 原始名:PhotoNameIndex 值改 (id, raw) 二元组(去
    ` (n)` 顺延后缀;Android ThumbSource.displayName = asset.name 同面;
    r26f 编译修轮丢了 stripIndexSuffix 语义)
  - ConcurrentGate 补 drain():run 事件流终结后等在飞 0x0B 应答归还再返
    回(并发化后 run 先于 spawned Task 返回的竞态;关停语义同正)
  - **albums() 15s TTL 缓存**:快径前置后 resolvePhoto 每请求一次
    albums()——全库计数 + 逐相册计数是 Photos IPC 往返,与 assets 同患;
    r26d 同款 TTL 收口
- swift 432/432 绿(新增快径前置两钉:命中 = 零 openSeekable/零编码器;
  不中 = 落全量路;内联 JPEG 断言同步 r26f 修3)

## r27 移动端视频两案(2026-11-25 末三;iOS 转码四根修+视频缩略图路由)

用户报:PC 开 iPhone——①视频文件无缩略图 ②视频弹窗「视频加载失败」
HEVC 引导词。现场取证链:本机 preview-tmp 两次会话(11-24 22:46 +
11-25 17:26)vseg 段**字节级相同**=确定性缺陷;段解剖(ffprobe+盒走)
=mdat 只有音频、视频 trun 样尺寸全 0、moov 缺 mvex/trex。Mac mini 建
**独立复现台**(真 AVAssetReader+VT+AudioConverter 泵,swiftc 单模块
编译,库代码零改动;artifacts/tc-harness/)逐环钉死四根因:

- **①moov 缺 mvex/trex**:fMP4 必需,Chromium MSE/ffmpeg 无 trex 拒段
  (hls.js append fatal)。修=buildMoov 补 mvex{trex×在场轨}(默认值 0,
  trun 恒携逐样面,纯在场义务)。
- **②VT 样形态误判**:macOS 26/iOS 26 的 VT format description
  extensions **缺 avcC 键但样恒 AVCC** 长度前缀形——原码无条件按
  Annex-B 转换,avccSample 找不到起始码**把全帧字节归零**(raw=12870→
  out=0 实证)=视频样全空/mdat 只剩音频的根因。修=sampleLooksAnnexB
  字节面嗅探(起始码判,Kit 纯函数)。
- **③avcC numSPS 位 0xE0→0xE1**:0xE0 的 numSPS 位=0 出「零参数集」
  avcC——解析器拒建流(non-existing PPS 0 / find_stream_info 失败)。
  ffmpeg 对照件钉死(good ff e1 / bad ff e0)。
- **④ASC 净化**:macOS 26 magic cookie 返回**整棵 esds 描述子树**(扩展
  长度 TLV,与好件 esds 逐字节同头),直装 DSI=Audio object type 0 解
  码失败。修=sanitizeAudioAsc(容器头 03→3B/04→13B 跳过后走树取 tag
  05 载荷;裸 ASC 透传)。泵三源(直通种子/转编 cookie/样本回补)全接。

**验证**:复现台 H.264+AAC(微信形态)与 HEVC+AAC(iPhone 拍摄形态,
720×1280→608×1080)双源——ffprobe 识流 + **ffmpeg 全量解码零错**;
段字节账(视频样尺寸真值/mdat 双流/118+170 样)全对。
**Android SegmentMuxer.kt 同款两根修镜像**(③④之 ③+mvex;Kotlin
MediaCodec 面无 ②④形态)。

**①视频缩略图**:iOS fs 根视频文件——平台后端仅 image 解码(移动端无
ffmpeg)恒空字节。修=thumb_routed 移动端臂(cfg ios/android):视频扩
展名先走 App 缩略图桥文件臂(IOSThumbEncoder AVAssetImageGenerator;
iOS 引擎与 App 同进程同沙箱同路径直开),不中回落平台后端。Android
SAF 桥根本不经此臂(前臂已桥)。

**附带**:thumb_bridge 两测试随 r26f 修3 契约(修 CI ubuntu 两红:
legacy path 臆造路径读败 + 缺 path 词面漂移)。

**r27 部署**:iOS TestFlight 20261125.7(转码四修+缩略图路由;当轮 Win/
Linux 沿 r26f 包——用户实机复验两项全过)。

## r28 视频时长封口(2026-11-25 末四;eof 通知链 + ENDLIST + 编码全速)

用户实机复验 r27 两项全过(缩略图✓/可播✓)但**时长从几秒慢慢涨到
全长**(HLS EVENT 形 + 源端 1x 实时产段 + 无完毕信号,列表永不封口)。
三面修:
- **①0x0C eof 通知链**:App(iOS/Kotlin)转码自然完毕发 `{eof,ok,
  request_id}`(取消/失败静默;段文件不随通知清理);引擎 RenditionBridge
  记 completed(deliver 前置分支);AndroidTranscoder 对完毕会话越界段
  改答**段越界**(segment_range,不烧等待窗)。
- **②查看端封口**:ViewerRendition 拉段遇段越界 = 记段尽 → playlist 封
  `#EXT-X-ENDLIST`——时长锁定全长 + 整条可 seek + 播放端正常结束态。
  **查看端面 = PC 也要换包**(本波 Win/Linux 重发的因由)。
- **③前拉窗 8→16 + VT RealTime 关**(全速编码,段产远快于播放,打开
  后几秒内时长到全长)。
- 附带:thumb_bridge legacy 臂补回 r26f 误撤的前缀 fail-closed + 读后
  unlink(path_under_thumb_serve_dir 复活;e2e「thumb-serve 必空」钉);
  thumb_bridge_e2e 两枚 r25 前旧断言随新约(回退链非 Windows 真出图)。

**部署**:Win `Synshu-1.5.0-r28-x64-setup.exe`(**41b34d17**…;双 env 烤入验证 1/1);
Linux `mtdss-1.5.0-r28-x86_64-linux.tar.gz`(0e47aa07…;信令烤入 1/
.invalid 0);iOS TestFlight **20261125.8**(UUID 9e0c4765)。测试:swift
434/434 + WSL daemon 全量(fallback armed_initiator 单跑绿=时序 flake)
+ Android responder 绿。

## r29 视频时长 VOD 化(2026-11-25 末五;open 应答携总时长,playlist 全长一次列)

r28 实机复验:PC↔PC 直连正常(容器自带 moov 时长),PC←手机仍慢慢涨——
r28 的 eof 封口只管终态,**EVENT 形时长恒=已列段**(源端 ~2x 实时产段,
前十几秒时长必然在长)。正解=时长先知:
- **①proto**:RenditionOpenResponse +`duration_ms = 4`(只增;0=未知落
  r28 EVENT 生长形,旧端互通零破)。
- **②0x0C 回执携时长**:iOS 探针 `RenditionDurationProbing`(F_GETPATH→
  AVURLAsset.load(.duration),App 注入;0 时键缺省=旧形逐字节);引擎
  RenditionBridge.open 返回 (session,duration),Transcoder trait 增
  duration_ms 缺省面,mgr/ctrl 应答透传。
- **③查看端 VOD 全列**:duration>0 → playlist **全部段一次列出+ENDLIST**
  ——时长自第一帧即全长、整条可 seek;未产段由段路由有界等待/远跳重开
  兜;重开锚自锚列至片尾。
- **④eof 发送闩**:段回告无序 Task 发送 begin/end 记账,eof 前 idle()
  等清零(保「eof 最后」wire 序;引擎本容忍乱序,闩保契约+测试确定)。

**部署(r29 三包,PC 与手机仍须同升)**:Win `Synshu-1.5.0-r29-x64-setup.exe`
(**0887694a**…;双 env 烤入验证 1/1)/Linux `mtdss-1.5.0-r29-x86_64-linux.tar.gz`
(**16d49069**…;信令烤入 1/.invalid 0)/iOS TestFlight **20261125.9**
(UUID 4f1d674d)。测试:swift 435/435(新增探针回执形钉)/Rust 全量
(伪管线 VOD 全列 26s→7 段+ENDLIST;bridge duration 透传;layout_full_chain
重载 flake 单跑绿——已知家族,非本波面;proto roundtrip 随增量——本地
包级检查漏 workspace 面,CI 揪出补)。

## r30 文档族两案(2026-11-25 末六;同源根修 + PDF 缩略图)

用户报手机侧:①文本打开「文档加载失败」②PDF 无缩略图 + 打开「PDF 加载
失败」。
- **①同源根修**:宿主页 `mtdsspreview://localhost` 与 raw 取数
  `mtdsspreview://127.0.0.1:{port}` 的 host:port 双异 = **跨源 fetch**,
  WebKit 按 CORS 拦(daemon 无 CORS 头)——文本/PDF/office 文档族全灭;
  图/视频走原生加载器不经 WKWebView 幸免 = r24 挂账「app 内取数败」
  真身,一直未闭环。修=端口挪进路径 `/p/{port}`(真同源,fetch/Range/
  预检全零);loopbackUrl 反解收紧面随迁(localhost host + /p/{数字}/f/
  余径,旧 authority 形拒)。pdf.js 本就主线程假工(零 worker 资产),
  同源修后渲染链即通。
- **②PDF 缩略图**:BrowseThumbs 门 +pdf;IOSThumbEncoder 增
  QLThumbnailGenerator 臂(QuickLookThumbnailing 首页);daemon 移动端
  缩略图臂门扩 pdf(桥走 App QL;桌面 Windows Shell 本有,移动对齐)。

**部署**:iOS TestFlight **20261125.10**(UUID d73a0538;GUI 域签名链
档——SSH 域 CodeSign 拒,launchctl asuser 501 过;纯 iOS+mobile-cfg 面,
PC 沿 r29 包不换)。测试:swift 435/435(PreviewRoute 新旧形/收紧矩阵
重钉 + Range 转发随新形 + BrowseThumbs pdf 真值)。

## r31 手机视频 HLS 消费(2026-11-26 晨;源端转码全端可播)

用户报手机侧:视频打开「无法播放此格式」(AVPlayer 直连,MKV 等
AVFoundation 不认容器;r24 已知限制)——用户裁定「源端 HLS 编码,客户
端都可以播放」。修:
- **视频双路由**:AV 原生容器(mp4/m4v/mov)= AVPlayer 直连(原生
  controls);其余复用 doc 容器 WKWebView+previewer(**hls.js 消费源端
  HLS,与桌面同面**;本地源经 App 桥转码同通用——手机自身即 rendition
  源端)。
- PreviewBoot.requestJson 增 hlsUrl/thumbUrl(视频 boot 携
  urls{raw,hls,thumb};缺省键缺=旧形);PreviewRoute 增 proxyHlsUrl/
  proxyThumbUrl(同源 /p 形)+ isAvNativeVideo。
- **PreviewProxy mpegurl 体改写**:playlist 内段/MAP 绝对 URI
  `http://127.0.0.1:{port}` → `mtdsspreview://localhost/p/{port}`(纯
  字节扫描;r30 同源纪律闭环到 HLS 面——跨 scheme fetch 恒拒)。

**部署**:iOS TestFlight **20261125.11**(GUI 域签名档;纯 iOS 面,PC 沿 r29)。测试:swift
437/437(hls/thumb URL 形钉 + isAvNativeVideo 矩阵)。
**挂账**:手机侧 PNG/视频缩略图缺位待定位(需用户确认浏览的源设备与
视频格式后定向)。

## r32 手机自览缩略图根修 + 视频恒 HLS(2026-11-26 晨)

用户实机:手机自览 PNG 无缩略图、视频无缩略图且 mp4/mkv 全打不开
(AVPlayer 直连真机系统性失败,格式无关)。**sim 地面真相取证**(三件套
preview_open+raw+thumb 直驱 + 双侧插桩):PNG ✓2421B / **mp4 ✎0B——
`[THUMBDBG]` 全链静默 = thumb_via_bridge 根本没被调**。
- **根因**:回环 /thumb **本机臂直调 thumb()**(缓存+平台后端),不走
  `thumb_routed` 路由单点——桥臂只有 ctrl 面(PC 看手机)在用 = 手机自览
  视频/PDF 缩略图恒空的不对称根源。修=本机臂统一走 routed(与 ctrl 应答
  面同点;桌面等价——无桥根时 routed 退化 thumb())。
- **验证**(sim 干净构建):mp4 自览缩略图 **13130B 全链通**(事件→
  responder→AVAssetImageGenerator→0x0B 内联);mkv=AVFoundation 不认
  Matroska 容器(与视频需 HLS 同因)缩略图留图标降级。
- **r31b 视频恒 HLS**(用户裁定「和 PC 统一都用 HLS」):撤 AVPlayer
  原生分叉,视频一律源端 HLS 消费(doc 容器 hls.js;mp4 同转码)。
  audio 仍 AVPlayer。

**部署**:iOS TestFlight **20261125.12**(UUID d2514247;引擎面改动仅
mobile-cfg+loopback 本机臂——桌面行为等价,PC 沿 r29 不换)。WSL daemon
全量绿(零 FAILED)。

## r35 直读测试轮(2026-11-26 午;用户裁定「不转码了,都直读,测格式支持」)

r30-r34 转码消费全撤(休眠不拆):boot 侧不携 urls.hls(previewer
hls-undefined 分支「不可预览报错」→「原档直连」;桌面 buildPreviewRequest/
iOS PreviewBoot 双侧摘除;HLS 机器完整保留,恢复=引导侧重携 hls 一行)。
目的=纯直读环境测三端×格式支持矩阵,拿数据定转码去留。
**部署**:Win `Synshu-1.5.0-r35-x64-setup.exe`(**6237506e**…;双 env 1/1)/
Linux `mtdss-1.5.0-r35-x86_64-linux.tar.gz`(**f9c0fafb**…)/iOS TF
**20261126.5**(UUID e26c408a)。ui 634/634。

## r36 直读两修(2026-11-26 午后)

r35 实测:PC 全绿(PC↔PC、PC←手机 mp4+mkv 全秒出);手机 mp4 直读
「能开但播放卡死」+ 远端 mkv 干转。修:
- **①Content-Length 透传**(iOS 代理;r33 流式化时误剥——WebKit 媒体
  管线要总长才推缓冲 = mp4 直读卡第一帧根因;缺席面=真流式不强造,
  内存仍块级)。
- **②RAW_META_TIMEOUT_MS=12s 看门狗**(共享 bundle):WebKit 对不可解
  容器可能既不 error 也不出 metadata——窗内无 loadedmetadata 即失败面
  (格式不支持快提示;PC 顺带治 HEVC 无扩展干转)。
**部署**:Win r36(**eaa8482f**…)/Linux r36(**d38950ed**…)/iOS TF
**20261126.6**(UUID e0ee68bc)。ui 634/634。

## r37 代理主线程串行(2026-11-26 午后三;纯 iOS)

r36 实测:本地 mp4「卡死→崩溃」、远端 mp4 看门狗超时——**r33 流式化
线程违规**:URLSession delegate 队列(后台)直调 WKURLSchemeTask
.didReceive 违反 WebKit 铁律(scheme task 调用必须主线程):后台持锁调
WebKit + 主线程 stop() 抢锁 = 死锁冻结;时序错开 = NSException 崩溃。
修 = delegate 回调携载荷主队列跳,start/stop/emit 主线程天然串行,锁
整个撤除(use-after-stop 竞窗同消);Content-Length 透传/playlist 改写/
Range 面不变。**部署:iOS TF 20261126.7**(UUID 823ba268;PC 沿 r36)。

## r37-r39 直读收官三连(2026-11-26 午后;iOS 代理与 blob 面成型)

- **r37 主线程串行**:r33 流式化在 URLSession delegate 队列(后台)直调
  WKURLSchemeTask.didReceive 违反 WebKit 铁律——死锁冻结/NSException
  崩溃双形态。修=回调携载荷主队列跳,锁撤除(r36 实测本地 mp4 直读 ✓)。
- **r38 blob 直读**:远端 mp4 看门狗超时=WebKit 媒体进程经自定义 scheme
  加载慢流挂起(远端 fetch 同代理畅通)。修=iOS boot 携 directBlob:
  fetch(网页进程)→帽检→blob→objectURL(媒体进程零 scheme 面)。
- **r39 三修**:本地归 URL 直连(r37 已证;blob 误伤本地大件=web 进程
  内存死白屏);看门狗起表时机分面(URL=立即;blob=**下载完成后**,
  下载期 300s 长窗——12s 死线曾掐断大件下载);帽预检用 meta.size。
**部署**:iOS TF **20261126.9**(UUID d4ca03e9;PC 沿 r36)。
