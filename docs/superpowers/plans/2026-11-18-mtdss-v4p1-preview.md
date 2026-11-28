# MTDSS V4-1 免下载预览+缩略图+平铺视图 实施计划

> **For agentic workers:** REQUIRED SUB-SKILL: Use superpowers:subagent-driven-development (recommended) or superpowers:executing-plans to implement this plan task-by-task. Steps use checkbox (`- [ ]`) syntax for tracking.

**Goal:** 三端(Win/Android/iOS)对远端与本地文件免下载内嵌预览(视频一律源端转码 HLS),源端缩略图基建,桌面/移动平铺视图。

**Architecture:** daemon 起仅回环的 HTTP Range/token 服务作为唯一预览数据口;远端按需读与缩略图/转码走既有 ctrl/传输面(新增流式帧序);手机源文件访问抽象为可 seek 的 FileSource(桥原语);渲染层=零 React 依赖的共享 previewer bundle 五端同源。

**Tech Stack:** Rust(proto/core/daemon/ipc)、Tauri+React(桌面)、Kotlin/Compose+Media3(安卓)、SwiftUI+AVFoundation(iOS)、pdf.js/hls.js/docx-preview/SheetJS/highlight.js(markdown-it+DOMPurify)、libheif(桌面 HEIC)。

**Spec:** `docs/superpowers/specs/2026-11-18-mtdss-v4p1-preview-design.md`(计划据此论证,执行者两份都读)

## Global Constraints

- proto 纪律:消息包裹/字段只增不改号/缺省值(protoc 3.12 兼容);新消息字段号=各消息下一空闲编号,实现时以 proto 文件现状为准
- golden 双端锁定:contract golden REGEN 后 Kotlin 断言/Swift 测试同步更新
- ui/ 零硬编码颜色(仅 themes.css);图标不自创笔画(icons.tsx 存档纪律)
- 预览不进任务列表、不落收件箱、不产生任务记录;关预览即断流(handle 引用计数归零停止一切源端工作)
- 版本=1.5.0;bump 联动=四处版本文件+Cargo.lock×2+golden+Kotlin 断言×5+android versionCode
- 本机跑 workspace 测试前 unset MTDSS_UPDATE_URL/MTDSS_SIGNALING_URL/http(s)_proxy(既有 env 卫生);CI clippy 可能领先本地(1.100 vs 1.99)——助手类小函数直取单布尔表达式
- Android fdpass 通道新号从 0x0A 起(0x01/0x02/0x04/0x05/0x06/0x07/0x09 已占);iOS 桥号按 ios/ 现状顺延
- 测试命令:Rust=`cargo test -p <crate> <name>`;ui=`npm -C ui test`(vitest)+`npm -C ui run build`;安卓=`./gradlew :app:testDebugUnitTest`(ANDROID_HOME 显式);iOS=Mac mini 按 docs/dev-mac-mini.md(SSH+钥匙串 unlock;swift test)

## Review Focus(测试必须钉住的五类;归属任务内含测试)

1. **源端中途离线**:预览数据口对 UI 返回 503+明确错误态,hls.js/渲染器停住可重试,不挂死——T15/T21 测。
2. **handle 泄漏**(面板关/独立窗关/进程杀):引用计数回收;源端转码会话与导出件超时兜底回收(60s 空闲)——T4/T12 测。
3. **并发互饿**:大文件传输进行中,预览首字节仍秒级(独立限流)——T3 e2e 测。
4. **Range 非法**(off>size/len=0/畸形 header/未带 token):416/400/401,不 panic 不错读——T4 测。
5. **旧对端互通**:对端不识新 op→CtrlError→UI「对端版本过旧,无法预览」降级,浏览/传输不受影响——T2/T12 e2e 测;另:桥条目 mtime 缺失→handle 以 (path,size) 退化键(T4 测)。

---

### Task 1: proto 增量——按需读/缩略图/转码会话

**Files:** Modify `crates/proto/src/*.proto`(ctrl/预览相关文件)、REGEN golden。
**Interfaces(Produces,后续全波消费):**
- `FileReadRangeRequest { device_id, path, mtime(i64,可缺省), size(i64), offset(u64), len(u32) }`;响应=流式 chunk 帧(帧序定义在 T2)
- `ThumbRequest { path, mtime(i64 可缺省), size(i64), width(u32) }` / `ThumbResponse { jpeg(bytes) }`
- `RenditionOpenRequest { path, mtime, size, target(enum HLS=0) }` / `RenditionOpenResponse { session_id(u64), segment_ms(u32), mime(string) }`
- `RenditionSegmentRequest { session_id, index(u32) }`(响应=流式 chunk)/ `RenditionCancel { session_id }`
- IPC(ctrl UDS JSON 面):`preview_open { device_id?, path, mtime?, size? } → { handle(string), port(u16), token(string) }`;`preview_close { handle }`

- [ ] **Step 1** 写 golden 期望测试(新消息在 contract_golden 的 encode/decode 往返样例,含 mtime 缺省形)。
- [ ] **Step 2** `cargo test -p mtdss-proto` 红。
- [ ] **Step 3** 增消息(包裹/只增/字段号按现状下一空闲),REGEN golden。
- [ ] **Step 4** `cargo test -p mtdss-proto` 绿;`cargo test -p mtdss-daemon contract_golden` 同步。
- [ ] **Step 5** Commit `feat(proto): V4-1 预览三组消息(按需读/缩略图/转码会话)+preview_open IPC`。

### Task 2: ctrl 面流式帧序(多帧响应)

**Files:** Modify `crates/core/src/transport/ctrl.rs`、daemon ctrl 接线;Test `crates/core/tests/` + ctrl_e2e。
**Interfaces:** Produces:流式响应协议=`响应首帧(Data-begin{stream_id})+若干 Data-chunk{stream_id,bytes}+Data-end{stream_id}`;未知 oneof 变体→既有 CtrlError(旧端安全)。Consumes: T1 消息。
- [ ] Step 1 失败测试:回环双 daemon 流式 echo e2e(发起方收齐 N chunk 且序不乱)+旧端收到未知变体回 CtrlError 不崩。
- [ ] Step 2 `cargo test -p mtdss-core --lib` + ctrl_e2e 红(协议不存在)。
- [ ] Step 3 实现 stream_id 注册表与背压窗口(按既有 transport 模式);守护:同连接普通请求-响应不受影响。
- [ ] Step 4 测试绿(WSL 实跑面按既有房法)。
- [ ] Step 5 Commit `feat(core): ctrl 流式多帧响应协议(V4-1 预览数据面基座)`。

### Task 3: 引擎远端按需读+独立限流+FileSource 抽象(std::fs 实现)

**Files:** Create `crates/daemon/src/preview/source.rs`(trait)、Modify `crates/daemon/src/jobs.rs`/ctrl 响应接线;Test daemon_e2e。
**Interfaces:** Produces: `trait FileSource { fn size(&self)->u64; fn read_at(&self,off:u64,buf:&mut [u8])->io::Result<usize>; }` + `open_source(device,path,mtime,size)->Arc<dyn FileSource>`(桌面=std::fs;手机桥实现 T8/T10 挂入);`ExportFileSource`=手机不可 seek 时的回退实现(复用既有 serve_export/photo_export 导出件,生命周期挂 handle,超时预算按大小计算,秒级小件);`PreviewLimiter`(独立令牌桶,优先级:浏览>预览>批量传输)。Consumes: T1/T2。
- [ ] Step 1 失败测试:e2e 双 daemon(同机)PC A 对 B 本地文件 read_range 逐段校验字节+大传输进行中预览首字节 <2s(限流断言)。
- [ ] Step 2 红。
- [ ] Step 3 实现(响应方解析请求→open_source→流式回;错误→CtrlError['source_offline'])。
- [ ] Step 4 绿。
- [ ] Step 5 Commit `feat(daemon): file_read_range 按需读+FileSource 抽象+预览独立限流`。

### Task 4: 回环 HTTP 服务+handle 注册表+preview_open IPC

**Files:** Create `crates/daemon/src/preview/loopback.rs`、`preview/handles.rs`;Modify ipc 面;Test 单元+e2e。
**Interfaces:** Produces:
- 路由:`GET /f/{h}/raw`(Range→206/200/416)、`/f/{h}/thumb?w=`、`/f/{h}/jpeg`、`/f/{h}/video/index.m3u8`、`/f/{h}/video/seg/{n}.m4s`(T15 挂);鉴权 `Authorization: Bearer <token>` 缺/错=401;仅绑 127.0.0.1 随机口
- handle=(device_id?,path,mtime?,size?) 短时句柄,引用计数;mtime 缺省→(path,size) 退化键;60s 空闲兜底回收(Review#2/#5);单 handle 并发 range 请求上限 4;bind 失败换口重试
Consumes: T3。
- [ ] Step 1 失败测试:单测(路由矩阵:206/416/401/404;handle 过期→410)+e2e(preview_open→raw 拉 N 段字节与源一致;close 后即 410)。
- [ ] Step 2 红。
- [ ] Step 3 实现(axum/tokio 既有栈;内存预读 LRU 64MiB 在此:同一 raw 连续 range 命中窗口)。
- [ ] Step 4 绿。
- [ ] Step 5 Commit `feat(daemon): 预览回环口+handle 生命周期+preview_open IPC`。

### Task 5: 预览落盘缓存(preview-tmp+全局 4GiB LRU)

**Files:** Create `crates/daemon/src/preview/cache.rs`;Test 单元。
**Interfaces:** Produces: `PreviewDiskCache::store(handle,&mut stream)->io::Result<PathBuf>`、`path_for(handle)`、全局 LRU 4GiB(淘汰最旧、在预览 handle 跳过);目录=`.mtdss/preview-tmp`(收件箱语义外,绝不触公开区)。Consumes: T4 handle。
- [ ] Step 1 失败测试:LRU 淘汰顺序/在预览豁免/容量计费(伪文件)。
- [ ] Step 2 红 → Step 3 实现(目录记账 json 原子写,沿 tasks.json 模式)→ Step 4 绿。
- [ ] Step 5 Commit `feat(daemon): 预览落盘缓存+全局 LRU`。

### Task 6: 桌面 HEIC→JPEG(libheif)

**Files:** Modify `crates/daemon/Cargo.toml`(libheif 动态链 feature)、`preview/loopback.rs`(/jpeg 路由);Test 单元(样例 HEIC 夹具)。
**Interfaces:** Produces: `/f/{h}/jpeg`=HEIC 源解码转 JPEG(结果进 T5 缓存);非 HEIC/解码失败→404/422(客户端回退 raw)。
- [ ] Step 1 失败测试:夹具 HEIC→/jpeg 200 且 image::load 校验尺寸;损坏件→422 不 panic。
- [ ] Step 2 红 → Step 3 实现(仅 Windows cfg 门;libheif LGPL 动态链,README 记分发注记)→ Step 4 绿。
- [ ] Step 5 Commit `feat(daemon): HEIC 预览转码(libheif,仅 Windows)`。

### Task 7: 缩略图 op 引擎面+源端缓存+Windows 后端(IShellItemImageFactory)

**Files:** Create `crates/daemon/src/preview/thumb.rs`、`thumb_win.rs`;Test 单元+e2e。
**Interfaces:** Produces: 响应方 `thumb(path,mtime,size,w)->Vec<u8>`(JPEG);源端缓存 key=hash(path|mtime|size|w) 目录 LRU 512MiB;Windows 后端=COM IShellItemImageFactory(256px;覆盖图/视频帧/PDF 首页/HEIC);失败→`ThumbResponse` 空字节(客户端回退类型图标)。loopback `/thumb` 透传。Consumes: T1/T4。
- [ ] Step 1 失败测试:本地临时图片文件→thumb 两次(第二次零重生成=缓存命中,mtime 变→重生成);视频文件出非空帧;无后端文件类型→空字节。
- [ ] Step 2 红 → Step 3 实现(COM 调用收 spawn_blocking;缓存沿 T5 记账模式独立目录)→ Step 4 绿。
- [ ] Step 5 Commit `feat(daemon): 源端缩略图(Windows IShellItemImageFactory+LRU 缓存)`。

### Task 8: 安卓——可 seek FileSource(fdpass 0x0A)

**Files:** Modify `android/app/src/main/java/com/mtdss/android/fdpass/*`(新通道 0x0A)、Kotlin bridge;`crates/daemon/src/preview/source.rs` 挂桥实现;Test bridge e2e(既有 androidTest JVM 档)+实机面留冒烟。
**Interfaces:** Produces: 桥命令 `open_seek(path)->fd`、read_at 经既有 0x01 seek 语义;SAF/MediaStore 两源;不可 seek 流→Err(触发 T3「导出再读」回退)。Consumes: T3 trait。
- [ ] Step 1 失败测试:JVM 契约测试(协议帧往返/错误形)+Rust 侧 e2e(模拟桥)。Step 2 红 → Step 3 实现 → Step 4 绿。Step 5 Commit `feat(android): 可 seek fd 源桥(0x0A)`。

### Task 9: 安卓——缩略图应答(0x0B)

**Files:** fdpass 新通道 0x0B(MediaMetadataRetriever 视频帧+BitmapFactory 图片含 HEIC,→JPEG bytes);Test JVM+模拟桥 e2e。
**Interfaces:** Produces: 桥命令 `thumb(path,w)->jpeg bytes`,喂 T7 响应方分发(安卓 daemon 收 ThumbRequest→桥)。步骤同 T8(测试→红→实现→绿→Commit `feat(android): 缩略图桥(0x0B)`)。

### Task 10: iOS——可 seek FileSource(资产 URL 直读)

**Files:** ios/ 照片/文件桥扩展(Swift;AVURLAsset/PHAssetResource;桥号按现状顺延);`source.rs` 挂桥。Test swift 单测+模拟桥 e2e。
**Interfaces:** Produces: `open_seek(path)->handle`(未卸载 iCloud 项→Err['icloud_not_local'],UI 提示「原件在 iCloud,需先下载」)。步骤同 T8(Commit `feat(ios): 资产直读源桥`)。

### Task 11: iOS——缩略图应答

**Files:** ios 桥(AVAssetImageGenerator+UIImage 降采样→JPEG)。步骤同 T9(Commit `feat(ios): 缩略图桥`)。

### Task 12: 转码会话引擎面(协议+源端会话管理+查看端装配骨架)

**Files:** Create `crates/daemon/src/preview/rendition.rs`;Modify ctrl;Test e2e(伪转码器)。
**Interfaces:** Produces:
- 源端:`RenditionSessionMgr::open(source,config)->session_id`(config:段长 4000ms/1080p/4-6Mbps 常量)、`segment(index)->Stream`、空闲 60s 回收(Review#2)、后端 trait `Transcoder`(v1 实现=T13/T14;Windows 后端 v1 缺席→open 返回 unsupported→查看端回退原流+海报,留票)
- 查看端(PC daemon):按 session 拉段入 T5 缓存、合成 HLS playlist(`#EXT-X-VERSION:7`/fMP4/`#EXT-X-MAP`),经 loopback 出(Review#1:拉段失败→503)
Consumes: T1/T2/T3/T5。
- [ ] Step 1 失败测试:e2e 伪转码器(3 段已知字节):open→seg0..2 逐字节;seg 越界→Err;cancel 后 seg→Err;60s 回收(注入时钟)。
- [ ] Step 2 红 → Step 3 实现 → Step 4 绿 → Step 5 Commit `feat(daemon): 转码会话协议+源端管理+查看端 HLS 装配`。

### Task 13: 安卓转码器(MediaCodec)

**Files:** android MediaCodec 解→编管线(fMP4 分段 mux;视频 H.264/音频 AAC 直通或转),bridge 命令 `rendition_control`;挂 T12 trait。Test JVM 协议测试+实机冒烟项(冒烟清单)。
**Interfaces:** Produces: `AndroidTranscoder`(段文件落 app cache 交引擎读;seek=就近关键帧重启管线=start_at 语义由「重开 session 起点段」实现)。
- [ ] Step 1 失败测试:JVM=控制协议/错误形;Step 2 红;Step 3 实现管线;Step 4 绿(解码真件留实机);Step 5 Commit `feat(android): MediaCodec 转码管线(HLS 段)`。

### Task 14: iOS 转码器(VideoToolbox)

**Files:** ios AVAssetReader+Writer(VideoToolbox 硬编,CMAF 分段);挂 T12。Test swift 单测+实机冒烟项。步骤同 T13(Commit `feat(ios): VideoToolbox 转码管线`)。

### Task 15: 查看端视频面——loopback HLS 服务+hls.js 消费

**Files:** Modify `preview/loopback.rs`(m3u8/seg 路由)、`preview/rendition.rs`(seek 缺段→向源请求该段/优先);Test e2e+hls.js 侧单测留 T21。
**Interfaces:** Produces: `/f/{h}/video/index.m3u8`(动态 playlist:已缓存段列出,未产段不列;段请求缺→触发源端跳段,有界等待 10s→503)。Consumes: T12。
- [ ] Step 1 失败测试:e2e 伪转码器+hls 播放列表形状断言(首段即列、seek 请求缺段→源收到跳段请求、源离线→503)。
- [ ] Step 2 红 → Step 3 实现 → Step 4 绿 → Step 5 Commit `feat(daemon): 查看端 HLS 服务与跳段 seek`。

### Task 16: previewer 包脚手架+类型路由+图片查看器

**Files:** Create `ui/previewer/`(vite lib,零 React;`src/index.ts`/`router.ts`/`image.ts`);ui 主包接入(静态 import);Test vitest。
**Interfaces:** Produces: `mountPreview(el:HTMLElement, req:PreviewRequest, onEvent:(e:PreviewEvent)=>void):{destroy():void}`;`PreviewRequest={kind:'image'|'pdf'|'video'|'audio'|'docx'|'xlsx'|'pptx'|'md'|'text'|'unsupported', urls:{raw?:string;thumb?:string;jpeg?:string;hls?:string}, meta:{name,size}}`;`PreviewEvent={type:'error'|'retry_needed'|'meta', ...}`。图片查看器:thumb→raw 渐进/zoom-pan/失败停缩略图+重试(spec §5.1)。
- [ ] Step 1 失败测试(vitest):kind 路由分发;图片渐进态机(thumb→raw ok/thumb→raw fail→retry 钮)。Step 2 红 → Step 3 实现 → Step 4 绿 → Step 5 Commit `feat(ui): previewer 包+图片查看器`。

### Task 17: previewer——pdf/文本/代码/md

**Files:** `ui/previewer/src/{pdf,text}.ts`;依赖锁定(pdf.js@pin、highlight.js@pin、markdown-it@pin、dompurify@pin——版本以 npm 现状锁定写入 lock)。Test vitest(假 fetch)。
**Interfaces:** pdf=自定义 range transport 打 raw URL,先渲 3 页/滚动续载;text=64KB 渐进+「加载更多」。步骤:测试(分段请求序列断言)→红→实现→绿→Commit `feat(ui): previewer pdf/文本渲染`。

### Task 18: previewer——office(docx/xlsx/pptx)

**Files:** `ui/previewer/src/office.ts`(docx-preview/SheetJS/pptx 系)。整取走 raw(隐式全量 range);先渲 3 页/500 行+「查看更多」;渲染失败→unsupported 页+下载引导。步骤:测试(「查看更多」翻页/失败降级)→红→实现→绿→Commit `feat(ui): previewer office 渲染`。

### Task 19: 桌面——平铺视图重做+视图记忆

**Files:** Modify `ui/src/components/FileTiles.tsx`、`FileList.tsx`(EntryIcon 缩略图变体)、App.css、`ui/src/lib/`(viewmode kv 持久化新模块);Test vitest。
**Interfaces:** Produces: tile=图标槽(aspect-1,等比不裁切)+两行文字;`EntryThumb(entry,w)`(img src=loopback thumb,失败回退 EntryIcon);三档尺寸 CSS 变量;viewMode 每目录持久(kv 键 `viewmode:{device}:{dir}`;现况纯 useState→迁 kv)。Consumes: T4/T7 URL 面(IPC 桥透传 preview_open 结果——T22 前桌面经 Tauri command 直连 daemon IPC,沿 kv_get/kv_put 先例)。
- [ ] Step 1 失败测试:tile 渲染形状(两行/悬停选择圈);viewMode 切换记忆恢复。Step 2 红 → Step 3 实现 → Step 4 绿(vitest+build)→ Step 5 Commit `feat(ui): Explorer 式平铺重做+视图记忆`。

### Task 20: 桌面——预览面板+弹出独立窗

**Files:** Create `ui/src/components/PreviewPanel.tsx`;Modify App/双击接线/Tauri command(弹出=WebWindow);Test vitest。
**Interfaces:** Produces: 面板(默认 40% 宽/左缘拖调 kv 持久/工具栏 ‹›·下载·打开方式·⧉弹出·✕/Esc 关);点行即换;双击远端&本地文件行=开面板(local=preview_open device 缺省);独立窗=同 previewer 页多开。Consumes: T16-18 bundle、T4/T6/T15 URL。
- [ ] Step 1 失败测试:面板开/关/切换源/宽度持久/弹出命令调用形。Step 2 红 → Step 3 实现 → Step 4 绿 → Step 5 Commit `feat(ui): 右侧预览面板+独立窗弹出`。

### Task 21: 桌面——媒体接线(hls.js/音频)+CSP/回环白名单

**Files:** Modify `ui/src/components/PreviewPanel.tsx`(video=hls.js 挂 hls URL;audio=HTML5 raw)、`ui/src-tauri`(CSP connect-src/img-src 增 `http://127.0.0.1:*`);Test vitest+CSP 快照。
**Interfaces:** Produces: video pane=hls.js(Hls.isSupported() 门,错误→海报+引导);HEIC 图=urls.jpeg 优先。Review#1 断言(503→错误态可重试)。步骤:测试→红→实现→绿(vitest+build)→Commit `feat(ui): 预览媒体面+CSP 回环白名单`。

### Task 22: 安卓——平铺+列表默认+缩略图

**Files:** android browse 面(LazyVerticalGrid 4 列/Coil;列表默认;「⋮」切换持久);Test JVM。Consumes: 桥预览 URL(安卓 daemon loopback 同构)。步骤:测试(网格形态/切换记忆/缩略图回退图标)→红→实现→绿(gradlew)→Commit `feat(android): 平铺网格+列表默认+行缩略图`。

### Task 23: 安卓——预览屏

**Files:** android PreviewActivity(全屏;图片=Compose zoom;视频=Media3 ExoPlayer 原流 raw;文档=WebView(bundle assets+回环口);「···」=下载/转发/打开方式/信息)。Test JVM(路由/降级)+实机留冒烟。步骤:测试→红→实现→绿→Commit `feat(android): 全屏预览屏`。

### Task 24: iOS——平铺+列表默认+缩略图

**Files:** ios browse(LazyVGrid 4 列;列表默认;「···」切换持久 UserDefaults;行缩略图)。Test swift。步骤同 T22(Commit `feat(ios): 平铺网格+列表默认+行缩略图`;Mac mini 构建)。

### Task 25: iOS——预览屏

**Files:** ios PreviewView(全屏 push;图片自研 zoom;AVPlayer 原流;WKWebView bundle;源离线提示;iCloud 未卸载提示透出)。步骤同 T23(Commit `feat(ios): 全屏预览屏`)。

### Task 26: 收口——golden/CI/版本/冒烟清单

**Files:** 版本 1.5.0 四处+lock×2+golden REGEN+Kotlin 断言×5+vc;CI 增 previewer 构建面;Create `docs/smoke-v4p1.md`(按 spec §8 八条编列,含直连/中继双路、HEIC/HEVC 实机、段 seek、限流并发、旧对端矩阵)。
- [ ] Step 1 全量绿:cargo workspace(本机 unset env)+ui vitest+build+gradlew+Mac swift;Step 2 golden REGEN 零漂移断言;Step 3 CI 全矩阵绿;Step 4 冒烟包(Win 双 env 烤入/APK/TestFlight 直传沿 runbook);Step 5 Commit `chore(release): 1.5.0(V4-1 预览波)`。

---

## 执行注记

- 执行=worktree `.claude/worktrees/v4p1` 分支 v4p1(沿既有波纪律);Mac mini clone 同步在 T24/25/26 前
- 转码器实机验证(T13/T14 单测只能到协议层,真解码留 smoke 实机——计划如实分层,不虚标)
- Windows 源×PC 观看 v1=原流直试+海报降级(桌面 MF 转码留票,验收线不涉)
- 每任务=实现+独立评审双闸;整支终审后 CI 全矩阵→实机场 smoke-v4p1
