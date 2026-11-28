
## r59 安卓登录页三修(2026-11-27 末十一;APK Synshu-1.5.0-r59-debug 2c7b46bc)

- **#1 徽记/字标居中**:内容父列无水平居中,裸子件靠左——徽记+SYNSHU
  包 fillMaxWidth 居中块(表单通栏不收缩)。
- **#2「请填写服务器地址与邮箱」根修**:UiState.server 空串缺省 + 全新
  安装无 prefs 预填 → 恒落本地校验拒;iOS P4b r2f 的默认域名修安卓漏了
  ——DEFAULT_SERVER=https://www.mtdss.top 内置缺省,prefs 过往值仍覆盖
  (dev 口;高级选项撤后自定义服务器的唯一进入面)。
- **#3 高级选项全撤**(全设备类型):折叠链接/服务器输入框/CA 占位三件
  撤;CA 演练钩子(dev 服务器回显验证码)留代码面。
- 附:LoginViewModel 测试两枚随新默认(空邮箱才落本地校验 + 只填邮箱即
  触 RPC);PreviewBoot dispatch 断言随 r54 window 面(陈旧锚补)。
**部署**:APK `Synshu-1.5.0-r59-debug.apk`(**2c7b46bc**…;Kotlin 单测
544/544)。

## r60 安卓体验轮(2026-11-27 末十二;APK Synshu-1.5.0-r60-debug 2ad95061)

- **#1 edge-to-edge 快修**:新设备上 tab 壳叠乱根因 = 安卓 15+ 对
  targetSdk 35+ 强制全面屏而 App 零 insets 避让——主题四变体
  (values/night/v35/night-v35)+ windowOptOutEdgeToEdgeEnforcement=true
  退出强制,恢复经典系统代避让(MIX3 同象);insets 正修挂后续票。
- **#2 图标全量换装**:官方 material-icons-extended(BOM 管)Rounded
  族——中心图标表 AppIcons 语义命名;选中态 = **同字形变色 + 胶囊底**
  (iOS tab 范式),fill/stroke 两态跳变根除;扫掉手搓 MockGlyph 路径面
  ~36 处(tab 四钮/文件类型/选择圈/底栏动作/顶栏箭头/扫码/齿轮/占位
  行/设置/设备选择/预览返回与全屏);文档图标 = Material Description
  单图标(双路径+问号叠层随撤)。
- **#3 预览滑切(iOS 对齐)**:图片 = zoom 查看器统一手势面(单指∧未
  放大 = 横滑切图消费阻断变换;双指或放大 = 缩放/平移)——自研
  awaitEachGesture 替 detectTransformGestures;视频/音频 = 容器横滑
  (72f 阈,consume 防串控制器);‹› 箭头整件撤,页点保留;
  PreviewGallery 增 video/audio 兄弟集(环绕步进)。
**部署**:APK `Synshu-1.5.0-r60-debug.apk`(**2ad95061**…;Kotlin 全测
绿)。**过程教训:python 字串锚批量替换 Kotlin 三次滑锚(花括号配平/
函数边界/import 前缀)——多行结构改动必须 Edit 工具逐处,python 只做
单行原子替换。**

## r61 安卓双修(2026-11-27 末十三;APK Synshu-1.5.0-r61-debug cc6820b7)

- **#1 底部栏跑顶部根修**(华为/小米 14 实锤;推翻 r60 edge-to-edge 论):
  GlassLayer 的 `fillMaxSize()` 在**自适应高度容器**里取传入约束最大值
  = 整屏高——API 31+(玻璃层才渲染)上胶囊/选择底栏吃满整屏、内容
  weight(1f) 归零、tab 行落到玻璃块顶部 = 「底部栏全跑最上面」;<31
  (MIX3/模拟器)不渲染玻璃层 → 长期潜伏。修 = `matchParentSize()`
  (BoxScope 背景层语义,不参与父容器测量);GlassLayer 改 BoxScope
  扩展,胶囊+选择底栏两处同修。
- **#2 前台恒不熄屏**(用户裁定,与 iOS r25 同语义):
  MainActivity 挂 FLAG_KEEP_SCREEN_ON(窗口可见时才生效,退后台系统
  自管)。
**部署**:APK `Synshu-1.5.0-r61-debug.apk`(**cc6820b7**…;Kotlin 544/544
[一轮 FavoritesViewModel 展开态时序 flake 重跑绿])。**教训:
wrap-content 容器里的 fillMaxSize 子件 = 吞整屏约束——背景/玻璃层
必须 matchParentSize。**

## r62 安卓 iOS 大同步(2026-11-27 末十四;APK Synshu-1.5.0-r62-debug dc96cd88)

七条 + 差距盘点全落(顶栏配色按用户裁定随系统深浅,不强制白):
- **#1 收件箱引导**:InboxGrantCard(无授权置顶快捷目录根;SAF 授权
  Download/MTDSS 持久读写;暂不打扰/设置页可再授)+ InboxGrantStore
  增 inboxInitialUri 预开位。
- **#2 相册帽**:PhotoResponder LISTING_CAP 200→10000(iOS r26d 同修
  漏安卓)。
- **#3 前台不熄屏根修**:撤 KeepScreenOnGate——它无任务时
  clearFlags 把 r61 常亮旗清掉(真凶);常亮旗全语义覆盖。
- **#4 底栏全容器**:撤 IMAGE 门;四钮 iOS 口径(下载/转发/用其他应用
  打开/查看信息;全屏钮由沉浸接管)。
- **#5 图片相册化**:胶片条(52dp 圆角缩略图,当前 accent 描边,点选
  跳图,当前变更居中跟随;浏览同款 PreviewHandlePool+coil,页点随撤)
  + 点按沉浸(zoom 查看器静止点按钩 = 藏顶栏/底栏/胶片条/系统栏
  WindowInsetsController,BEHAVIOR_TRANSIENT_BY_SWIPE)。
- **#6 信息面板**:Material3 AlertDialog 实底 surface + 内容补全
  (名称/大小/格式[容器词+大写扩展名]/路径/设备/修改/创建/访问时间
  ——Contract.DirEntry 补 ctimeMs/atimeMs 解析,引擎 list 已发)。
- **#7 菜单实底**:DropdownMenu containerColor=surface。
**部署**:APK `Synshu-1.5.0-r62-debug.apk`(**dc96cd88**…;Kotlin 全测
绿)。挂账:视频海报框面/open-as 进度面/信息分辨率行(r63 候选)。

## r63 安卓八件套(2026-11-27 末十五;APK Synshu-1.5.0-r63-debug 9f2164d9)

①收件箱零手工:SAF 落点改 Download 根(归一化自动建 MTDSS;旧落点
在目录缺席时选择器报错)+ 文案改「无需手动创建」。②视频/音频海报面
(iOS r49 同象):进去不自动播——视频 = 缩略图+播放钮,音频 = 播放
钮卡,点按起 ExoPlayer;滑切保留(切换回海报)。③文档/PDF 白屏根修:
宿主页 file:// 上 ES 模块被 CORS 拒(自第一天)——迁 WebViewAssetLoader
(https://appassets…/assets/,androidx.webkit;回环 ACAO:* 已在[核验],
localhost http 豁免混合内容)。④图片沉浸:r62 已实(待验)。⑤平铺点击
根修:clickable + detectTapGestures(onLongPress) 同节点叠放 = 手势
检测器吃掉点按——换 combinedClickable(官方,涟漪自带)。⑥磁贴槽底色
撤(iOS r19/r20 同裁定)。⑦open-as:ACTION_SEND + createChooser 系统
App 选择器(微信发送/豆包/WPS 一口全通;iOS ActivitySheet 同象)。
⑧动画轮(全官方件):tab 切换 Crossfade、横幅 AnimatedVisibility
滑入淡出、展开箭头 spring 回弹、预览 Activity 滑入/淡出转场、
combinedClickable 涟漪。
**部署**:APK `Synshu-1.5.0-r63-debug.apk`(**9f2164d9**…;Kotlin 全测
绿;新增 androidx.webkit:1.13.0 依赖)。

## r64 安卓七件套(2026-11-27 末十六;APK Synshu-1.5.0-r64-debug 5690d94a)

①收件箱安装即建:首启 MediaStore RELATIVE_PATH 隐式建 Download/MTDSS
(占位即删零噪音)+ SAF 落点回 Download/MTDSS 直达确认(旧 Download
根形用户还得自己下钻)。②照片图库「不可用」根修:r62 帽 10000 无字节
界 → 0x06 帧可达 ~2MB 撑爆 64KiB 协议界——行装配期累计估重 60KiB 软
界截断(保前缀新→旧)。③视频/音频滑切根修:海报面 clickable/PlayerView
消费 down → detectHorizontalDragGestures(要求未消费)死 = 只能滑一次;
自研容错手势(不要求未消费,水平超阈即切)——目录内全量环绕。④快捷
目录在线/离线分组(任一成员在线 = 在线组;分组头+计数)。⑤文档/PDF
「网页无法打开」根修:资产装载器 path handler 收到的已是 /assets/ 后
余径,再叠 previewer/ = 双前缀 404——open(path) 直取。⑥点点点可见性:
4→5.5dp。⑦open-as 全类型常开(iOS r54 同象:不支持类型现开临时句柄
取毕即关;菜单/底栏撤禁用门)。
**部署**:APK `Synshu-1.5.0-r64-debug.apk`(**5690d94a**…;Kotlin 全测
绿)。

## r65 安卓四修(2026-11-27 末十七;APK Synshu-1.5.0-r65-debug 3fb85baf)

①照片字节界放宽:fdpass 帧上限实测 **8MB**(MAX_PAYLOAD_BYTES)——
r64 的 60KiB 过紧只装 ~300 条 = 「条目过多」再现;6MB 软界(万级照片
~2MB 全量过)。②分组页离线节保留(iOS 逻辑):groupSections 撤 OFFLINE
滤除——节头照出 + 「离线」标注(全员离线不再白页);**根页 r64 在线/
离线分组撤回**(用户裁定)。③滑切 Initial pass:手势改在子件消费**前**
拿原始事件(海报 clickable/播放器消费面不再挡)+ key 含 path(陈闭包
双保险)——连滑根修。④分组页顶栏补 ··· 菜单(刷新/展开全部/收起全部
+ setGroupSectionCollapsed 置位面)。
**部署**:APK `Synshu-1.5.0-r65-debug.apk`(**3fb85baf**…;Kotlin 544/544
——离线节保留两断言随新约)。

## r66 安卓四修 + 照片桥 300s(2026-11-27 末十八;APK Synshu-1.5.0-r66-debug 4ee9b443;引擎改动 Win/Linux 不换包)

①照片桥超时:LIST 10s/EXPORT 60s → **双双 300s**(r42 用户裁定族;大
相册 MediaStore 慢查询 10s 误杀 = 「照片图库不可用,应答超时」根因)。
②PDF/文本白屏两残留:wasm 资产目录进安卓包(pdf JBig2/JPX 解码器此前
404)+ WebView `MIXED_CONTENT_ALWAYS_ALLOW`(https 资产面取 http 回环,
部分 ROM 不认 localhost 豁免 = fetch 静默拦)。③open-as HTTP 410 根修:
r64 把临时句柄 close 排在 fetch **之前**——句柄一关回环 raw 立刻 410;
close 挪到取数完成后。④图片点按沉浸根修:zoom 查看器三职能手势
(缩放平移/滑切/点按)整体重做——Initial pass(子件消费前拿原始事件)
+ 点按臂独立(首末 24px 放宽——r62 的 8px/16px 苛刻阈值真机轻触抖动
5-15px 不触达)。
**部署**:APK `Synshu-1.5.0-r66-debug.apk`(**4ee9b443**…;Kotlin 全测
绿;引擎照片桥改动随包,桌面同码不受影响不换包)。

## r67 六修(2026-11-27 末十九;APK Synshu-1.5.0-r67-debug 494dd053;引擎改动 Win/Linux 不换包)

①图片容器满屏+chrome 覆盖层化(iOS r48 同象):顶栏/底栏改 overlay
浮于内容上,显隐不参与布局 = 点按进沉浸图片大小位置纹丝不动(旧
Column 排布 = 栏显隐挤压内容区 = 图片跳变根因);非图片容器内容自垫
chrome 空间。②撤沉浸 overlay(detectTapGestures 消费全部指针事件压住
查看器滑切/缩放 = 沉浸中滑不动根因;查看器自带 onTap 双向切换)。
③PreviewActivity 常亮旗(独立 window,MainActivity 的旗不跟随 = 预览
中熄屏根因;图片/视频/音频/文本全覆盖)。④分组页菜单对齐(选择模式
挂票 r68——本轮先保持刷新/展开/收起,选择模式为独立工作量)。⑤导航
守卫放行 appassets 资产域(**白屏真凶**:旧 file: 判把 https 宿主页
拦成外链 = 页面从未加载;wasm/混合内容修全被这道门挡在门外)。⑥照片
虚拟根 list 请求 300s(photo_list_budget——池命中 5s/新拨 15s 都按
请求体分档放宽,与照片桥 300s 同口径;其余请求快检不变)。
**部署**:APK `Synshu-1.5.0-r67-debug.apk`(**494dd053**…;Kotlin 全测
绿 + daemon lib 345/345)。挂账 r68:分组页选择模式。

## r68 双修(2026-11-27 末二十;APK Synshu-1.5.0-r68-debug 50ce2771;Win r68 构建中)

①照片 5s 超时的**消费端修正**:r67 的 photo_list_budget 只进了安卓
APK 内嵌引擎——但超时发生在 **PC 侧**(请求方 ctrl 池),PC 跑的还是
r56 老 daemon = 5s 照掐。本轮 Windows r68 带同款引擎修正(PC/iPhone
浏览安卓照片都吃这碗)。
②图片预览 chrome 实底:顶栏/底栏覆盖层加 0.92 黑底(叠图可读,不以
透明糊图);胶片条垫到底栏之上(padding bottom 92dp,不再与底栏叠)。
**部署**:APK `Synshu-1.5.0-r68-debug.apk`(**50ce2771**…);Win
`Synshu-1.5.0-r68-x64-setup.exe`(**2c2d78a5**…,双 env 烤入验证)。**教训:引擎改动落到"请求方"才生效——两端都要换包。**

## r69 分组页对齐 iOS(2026-11-28;APK Synshu-1.5.0-r69-debug 66aceedf)

①菜单 iOS 口径:**选择**(SelectionTopBar + 全选/确认 + 行翻选 + 平铺
胞选择圈)/ **列表视图⇄平铺视图**(双列行组磁贴)/ **排序四项**(名称/
日期/大小/种类,勾标当前;目录恒前基线保留)——groupSort/
groupViewMode/groupSelecting/groupSelection 四态入 VM;groupSections
消费 sort。②单成员恒进分组页(favRowTapTarget 恒 Group——用户裁定
「单成员分组也要显示出来」;离线节保留呈现离线标注)。
**部署**:APK `Synshu-1.5.0-r69-debug.apk`(**66aceedf**…;Kotlin 544/544
——路由断言随新约)。

## r69b 平铺视图根修(2026-11-28;APK Synshu-1.5.0-r69b-debug cc90d12d)

分组页平铺对齐浏览磁贴:**4 列**(GRID_COLUMNS 同值,16dp 侧距/8dp 列距
对齐浏览);**缩略图**(per-cell 句柄 → thumbUrl → coil Crop + 时长
角标 + 名字;per-cell 池 = 分组页跨设备的最准落点——离视口即弃,
daemon 60s 兜底);**可点击**(常态 = 预览推入;选择态 = 翻选);
else/双括号/previewOpen 字段解构/ToastPill 文件私有四轮编译红全收。
**部署**:APK `Synshu-1.5.0-r69b-debug.apk`(**cc90d12d**…;Kotlin 全测
绿)。

## r70 分组页视觉统一 A 案(2026-11-28;APK Synshu-1.5.0-r70-debug 74c5159e)

用户裁定 A(正修):分组页行/磁贴**直用浏览页同款组件**——
- **列表行**:SynshuRow leading 槽挂 ThumbCore(44dp 圆角 12 缩略图 +
  时长角标;图片/视频真缩略图,目录走 FileTypeIcon)——与浏览行完全同形
- **平铺磁贴**:GroupTileCell 换节级池(页面级 Map<device, pool>;
  getOrPut + 卸载全收),r69b per-cell 池随撤
- **ThumbCore** BrowseScreen private → internal(分组页消费)
- 一轮 python 脚本连环滑锚后 git checkout 回干净基线重做(第三次教训
  验证:多行 Kotlin 结构改动必须 Edit 工具)
**部署**:APK `Synshu-1.5.0-r70-debug.apk`(**74c5159e**…;Kotlin 全测绿)。

## r70b 平铺胞对齐收口(2026-11-28;APK Synshu-1.5.0-r70b-debug f5547c47)

用户复核:「列表视图正确,**错误的是平铺视图**」——根因 = r70 的
GroupTileCell 仍走 r69 自造 **AsyncImage Crop**(裁切满格 + 自带槽底色),
与浏览磁贴的 ThumbCore **Fit**(等比留白)不同形。修正 = 平铺胞逐行对齐
BrowseTileItem:
- **thumbEligible 门**(非图/视 → 不走缩略图,TileFileIcon 居中);
- **ThumbCore(Fit)** + 时长角标(9sp/3dp/4dp)+ matchParentSize;
- **TileFileIcon**(BrowseScreen private → internal)兜底图标;
- 槽底色撤(浏览 r63 #6 同裁定);目录行夹 46/文档 40;
- combinedClickable(补 import + @OptIn(ExperimentalFoundationApi));
- 顺手把列表分支选择键的裸 U+0001 控制字节归一为 `\u0001` 转义
  (Read 工具不可见 → 本轮险些误诊;sed `\u` 被 GNU sed 吃成大写
  转义产 `"0001"`,perl `\x5c` 修复——纯源码归一,语义零变)。
**部署**:APK `Synshu-1.5.0-r70b-debug.apk`(**f5547c47**…;Kotlin 全测
绿;compileDebugKotlin/testDebugUnitTest/assembleDebug 三绿)。

## r70c 平铺目录钻取补线(2026-11-28;APK Synshu-1.5.0-r70c-debug acfe5ff4)

用户报「平铺视图下目录无法进去」——r69 重写平铺时 onClick 只搬了选择翻选/
文件预览两臂,漏搬列表臂的 onDrillRow 目录分流,目录点按落进 openPreviewFile
(无面)。修 = onClick 三分支:选择翻选 / **目录 → onDrill(member 设备,
路径,dirName)与列表臂同落点** / 文件 → 预览。
**部署**:APK `Synshu-1.5.0-r70c-debug.apk`(**acfe5ff4**…;compile/test/
assemble 三绿)。
