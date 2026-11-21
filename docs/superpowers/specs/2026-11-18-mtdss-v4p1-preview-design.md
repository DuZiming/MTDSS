# MTDSS(Synshu)V4-1 免下载预览+缩略图+平铺视图 波内设计

- 日期:2026-11-18
- 状态:待用户批准(brainstorm+mockup 定稿,builder check 待跑)
- 上游:伞形 `2026-11-18-mtdss-v4-design.md` §3(2026-11-18 用户批准)
- mockup 存档:`.superpowers/brainstorm/2034-1795001160/content/`(preview-window-form / tile-view-v4-icons / tile-view-v5-size / mobile-tiles-v2 / mobile-preview / desktop-preview-panel)
- 版本:1.5.0;范围=Win/Android/iOS 三端(Linux/macOS 挂载归 V4-3)

## 1. 用户面定稿(mockup 已过用户目)

**桌面预览形态**:右侧滑入面板(默认宽≈38-42%,左缘可拖调并记住);面板工具栏=‹› 切换/文件名+大小/下载/用其他应用打开/**⧉ 弹出为独立窗口**/关闭;点列表其他行=即时换预览;Esc=关面板;弹出后为 Tauri 独立窗可多开,与面板互不排斥。

**桌面平铺**:Explorer 式磁贴(现行 folder/filetint 图标体系原样放大:文件夹 78px/文件页形 66px 档);缩略图槽 aspect-1、**等比缩放居中不裁切**;视频=抽帧+时长角标;文档=类型淡染页形+角标;名字+第二行小字(文件夹「日期 · N 项」/文件「类型 · 大小」);悬停浮出选择圈+快捷下载;滚轮/Ctrl± 三档大小;记住每目录形态。

**移动端**:列表为默认(行升级 44/40px 缩略图方图,视频带时长角标),「···/⋮」菜单可切平铺(一行 4 个,上图标下文字,磁贴≈76-82pt 严格内收,名字一行截断),记住选择;预览=全屏推入(图片查看器=沉浸+左右滑+双指缩放;视频/音频=AVPlayer/ExoPlayer 标准控制器;PDF/office=先渲前 3 页+「查看更多」续取;txt/md/代码同骨架)。

**通用语义**:远端文件双击/单击行=预览;预览纯流式/缓存渲染,**不进传输列表、不落收件箱**;源端离线=打开即明确提示;本机文件预览走同一面板(读本地盘,不经远端)。

## 2. 引擎原语:回环 HTTP Range 口

daemon 内起**仅回环**HTTP 服务(127.0.0.1,随机口,启动时生成 token,仅本机进程可达):
- 路由:`GET /f/{handle}/raw`(Range: bytes=必选支持)+`GET /f/{handle}/thumb?w=256`;`handle` 为短时句柄(由 IPC 申请:`(device_id, vpath/path, mtime, size)` 换 handle,mtime/size 变更即失效重申请)
- 鉴权:`Authorization: Bearer <token>`;token 仅经 IPC 交给本机 UI/WebView
- 后端:远端=按需读(§3);本机=直接 std::fs/桥读
- 预读窗口 2 MiB + daemon 内 LRU 缓存(上限 64 MiB);单 handle 并发 range 请求上限 4(视频 seek 与 pdf 翻页互不打架)
- UI webview 的 CSP 增补 `connect-src http://127.0.0.1:*(token 走头)`;安卓 WebView/iOS WKWebView 同理白名单回环口

## 3. 远端按需读 op

proto 新增只读流 op(消息包裹、字段只增):`file_read_range { device, path, mtime, size, offset, len } → 流式返回`(走 ctrl 面复用连接,与浏览/传输并存;背压按窗口)。挂断/切换设备的既有语义照旧;源离线=即回错误(面板转「源设备离线」态)。engine 内与 download 共用传输栈但**独立限流**(预览不让大文件传输饿死,反之亦然:预览优先级低于交互浏览,高于批量传输)。

**ctrl 面现况=请求-单帧响应**(`crates/core/src/transport/ctrl.rs`),流式返回需新帧序协议——实现细节归 plan,但属本波必做面。

### 3.1 手机命名空间(照片图库/SAF)的读路径分型(builder check 定夺)

现状:对端拉手机内容走 photo_export/serve_export(整文件导出到临时件再读,EXPORT_TIMEOUT=60s)——**无随机访问原语**。分型裁定:

- **图片/PDF/office/txt(小文件,本就整取或量小)**:沿用「导出再读」——handle 绑定导出件(复用,不重复导出),超时预算改为按大小计算(小件秒级);导出件生命周期挂 handle
- **视频/大文件(需要随机 seek)**:**新桥原语=可 seek 的 fd 读**——Android=MediaStore ContentResolver openFileDescriptor 走既有 fdpass 通道(0x01 可 seek 直读已存在,扩到响应方);iOS=AVURLAsset/PHAssetResource 文件 URL 直读(未卸载到本机的 iCloud 优化项=明确提示「原件在 iCloud,需先下载」,不静默全量拉)
- 回退:桥原语不可用(旧对端/SAF 流不可 seek)=退「导出再读」,视频预览首字节慢属已知降级

## 4. 缩略图基建(源端生成,五端)

- 请求方发 `thumb { path, mtime, size, w }` → 源端生成+缓存后回 JPEG(WebP 备选)字节;缓存 key=`hash(path|mtime|size|w)`,源端缓存目录 LRU 上限 512 MiB,失效=key 天然变
- Windows 桌面源:**IShellItemImageFactory**(COM,系统原生出图,覆盖图片/视频抽帧/PDF 首页/HEIC,零第三方解码依赖;像素 256);失败回退=类型图标
- Android 源:MediaMetadataRetriever(视频)+BitmapFactory(图片,含 HEIC 系统解码)经既有 fd-pass 桥喂回引擎
- iOS 源:AVAssetImageGenerator(视频)+UIImage 缩略(图片含 HEIC)经照片桥/文件桥喂回
- Linux/macOS 源:V4-3 波内补(gstreamer/AVFoundation);期间回退类型图标
- 移动端照片库(SAF/PHAsset)沿用各自照片桥先例取帧
- 剪切板「文件封面」(V4-2)复用同一 op

## 5. 共享 web 预览器(一份 JS bundle,五端同源)

目录建议 `ui/previewer/`(独立 npm 包,零 React 依赖,可被桌面 import、安卓 WebView assets、iOS WKWebView resource 三处消费):

| 格式 | 库(锁定版本) | 策略 |
|---|---|---|
| PDF | pdf.js(Apache-2.0) | 自定义 range transport 接回环口;先渲首页/前 3 页,滚动或「查看更多」续取 |
| docx | docx-preview(MIT) | 后台整取(≤64 MB)后逐页渲,先出前 3 页 |
| xlsx | SheetJS CE(Apache-2.0)+ 轻量表格渲染 | 整取后按 sheet 渲首屏行(500 行),续滚续渲 |
| pptx | pptx-preview 系(MIT;实现期择优,渲染失败=类型页+下载引导) | 整取后逐页渲 |
| md | markdown-it(MIT)+DOMPurify | 头部 64 KB 渐进 |
| 代码/txt | highlight.js(BSD) | 头部 64 KB 渐进,大文件续读 |
| 图片 | 自研查看器(zoom/pan) | 缩略图→原文件渐进 |
| 视频/音频 | 视频桌面=hls.js(MIT,吃 §5.1 转码 HLS);音频=HTML5 `<audio>`;移动=AVPlayer/ExoPlayer 原流直传(不吃 bundle) | 段式流 / range |

### 5.1 HEIC 图片与视频统一转码流(2026-11-18 用户裁定:视频要集成,不要降级)

- **HEIC 图片**:静态查看即可——请求端 daemon 内嵌 libheif(LGPL 动态链)解码转 JPEG 经回环口 serve,确定性解决;渐进中途失败=停在缩略图态+重试钮
- **视频=源端小型流媒体服务,一律转码**(2026-11-18 用户钦定②改定:「无论什么格式,都在源端转码成流媒体」):
  - **流媒体格式裁定=HLS(CMAF/fMP4 分段,~4s/段,H.264/AAC,1080p≈4-6 Mbps 封顶)**。三候选比较:fMP4 渐进单流(拖动超前沿必重启转码,MSE 喂流逻辑全自写)/DASH(机制更重无增益)/**HLS 段式=每段独立起头,拖动=取段,段内缓存天然增量与淘汰,桌面 hls.js(MIT)成件成熟,段级并行可跳转——最快最稳,选定**;点播场景无直播低延迟诉求,4s 段粒度无感
  - **统一触发**:PC 端观看任何视频→一律走转码 HLS(不做编解码嗅探分支,一条路径;4K 高码率源顺带被压到预览码率=省带宽);**手机端观看走原流 range 直传**(原生播放器全格式+可 seek,转码纯属浪费——「都转码」落在需要它的 PC 方向);桌面源的 HEVC→手机同样豁免
  - **数据面**:源端转码器逐段产出→经现有传输 op 段级推送/按需拉;PC daemon 落**本地段缓存**并合成指向回环口的 HLS 播放列表;拖动=缓存段即响应,未转段=向源端请求优先/跳转该段(就近关键帧,有界时延);关预览=会话随 handle 释放(手机端编码器即停)
  - 音频文件(m4a/mp3 等)不转码,原流直传(全端原生可解)

- 「整取」=静默后台拉全文件到 **handle 私有落盘缓存**(`.mtdss` 下 preview-tmp,UI 无下载动作、不进任务列表)
- **预览缓存不设单文件上限**(2026-11-18 用户裁定③修订:「目标端,算了不限制,反正最大顶多也就只能是原文件的大小」)——整取/转码段缓存天然封顶≈原文件量级;唯一硬顶=**全局磁盘 LRU 4 GiB**(超限淘汰最旧预览缓存,不影响在预览的 handle)
- **内存 LRU 64 MiB 只管流式预读**(文本/小文件 range),与落盘缓存分账,互不挤占
- 分段参数:PDF 首取 3 页字节;预读 2 MiB;office 首渲 3 页;文本 64 KB;数值收口为常量便于调
- bundle 与回环口通信:URL 参数传 handle+token,渲染器只认 URL,不含平台代码

## 6. 三端接线

**Windows(主战场)**:
- 平铺:FileTiles 重做(磁贴槽+缩略图 `<img>` 懒加载=IntersectionObserver+回环口;三档尺寸=CSS 变量;第二行小字)
- 预览面板:App 内右侧面板组件(宽持久 kv)+`⧉`弹出=Tauri WebWindow 加载同一 previewer 页(多开允许);面板常开时列表点行即换
- 视图形态记忆扩到平铺(现有 查看 切换记忆面扩一档)

**Android**:
- LazyVerticalGrid(4 列)+Coil 缩略图;列表默认,菜单切换持久(SharedPreferences)
- 预览:全屏 Activity;文档=WebView(bundle assets+回环口);图片=自研 Compose 查看器;视频=Media3 ExoPlayer
- 行缩略图/平铺缩略图统一走回环口 thumb

**iOS**:
- LazyVGrid(4 列)+缩略图;列表默认,「···」切换持久(UserDefaults)
- 预览:全屏 push(SwiftUI);文档=WKWebView(bundle resource+回环口);图片=自研 zoom 视图;视频=AVPlayer
- 收件箱/授权根 SAF 对应面照旧(预览面不动接收语义)

## 7. 边界与不支持

- 不支持预览(exe/无扩展名/老 .doc/.xls/.ppt/渲染失败)=类型图标页+「下载/用其他应用打开」
- 源离线=面板/全屏明确提示,不排队不等待
- 取消:关面板/退出全屏=即断流(handle 引用计数归零即停读)
- 预览不产生任务记录、不写收件箱;office 整取缓存随 handle 释放丢弃(daemon LRU 自然淘汰)

## 8. 验收线(=伞形 §3.4 + 波内细化)

1. PC↔MIX3↔iPhone 三端互看:远端图片/视频/PDF/docx/xlsx/pptx/md/代码/txt 免下载内嵌预览;**视频在 PC=任意格式一律转码 HLS 播放+拖动进度条**(直连与中继两路各验一次;含段级 seek 跳转);手机看视频=原流直传拖动;HEIC 图片在 PC=libheif 转码后正常渐进
2. PDF 翻页续载(第 4 页起触发续取);office 先出前 3 页+「查看更多」
3. 桌面平铺:真实图标体系磁贴+照片真缩略图(含手机源 HEIC/视频抽帧)+三档大小+每目录记忆
4. 移动端:列表默认+平铺可切(4 列)+行缩略图;图片查看器手势;全屏预览转场
5. 缩略图源端缓存生效(二次浏览零重生成);源文件改动后缩略图刷新(key 失效)
6. 预览不进任务列表/不落收件箱;源离线提示正确;关面板即断流(日志验证)
7. 本机文件同面板可预览
8. golden/CI 既有面全绿;proto 增量纪律(只增不改号)

## 9. 非目标(本波)

- Linux/macOS 端 UI 挂载(V4-3);剪切板消费(V4-2)
- office 编辑;视频转码/缩略图云端化;预览历史/收藏
- 平铺视图拖拽多选改造(沿用现有多选手势,拖拽进出平铺的行为=列表同构,若现网平铺拖拽已有缺陷不在本波修)

## 10. 留给实施计划(plan)的面

任务切分与顺序、proto 字段编号、handle 生命周期实现细节、回环口端口冲突处理、ctrl 流式帧序协议(file_read_range 新帧面)、Tauri WebWindow 工程细节、Android WebView 与 Coil 的 CSP/混绘配置、iOS WKWebView resource 打包、各库版本锁定值、golden 新增面、CI 构建档(三端)、桌面 viewMode 持久化新面(现况纯 useState)、桥条目 mtime 缺失的 handle 退化语义、冒烟清单(smoke-v4p1.md)。

## 11. 本波裁量清单(builder check 三问+用户三裁定,交审时过目)

1. HEIC 桌面解码=请求端 daemon 内嵌 libheif(确定性,无商店扩展依赖);
2. **视频一律源端转码流媒体(用户钦定 2026-11-18 二次修订:不分格式)**:格式裁定 **HLS/CMAF fMP4 分段**(三候选比较后取段式,拖动=取段最快最稳;hls.js 桌面消费);PC 看视频统一转码、手机看视频原流直传(原生全解,转码无谓);音频不转码;
3. 手机源读路径分型:小文件=导出再读(handle 绑件+按大小超时);视频=新可 seek fd 桥原语(Android fdpass 扩展/iOS 资产 URL 直读;iCloud 未卸载项明确提示)——转码器读源走同一原语;
4. **预览缓存不设单文件上限(用户钦定)**:天然封顶≈原文件量级;唯一硬顶=全局磁盘 LRU 4 GiB;内存 LRU 64 MiB 只管流式预读;
5. 转码码率封顶 1080p≈4-6 Mbps、段长 ~4s(记录人裁量:手机发热与带宽平衡点,实现期可调)。
