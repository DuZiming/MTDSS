# V4-1 冒烟测试手册(免下载预览+缩略图+平铺:回环口/HLS 转码/共享 previewer 三端接线)

配套 `docs/superpowers/specs/2026-11-18-mtdss-v4p1-preview-design.md`(§2 回环口 /
§3 按需读 / §4 缩略图 / §5 previewer / §6 三端接线 / §8 验收线八条)与
`docs/superpowers/plans/2026-11-18-mtdss-v4p1-preview.md`(Task 26 收官编制)的逐项展开;
手册风格沿 `docs/smoke-v3p6.md`。自动化面已全绿收口(Rust workspace 双平台 / ui
vitest+build+previewer / gradlew / Mac swift 全绿 + golden REGEN 零漂移 + clippy;引擎面锚:
ctrl_e2e 预览限流/分段族、preview_loopback 本机+远端 raw、seek_source_e2e 交付/回退/照片臂、
rendition_bridge_e2e 主线+回退、contract golden 零漂移),本手册只收**实机判定**:
真解码(H.264 编码器/MediaCodec/VideoToolbox/图片系统解码)、WebView 真渲染(桌面 Tauri/
安卓 file://+universalAccess/iOS 自管 scheme)、pdf.js 206 段取、Coil/NSCache 真出图、
HEIC/HEVC 实机、B 帧设备矩阵、旧对端矩阵、性能温控。全部条目标注来源任务号(T##)。

> ## 前置(实机场前)
>
> ① Windows PC 装 1.5.0 冒烟包(`artifacts/v4p1-field/Synshu_1.5.0_x64-setup.exe`,
> 更新链双串已 grep 验证);启动带 `--log-file` + `RUST_LOG=debug`(info 也够看转码/回环
> 主线;debug 行多段缓存/限流排队)。**PC env 勿设 `MTDSS_TEST_*` 泄漏**。② MIX 3 装
> v4p1 波 APK(`artifacts/v4p1-field/Synshu-1.5.0-debug.apk`,Android 10/MIUI;
> `adb install -r`),留日志窗:`adb logcat -s mtdssd RenditionTranscoder VideoTranscoder
> RenditionResponder ThumbResponder SeekResponder`。③ iPhone16 装 V4-1 TestFlight 包
> (版本 1.5.0;上传通道挂控制器,构建号日期制 20261120.x);当日不可得则 iPhone 退居
> 对照端(装既有 1.4.0 包=旧对端矩阵兼用),转码 iOS 源段顺延。④ **差网场**(§A 中继
> 段):手机热点 + PC 走对称 NAT 侧,或严苛路由器(v3p5 §D3/v3p6 §A 同款)。⑤ 同账号
> 设备在线(PC + MIX 3 + iPhone);账号沿用既有账号。⑥ 素材清单(双端各备):
> - 图片:JPEG 数枚、**HEIC ≥1**(iPhone 相册原件;MIX 3 相机 HEIC 档可选)、大图 ≥4000px ≥1(降采样观察)
> - 视频:**H.264/AVC mp4** ≥1、**HEVC/H.265 ≥1**(iPhone 相机默认档即 HEVC)、**4K 高码率 ≥1**、**竖拍 rotation ≥1**、30/60fps 各 ≥1、**长片 ≥10min**(温控/资源面)、**B 帧源 ≥1**(相机直出 mp4 多含 B 帧;确认法见 §A7)
> - 音频:AAC(m4a)≥1、**非 AAC ≥1**(MP3/AC3)、无音频轨视频 ≥1、源轨缺 cookie 形(iPhone 相录转存件)可选
> - 文档:**PDF ≥5 页**(线性和非线性各 ≥1 更佳)、docx/xlsx/pptx 各 ≥1(pptx 含图表页更佳)、**Office 加密件或损坏件 ≥1**、md/txt/代码文件 ≥1、**大 md/大 txt >64KB ≥1**
> - 不支持类:exe/无扩展名/老 .doc ≥1(类型页+下载引导面)

## 0. 能力边界(本波已交付,自动化面)

| 能力 | 状态 |
|---|---|
| proto 增量只读面:file_read_range/thumb/rendition open·segment·cancel·事件(photo/serve vpath 直通臂对齐;字段只增不改号) | ✅ T1 |
| ctrl 面流式帧序(多帧响应;帽 8/滴流 60s/DataEnd=Sender drop) | ✅ T2 |
| 引擎远端按需读+FileSource 抽象(fs 实现)+独立限流(预览档令牌,浏览>预览>批量) | ✅ T3 |
| 回环 HTTP Range 口(`/f/{handle}/raw`+`/thumb?w=`;token 鉴权+`?t=` 查询令牌等价 Bearer;并发帽 4;预读窗 2MiB+内存 LRU 64MiB;close 即停) | ✅ T4/T20 |
| 预览落盘缓存(preview-tmp+全局 4GiB 磁盘 LRU) | ✅ T5 |
| 桌面 HEIC→JPEG(libheif 动态链;读前 size 快照帽 256MB;/jpeg 路由逐面降级) | ✅ T6 |
| 缩略图 op 引擎面+源端缓存 512MiB LRU+Windows 后端(IShellItemImageFactory) | ✅ T7 |
| 安卓可 seek FileSource(fdpass 0x0D;seek 直读先行→导出件回退→照片臂 0x06/0x07) | ✅ T8 |
| 安卓缩略图应答(0x0B;MediaMetadataRetriever+BitmapFactory;降采样) | ✅ T9 |
| iOS 可 seek FileSource(资产 URL 直读;0x0D 双端同号;iCloud/编辑件守卫词面) | ✅ T10 |
| iOS 缩略图应答(AVAssetImageGenerator+UIImage;0x0B 双端同号) | ✅ T11 |
| 转码会话引擎面(协议+源端会话管理+查看端装配;Windows 桌面源 v1 缺席=unsupported→回退原流+海报) | ✅ T12 |
| 安卓转码器(MediaCodec→AVC;fMP4 HLS 分段 ~4s;1080p≈4-6Mbps 封顶;cutBeforeSync 段界恒关键帧) | ✅ T13 |
| iOS 转码器(VideoToolbox;AVAssetReader 真解;旋转合成烤入;音频转编) | ✅ T14 |
| 查看端视频面(loopback HLS 服务+playlist+段缓存;跳段=乱序直取+重开窗) | ✅ T15 |
| proto 增量 start_at 远跳重开(10s 界;503 Retry-After 重试语义;段 MIME 帽修) | ✅ T15b |
| previewer 包(零 React;类型路由+图片 zoom 查看器) | ✅ T16 |
| previewer pdf/文本/代码/md(pdf.js range transport;64KB 渐进;DOMPurify) | ✅ T17 |
| previewer office(docx-preview/SheetJS/pptx;256MB 双道门;CSP 契约) | ✅ T18 |
| 桌面平铺视图重做(真实图标体系磁贴+缩略图+三档大小+每目录记忆 kv) | ✅ T19 |
| 桌面预览面板+弹出独立窗(WebWindow 多开;‹› 域;CSP 白名单回环+worker-src blob:) | ✅ T20 |
| 桌面媒体接线(hls.js+音频 raw)+时长角标机会主义(duration_secs 可选) | ✅ T21 |
| 安卓平铺+列表默认+缩略图(Coil+netsec 白名单;preview_open 本机臂桥路由) | ✅ T22 |
| 安卓预览屏(四容器;WebView file://+universalAccess;ExoPlayer;boot 注入单点) | ✅ T23 |
| iOS 平铺+列表默认+缩略图(NSCache;ATS 回环豁免进包) | ✅ T24 |
| iOS 预览屏(四容器;WKURLSchemeHandler 自管 scheme 同源面;AVPlayer;Range 转发出站单点;open-as 流式落盘) | ✅ T25 |
| 1.5.0 版本 bump 联动(四处版本文件+lock×2+golden REGEN 零漂+Kotlin×5/Swift×3 断言+android vc7)+CI 全矩阵核对+测试包 | ✅ T26(本任务) |
| Linux/macOS 源缩略图、桌面 MF 转码 | ⏸ V4-3/后票(类型图标降级/原流回退即设计态) |

## 1. 环境与准备

| 环境 | 覆盖测试项 | 准备要点 |
|---|---|---|
| Windows PC(本机) | §A PC 端全部 / §B / §C / §F / §G1 / §H1 | 1.5.0 冒烟包;daemon `--log-file` + `RUST_LOG=debug` |
| MIX 3(Android 10/MIUI,真机) | §A 源端 / §D 全部 / §F / §G2 / §H2 | v4p1 APK;同账号在线;`adb logcat` 窗(TAG 见 §3) |
| iPhone16(TestFlight) | §A 源端 / §D iOS 面 / §E / §F / §G3 / §H3 | V4-1 TestFlight;不可得则退对照端(旧对端矩阵兼用) |
| 服务端/账号 | 全部 | www.mtdss.top;沿用既有账号(验证码走真邮件) |
| 差网场(§A4-§A5) | 中继路 | 手机热点 + PC 对称 NAT 侧,或严苛路由器(打洞必败形态) |

**观察口径(日志面,V4-1 增量指纹)**:

| 指纹(tracing/Logcat 原文) | 端 | 含义 |
|---|---|---|
| `段等待过半未达,远跳重开转码会话`(info,带 target/start_at_ms) | PC | 远跳重开(T15b;§A5 主证) |
| `远跳重开失败(源端面),等待按窗尽收口`(warn) | PC | 重开会话失败,查看端按窗尽收口后走 503 重试语义 |
| `转码桥会话开启失败,转码后端缺席(unsupported)`(warn) | PC | PC 看桌面源视频回退原流(v1 设计态;§H1 佐证) |
| `段缓存件读盘失败,回源重拉`(warn) | PC | 段缓存盘上缺席(外力删除),回源自愈 |
| `起点段 seek: <名> @<ms> ms`(logcat,i) | MIX 3 | 源端 start_at 就近关键帧重开(§A5/A9 主证) |
| `转码完成 <名>(start@<ms>): <n> 段`(logcat,i) | MIX 3 | 全量急产到 EOF 完成(管线主线) |
| `编码器拒基线 profile,落缺省(非单调 pts 运行时拒)`(logcat,w) | MIX 3 | B 帧设备 profile 降级(§A7 佐证) |
| `编码器拒旋转元数据,输出方向退化`(logcat,w) | MIX 3 | rotation 提示面被拒——**方向肉眼判定为主证**(§A6) |
| `编码器输入暂满,弃帧 @<ptsUs>`(logcat,w) | MIX 3 | 编码器背压弃帧(偶发无害;频发=设备瓶颈记档) |
| `0x0C 回执未被受理 / 段回告发送失败 / 转码管线失败`(logcat,w) | MIX 3 | 会话链故障面(§A10 失败臂观察) |
| `取消事件缺 session id,丢弃` / `段目录清理失败`(logcat,w) | MIX 3 | cancel 竞窗/清扫失败(§A10/A11 观察) |
| `rendition-serve 清扫失败`(logcat,w) | MIX 3 | 挂载清扫异常(正常缺席) |
| `thumb 缓存临时件写失败/就位失败(本次不缓存)`(warn) | PC | 源端缩略图缓存写失败(降级不缓存;正常缺席) |
| `thumb 桥导出不可达,空字节回退` / `thumb 照片回退:*`(debug) | PC | 缩略图回退链(§F 权限拒/离线面) |
| `预览回环服务退出`(warn) | PC | 回环口异常退出(正常缺席) |
| `打洞穷尽,armed 记忆建立` / `armed 记忆命中` / `回切:关闭中继半连`(v3p6 既有) | PC | 直连/中继路归因(§A4 分路用) |

## 2. 测试项清单(按 spec §8 验收线八条编列)

> 每项:前置 / 操作 / 预期;标注来源任务号。**实机场:用户在场协作**;建议顺序:
> **§H(本机预览,先验壳免远端因素)→ §C+§F+§G(桌面文档/缩略图/语义)→ §D(移动端
> 互看)→ §B(PDF/office 续载)→ §A(转码实机矩阵+差网场,环境折腾放最后)**。
> 验收线 8(golden/CI/proto 纪律)为自动化面,收口报告已录(零漂 REGEN+四套件绿+CI
> 全矩阵核对),实机场无操作项,文末 §5 记录表留一行归档位。

### A. 验收线 1——三端互看+视频一律转码 HLS(PC 方向;直连/中继双路+段级 seek)

- [ ] **A1 安卓源→PC 转码主线:SAF 根视频 HLS 出画+拖动**(T13/验收线 1)
  - 前置:MIX 3 在线,SAF 授权根备 H.264 mp4;PC 预览面板打开该视频。
  - 操作:等首段出画;拖动进度条数枚位置;观察 PC daemon 日志。
  - 预期:面板内 hls.js 出画(画质≈1080p 封顶);拖动后 1-2 段内续播;段文件落
    preview-tmp;logcat 见 `转码完成`/段产出。
- [ ] **A2 安卓源→PC:照片图库(照片桥 vpath)视频 HLS**(T8/T13)
  - 操作:PC 对 MIX 3「照片图库」树内视频起预览。
  - 预期:seek fd 直读先行(serve-tmp 恒空=不经整件导出;daemon 日志无 0x09 触发即
    佐证);HLS 出画同 A1。
- [ ] **A3 iOS 源→PC 转码主线:照片图库/收件箱视频**(T14;iPhone 无 TestFlight 则顺延)
  - 预期:同 A1 形;iPhone 侧编码器启动无明显发热即续播;旋转竖拍视频方向正确(旋转
    合成烤入=T14 臂,与安卓提示面分叉)。
- [ ] **A4 直连与中继两路各验一次**(验收线 1;差网场)
  - 操作:同一视频分别在①直连场(同网/打洞成功:`回切`/warm 行在场)②差网场
    (打洞穷尽→中继:`中继兜底已建立` 行在场)各起预览;PC 段拖动同 A1。
  - 预期:两路均出画;中继路段拉取耗时略高属预期;预览不受批量传输饿死(同时跑一枚
    大文件下载,预览仍可拖动=独立限流 T3 佐证)。
- [ ] **A5 段级 seek:近跳缓存即响应 / 远跳 start_at 重开**(T15/T15b/验收线 1)
  - 操作:①已播区段内回拖(近跳)→秒级续播;②拖到远未转码位置(远跳)→PC 日志见
    `段等待过半未达,远跳重开转码会话`,logcat 见 `起点段 seek: … @… ms`;③远跳后
    **回拖**到远跳锚之前的未缓存旧段(重开洞域,§A5b)。
  - 预期:近跳即时;远跳在新 start_at 关键帧起出画(起点=就近关键帧,非逐帧精确);
    洞域段=止步缓冲/404 面(v1 取舍=重开只前进,见 §A5b 注)。
- [ ] **A5b 重开洞域/换道观察面**(T15b,如实记录不设门槛)
  - backward 相邻 seek(目标 ≤ 前沿)恰逢慢产 → 5s 窗触发重开换道(App 就近关键帧
    重启近似零损耗,丢弃在途生产提前量=已知取舍);
  - start_at=目标段索引×段长,亚段对齐误差由 playlist EXTINF 承载(目验:跳转落点
    与进度条位置亚段级偏差可感不致错乱)。
- [ ] **A6 兼容矩阵:HEVC/4K/rotation/帧率**(T13/T14/验收线 1)
  - 操作:依次预览 HEVC 源、4K 高码率源、竖拍视频、30fps 与 60fps 各一枚(双手机源
    各过一遍,桌面源走 §H1 回退臂)。
  - 预期:HEVC 源转 AVC 正常出画(源端解码面真跑);4K 源出画且码率被 1080p≈4-6Mbps
    封顶(带宽占用目测低于原文件码率);竖拍方向正确(安卓=MediaFormat rotation 提示
    面,**若 logcat 出现「编码器拒旋转元数据,输出方向退化」且画面方向错误,记档开
    票——MIUI 设备提示面被拒即设备矩阵露洞**;iOS=合成烤入恒正确);30/60fps 均出画
    不呈快慢放。
- [ ] **A7 B 帧设备:「pts 非单调」失败链→查看端回退原流**(T13)
  - 前置:B 帧源(手机相机直出 mp4 多含;确认法:ffprobe 见 has_b_frames≥1,无 ffprobe
    则跳过确认直接试)。
  - 操作:PC 预览该视频;观察 logcat。
  - 预期:两分支其一——①编码器拒基线 profile 落缺省后正常转码出画(logcat 见
    「编码器拒基线 profile」);②管线报非单调 pts 失败 → PC 端**回退原流播放**(面板
    仍出画=ExoPlayer/hls 回退语义;回退形态=原流直传,拖动走原流 range)。两分支均
    合格,回退分支附日志记档。
- [ ] **A8 音频两臂:直通/转编/无轨/缺 cookie**(T13/T14)
  - 操作:依次预览 AAC m4a、非 AAC(MP3/AC3)、无音频轨视频、源轨缺 cookie 形。
  - 预期:AAC 直通出声;非 AAC 转编出声;无轨纯画面不报错;缺 cookie 兜底面不炸管线
    (iOS 臂;音频哑或降级记档)。
- [ ] **A9 seek start_at>0 重开:起点段出画**(T13/T14)
  - 并入 A5②;单列预期=起点段恒自关键帧起出画(无花屏/绿帧首段)。
- [ ] **A10 资源面:关预览即 cancel;失败已产段保留**(T13/T14/验收线 6)
  - 操作:①预览中关面板/退出全屏→logcat 见 cancel 事件与段目录清理,PC 侧段缓存目
    录清空、CPU/编码器释放(发热停止);②构造管线失败(预览中飞行模式断源端网络)
    →已产段保留(不误删)、查看端错误面+重试钮。
  - 预期:cancel 及时有界(≤秒级);失败臂已产段保留且重试可续。
- [ ] **A11 挂载清扫:杀进程残段→下次挂载清空**(T13/T14)
  - 操作:转码中 `adb shell am force-stop com.mtdss.android`(或杀 mtdssd)→重开 App。
  - 预期:残留段目录在下次挂载/会话开启时清空(段目录无跨会话堆积)。
- [ ] **A12 性能温控:1080p 速率与长会话温控;长片音频预排水**(T13/T14)
  - 操作:长片(≥10min)预览 ≥5min,双端各一轮。
  - 预期:转码速率 ≥ 实时播放速率(进度条不被生产节拍追平);长会话温控降频形态=
    出画卡顿而非断流(记录设备型号+表现);长片音频转编臂无「音频越拉越落后」(预
    排水防线=iOS T14 臂,听感音画同步)。
- [ ] **A13 HEIC 图片在 PC=libheif 转码后正常渐进**(T6/验收线 1)
  - 操作:PC 预览 iPhone 源 HEIC。
  - 预期:缩略图先显→/jpeg 端点渐进出全图;超大 HEIC(>256MB 形难构,可跳过)按
    size 门诚实拒;中途失败=停在缩略图态+重试钮(逐面降级语义)。
- [ ] **A14 手机看视频=原流直传拖动;桌面源 HEVC→手机同样豁免**(验收线 1/T23/T25)
  - 操作:MIX 3/iPhone 互看对方视频(含桌面源 HEVC);拖动进度条。
  - 预期:原生播放器(ExoPlayer/AVPlayer)原流 range 直传出画可拖;**不转码**(logcat
    无转码行=豁免主证);PC 看桌面源视频=原流直试+海报降级(§H1)。

### B. 验收线 2——PDF 翻页续载+office 前 3 页+查看更多

- [ ] **B1 PDF 翻页续载:第 4 页起触发续取**(T17/验收线 2)
  - 操作:PC 预览 ≥5 页 PDF,滚到第 4 页;PC daemon 日志观察回环口 range 序列。
  - 预期:首取 3 页字节;第 4 页起新增 range 请求(续取);页码指示正确;非线性化
    PDF 续取同样成立;翻回已载页零新请求(range 缓存)。
- [ ] **B2 PDF 跨端同面:安卓 WebView/iOS WKWebView**(T23/T25)
  - 预期:移动端 pdf.js 同 bundle 真翻页;iOS 侧经自管 scheme 代理臂 **206 段取真渲
    染**(pdf.js 按 Accept-Ranges 广告发段,恒收 206;若弹「Unexpected server
    response」=代理臂回归,记档开票——T25 fix#1 钉面)。
- [ ] **B3 大文本/大 md「加载更多」断点续读;代码高亮命中**(T17/验收线 2)
  - 操作:预览 >64KB 的 md/txt 与一枚代码文件。
  - 预期:头部 64KB 渐进显出;「加载更多」续读到位;高亮语言自动命中(常见三样
    js/py/rs);md 渲染经 DOMPurify 无样式逃逸。
- [ ] **B4 docx 分页+「查看更多」**(T18/验收线 2)
  - 预期:后台整取(≤64MB)后先出前 3 页;「查看更多」续页;分页与 Word 桌面打开
    语义大致对齐(允许分页引擎差异,记档观察)。
- [ ] **B5 xlsx sheet 标签切换+大表续行**(T18)
  - 预期:首 sheet 首屏行(500 行)先出;sheet 标签可切;大表滚动续渲不卡死;列数
    无帽面(极端宽表卡顿记档,T18 M5)。
- [ ] **B6 pptx 幻灯片渲染+滚动续页+图表**(T18/T19 交接)
  - 预期:逐页渲;滚动续页;图表页经可选 loader 懒注册出图(失败=图表占位不炸页);
    **blob classic worker 面**——若 pptx 页静默不渲,查 CSP `worker-src blob:`(容器
    必验交接项,T18→T20/T21/T22-25)。
- [ ] **B7 Office 加密件/损坏件→unsupported+下载引导**(T18/验收线 2/spec §7)
  - 预期:类型页+「下载/用其他应用打开」;无半渲残面。
- [ ] **B8 docx 病理性全局 CSS 目验**(T18 M2 挂账)
  - 操作:打开含极端 STYLE 节点的 docx,观察宿主页其余元素。
  - 预期:样式不外溢宿主面板(全文档作用域收敛;若污染面板背景/字体,记档开票)。
- [ ] **B9 中继/直连两路各验一次整取耗时与 spinner 文案**(T18)
  - 操作:同一 docx 分别直连/中继打开。
  - 预期:两路均可打开;spinner 文案面正确;中继耗时略高属预期。

### C. 验收线 3——桌面平铺:磁贴+真缩略图+三档大小+每目录记忆

- [ ] **C1 磁贴真实图标体系+三档大小+每目录记忆**(T19/验收线 3)
  - 操作:桌面平铺视图滚轮/Ctrl± 切三档;切目录再回。
  - 预期:文件夹 78px/文件页形 66px 档真实图标体系;三档缩放即时;形态按目录记住
    (kv 持久化);重启后仍在。
- [ ] **C2 Windows 源真缩略图:图片/视频抽帧/PDF 首页/HEIC**(T7/验收线 3)
  - 操作:PC 本机目录(平铺)含图片/视频/PDF/HEIC 各数枚。
  - 预期:IShellItemImageFactory 系统出图全覆盖;视频=抽帧+时长角标(机会主义:桌面
    目录扫描不读 shell 属性→**桌面源角标可缺席,属设计态** T21 裁定);HEIC 有图
    (系统解码);失败项=类型图标不空槽。
- [ ] **C3 手机源真缩略图:HEIC/视频抽帧**(T9/T11/验收线 3)
  - 操作:PC 平铺浏览 MIX 3「照片图库」与 iPhone 相册目录。
  - 预期:图片(含 HEIC)真缩略图;视频=抽帧图;降采样大图不出 OOM/超时;源端缓存
    生效见 §F。
- [ ] **C4 悬停浮出选择圈+快捷下载;目录第二行日期**(T19)
  - 预期:悬停浮出选择圈(Ctrl 切换=与行同语义)+快捷下载钮(仅远端设备目录位文件
    行);目录磁贴第二行=日期(**「N 项」无数据源不显=波级决策在册,终审裁**);文件
    =「类型 · 大小」。
- [ ] **C5 弹出独立窗(WebWindow)多开+多屏 DPI**(T20)
  - 操作:预览面板 ⧉ 弹出独立窗;多开两枚;若有多屏环境拖窗跨屏。
  - 预期:独立窗加载同一 previewer 页,与面板互不排斥;多屏 DPI 无模糊/错位(DPI
    打磨冒烟项 T20 挂账;单屏环境可跳过并注记)。
- [ ] **C6 滚量大目录缩略图排队观察**(T19 挂账)
  - 操作:平铺浏览 ≥500 文件目录,快速滚动。
  - 预期:缩略图逐可见批出图;若滚动卡顿可感(串行 IPC 排队,每可见缩略图一枚
    preview_open/close 往返),记档开票(归并/节流候选)。

### D. 验收线 4——移动端:列表默认+平铺 4 列+行缩略图+手势+全屏转场

- [ ] **D1 安卓:列表默认+平铺可切(4 列)+记忆**(T22/验收线 4)
  - 预期:浏览页默认列表;「⋮/···」切平铺一行 4 个;重启后记忆保持;磁贴≈76-82pt
    严格内收,名字一行截断。
- [ ] **D2 安卓行缩略图/平铺缩略图真出图(Coil)**(T22)
  - 预期:44/40px 行缩略图方图;视频带时长角标(照片库源在册即填;SAF 源可缺席=
    机会主义设计态);Coil 真加载回环口 thumb(白名单/netsec 面);本机 SAF/收件箱
    缩略图经桥路由也出图(T22 fix「自机缩略图双端点亮」真机复验)。
- [ ] **D3 安卓图片查看器手势**(T23/验收线 4)
  - 预期:全屏推入沉浸;左右滑切兄弟图(图库环绕);双指缩放 1x-8x;放大拖拽;1x
    复位;thumb 先显→raw 渐进;HEIC=jpeg 端点优先降级;页点 ≤20 枚(>20 只 ‹›)。
- [ ] **D4 安卓文档容器 WebView file:// 真渲染**(T23——本波安卓最大真机风险面)
  - 操作:安卓预览 PDF/md/txt/代码/docx。
  - 预期:file:// 宿主页+assets bundle+`allowUniversalAccessFromFileURLs` 下 ES
    module 加载与 fetch 回环口全通(PNA 行为实机确认);失败形态=「缺少预览参数」
    或 fetch 降级字面(记档即露洞开票)。
- [ ] **D5 安卓视频/音频原流直传**(T23)
  - 预期:ExoPlayer 原流直连可拖;错误面=字面+下载引导+重试整链重建;后台即暂停。
- [ ] **D6 安卓「用其他应用打开」(open-as)**(T23)
  - 预期:在场句柄 raw 拷 cache→系统选择器拉起;名字净化扩展名保留;大文件耗时/
    流量感知(整文件下载=显式用户动作,可接受);不支持格式无此入口。
- [ ] **D6b 安卓旋转重建观察**(T23 已知取舍)
  - 操作:预览屏停留时旋转设备(竖↔横)。
  - 预期:预览态复位回落列表点入即恢复(v1 未做 state retain;旧句柄组合销毁序正常
    关闭,无泄漏无挂死=合格);若出现双句柄泄漏/ANR 记档开票。
- [ ] **D7 iOS:列表默认+平铺+行缩略图**(T24/验收线 4)
  - 预期:同 D1 形(UserDefaults 记忆);NSCache 同 URL 复进零重拉;磁贴下拉刷新
    (refreshable iOS 16.4+)真加载(ATS 回环豁免键已进包真机确认)。
- [ ] **D8 iOS 预览屏:WKWebView 自管 scheme+AVPlayer+HEIC jpeg 端点**(T25)
  - 预期:文档=WKWebView 载宿主页+scheme handler 供 bundle 资产+反解代理回环口
    (同源零 CORS 预检);视频/音频=AVPlayer 原流直连起播可拖;图片 HEIC=jpeg 端点
    优先降级出图;Range 转发逐字(pdf.js 段取);「用其他应用打开」流式落盘(大件不
    炸内存);导航守卫(file:+mtdsspreview 放行,http(s) 交系统浏览器)。
- [ ] **D9 iOS 图片查看器手势+图库**(T25/验收线 4)
  - 预期:双指缩放 1x-8x/放大拖拽/1x 复位;‹› 切换兄弟集(图库集=兄弟图片类);页
    点 ≤20;全屏转场(浅色骨架+深色观看面)。
- [ ] **D10 移动端全屏预览转场+菜单四项**(T23/T25/验收线 4)
  - 预期:全屏推入转场;「···/⋮」菜单=下载/转发/用其他应用打开/查看信息 四项全容
    器;信息=名字·大小·路径·设备。

### E. iCloud/编辑件/权限(iOS 特有)

- [ ] **E1 iCloud 未卸载项:「原件在 iCloud,需先下载」**(T10/T25)
  - 操作:iPhone 自机照片库预览一枚已卸载原件的 iCloud 项。
  - 预期:明确提示词,不静默全量拉(icloud_not_local 稳定词透出);**远端预览(PC 看
    iPhone 的 iCloud 项)仍静默走导出全量拉=v1 取舍**(T10 在案,如需透出提示另立
    票——观察记录)。
- [ ] **E2 已编辑照片 size 门**(T10 挂账)
  - 操作:预览一枚「已编辑」(裁剪/滤镜后)照片。
  - 预期:正常出图(渲染件);若渲染件 size ≠ 原片声明 size 触发 NOT_FOUND 诚实拒
    →查看端重新浏览/走下载(露洞则记档开票:拒词特判或声明口径改渲染件)。
- [ ] **E3 照片权限拒绝拒答链**(T9/T11)
  - 操作:App 无照片权限时 PC 预览其照片图库项。
  - 预期:thumb/读拒答链完整不挂死;查看端得到明确失败面(权限授予后恢复)。
- [ ] **E4 大图降采样耗时**(T9/T11)
  - 操作:预览 ≥4000px 大图(缩略图+全图两跳)。
  - 预期:缩略图秒级;全图渐进可感不超时;无 OOM。

### F. 验收线 5——缩略图源端缓存+源文件改动刷新

- [ ] **F1 二次浏览零重生成**(T7/验收线 5)
  - 操作:首次平铺浏览目录(缩略图全出)→切走再回/重启 App 再回。
  - 预期:二次浏览秒出(源端缓存 key=hash(path|mtime|size|w) 命中;PC 侧 debug 日志
    无重新生成交织;移动端 NSCache/Coil 内存+磁盘双层命中零重拉)。
- [ ] **F2 源文件改动→缩略图刷新**(T7/验收线 5)
  - 操作:源端替换目录内一枚图片(内容变,mtime 变)→PC 重新浏览。
  - 预期:新缩略图出图(key 失效重生成);旧图不滞留。安卓 ThumbCore 键 mtime 滞旧
    边角(T22 M5:同名覆盖 mtime 不变形)如遇记档。
- [ ] **F3 源端缓存 LRU 上限**(T7)
  - 观察:大量目录浏览后源端 thumb 缓存目录占用有界(512MiB LRU);超限淘汰不报错。

### G. 验收线 6——预览语义:不进任务/不落收件箱/源离线/关面板断流

- [ ] **G1 预览不进任务列表、不落收件箱**(验收线 6/全链)
  - 操作:预览若干文件(图片/视频/PDF/office)后查两端任务列表与收件箱。
  - 预期:零任务记录、零收件箱落盘;preview-tmp 落盘不在用户可见目录(整取/段缓存
    属内部面)。
- [ ] **G2 源离线=打开即明确提示,不排队不等待**(T20/T23/T25/验收线 6)
  - 操作:源端飞行模式/杀 App 后,PC 与另一手机预览其文件。
  - 预期:面板/全屏即出「源设备离线」面(离线族稳定词:设备离线/peer_offline/
    source_offline/源打开失败);不转圈等待不排队。
- [ ] **G3 关面板/退出全屏即断流(日志验证)**(T4/T20/T23/T25/验收线 6)
  - 操作:预览大视频/大文件中关面板(桌面)或退出全屏(移动)。
  - 预期:handle 引用计数归零即停读(桌面 `close 后 410` 面=e2e 已钉,实机以「源端
    传输活动停止+CPU 回落」目验);移动端句柄收口(条目切换旧 close、屏面卸载全量收、
    daemon 60s 空闲回收兜底);logcat 无持续读活动。
- [ ] **G4 预览与批量传输互不饿死**(T3/验收线 1 附带)
  - 操作:大文件传输进行中起预览并拖动。
  - 预期:预览可开可拖(预览档令牌独立;传输限速让路);传输速率受控下降不中断。

### H. 验收线 7——本机文件同面板预览 + 旧对端矩阵

- [ ] **H1 PC 本机文件同面板预览**(T20/验收线 7)
  - 操作:本机文件(图片/视频/PDF/docx/md)走同一预览面板打开。
  - 预期:读本地盘不经远端;**本机视频=原流直试+海报降级**(Windows 桌面转码 v1 缺
    席=unsupported→回退臂,logcat/daemon 日志见「转码后端缺席」;桌面 MF 转码留票
    属后波);本机缩略图照出(IShellItemImageFactory)。
- [ ] **H2 安卓本机(SAF/收件箱)预览**(T22/验收线 7)
  - 预期:SAF 授权根与收件箱文件同预览屏可开(本机臂 std::fs 直开;缩略图经桥路由
    T22 fix 真机复验=§D2)。
- [ ] **H3 iOS 本机(收件箱/授权根)预览**(T24/T25/验收线 7)
  - 预期:收件箱件同预览屏可开;照片库自机项走 PhotoKit 直读。
- [ ] **H4 旧对端矩阵**(T8/T9/T10——混合版本两两对)
  - 矩阵:旧 App(1.4.0)×新 PC、新 App(1.5.0)×旧 PC(回滚 PC 包对照)、安卓新×iOS 旧。
  - 预期:旧对端无 fdpass/缩略图/转码桥 → 引擎回退既有语义(缩略图=空字节类型图标、
    读=导出再读/直传、转码=503/unsupported 诚实失败面);无挂死无崩溃;升级后各面
    点亮(只增不改号=新旧互通底线)。

### I. 验收线 8——golden/CI/proto(自动化,收口已录,实机场无操作)

收口记录(T26 报告):golden REGEN 零漂移(仅 hello.resp.json version 行)、「四处版
本文件+lock×2+Kotlin×5+Swift×3+android vc7」联动全落、Rust workspace 双平台全绿、
ui vitest+build+previewer 绿、gradlew 全绿、Mac swift 全绿、clippy 双平台净、CI 全矩
阵对波内新面核对(previewer 构建步在 ui job/android job 走仓内 bundle 无 node 步/ios
job swift 纯函数面)。proto 纪律=字段只增不改号(T1 REGEN+T15b start_at 增量背书)。

## 3. 排查速查

| 症状 | 首查 | 备注 |
|---|---|---|
| PC 预览面板白屏/「缺少预览参数」 | CSP 控制台(connect-src/img-src/worker-src 回环+blob:) | worker-src blob: 缺=pptx 静默不渲(§B6) |
| PC 视频 200 全量不出段 | 回环口 Accept-Ranges/206 响应面;hls.js 控制台 | 恒 200=假广告回归面(T25 iOS 同款) |
| 安卓文档容器「缺少预览参数」 | logcat WebView console;file:// ES module 加载 | PNA/universalAccess 面(§D4 失败形态) |
| iOS 文档页 fetch 拒 | scheme handler 反解;ATS 豁免键 | 同源面零 CORS 预检;见拒=handler 回归 |
| 远跳后卡缓冲不动 | PC 日志 `段等待过半未达,远跳重开` 缺席/在 | 洞域=锚前未缓存段止步(v1 取舍 §A5b) |
| 竖拍视频方向错 | logcat `编码器拒旋转元数据,输出方向退化` | 安卓提示面被拒=设备矩阵露洞,记档开票(§A6) |
| B 帧源不出画 | logcat 转码管线失败行;PC 端回退原流 | 回退分支合格(§A7);两臂皆败才开票 |
| 缩略图全空 | `thumb 桥导出不可达`/权限链 debug 行;源端 thumb-tmp | 旧对端/权限拒=预期回退;新对端全空=露洞 |
| 关面板后源端仍读 | PC daemon close 后 410;移动端句柄收口 | 引用计数/卸载收口链(§G3) |
| 预览把传输饿死 | 预览档令牌独立限流(T3) | 反向同样;两向饿死均露洞(§G4) |

## 4. 实机场信息与记录格式

- 设备:PC(Windows,1.5.0 冒烟包)/ MIX 3(Android 10/MIUI,v4p1 APK)/ iPhone16
  (V4-1 TestFlight);服务端 www.mtdss.top;差网场=手机热点+PC 对称 NAT 侧。
- 包位:`artifacts/v4p1-field/`(Win 安装包/APK/linux tar/macos tar,各附 sha256
  侧车;README 见同目录);TestFlight 上传挂控制器(runbook 通道)。
- 记录格式(逐项):`[项号] 日期 | 结果 ✓/✗/降级记档 | 观察摘录(日志行/截图位)|
  露洞→票号`。
- 判级:✗=验收线主证面失败;降级记档=v1 取舍/机会主义缺席面(§0 ⏸ 与各项注);两项
  headline=**A1/A3(转码主线出画)+A5(段级 seek 含远跳重开)**,其余按序。
