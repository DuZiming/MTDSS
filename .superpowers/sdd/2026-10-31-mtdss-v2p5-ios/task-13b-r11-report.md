# T13b R11 收尾打磨波报告(20261103.2 设备轮三项:①扩展设备列表=引擎实时 / ②扩展设备行去 › 对齐 App / ③文件夹左滑)

状态:**三项全交付,Kit 216/216 全绿 + BrowseListUITests 9/9 全绿零 skip(含 R11③ 文件夹滑全链新钉)+ sim 构建 app+extension 双绿 + device 编译绿 + 两帧截图** / 基线 c61b0a3 → HEAD(6 commits)
执行日期:2026-11-03。执行面:Windows(编排+自审+对端 dev daemon)+ Mac mini(swift test + build.sh app/device + xcodebuild UITest + 截图)。**WSL/Rust 零产品改动**(R11③ 引擎目录操作支持面 = 纯源码核查,diff 无 crates/)。

## 0. 逐项对账(brief 三条)

### 项 ① 扩展设备列表 = 引擎实时(用户:分享扩展设备列表和 App 内不同步)

**裁定实现形态 = 简报首选「面板载入即早开机」**(cache 先渲染的 swap-in 是其自然组成,非另一案):面板装载序 = 读缓存即刻渲染(不空等)→ `engine.liveDevices()`(早开机 + `devices` 现拉)→ live 换入;boot/devices 失败 = 缓存兜底,**既有陈旧置灰/缺失提示语义只属该兜底路径**。早开机同时买到「点选即发」(点选时引擎已热,`ensureBooted` 短路;run() 内 boot/订阅/受理链零等待),引擎生命周期本就是面板生命周期(收口 shutdown 不变),成本形态不变。

- **Kit 状态机** `ShareDeviceList`(DeviceCache.swift,纯结构体宿主可测):`source = .cache(rows, stale) / .missing / .live(rows)` + `liveUnavailable` 标记;入口 `loadCache`(幂等)/ `adoptLive`(任何态可换,晚到不拒)/ `markLiveUnavailable`(幂等,缓存态原样)。钉测 3 例(ShareQueueTests):cache→live 换入(陈旧面消失+新设备即现)/ live 失败缓存原样保留(幂等)/ missing 缓存也吃 live(缓存写滞后于会话同步形)。
- **编排** `DirectSendEngine.liveDevices()`(扩展目标):闸复用 `available`(会话素材在场 + UDS 预算内,未同步根本不 boot = 缓存语义不变);`ensureBooted()` + `repo.devices()`(RpcGuard 10s 限,同主 App 缓存刷新口径)→ `SharedDeviceEntry` 投影。**`ensureBooted` 增在途 Task 去重**(早开与点选并发 boot 复用同一 Task,不重复拉起;失败同抛,早开=缓存兜底 / 点选=收编队列,各走各降级)。
- **视图** SharePanelView:`@State deviceList = ShareDeviceList()` 替代三枚散状态;投影计算属性 deviceRows/cacheStale/cacheMissing;os_log 增 live skipped/loaded/unavailable 三行(subsystem top.mtdss.ios.share/panel,真机排障面)。
- **不滤本机**(与缓存/主 App 缓存写口径一致,零行为变更面):现拉表=目录原样,本机行同缓存时代呈现(「iOS · 在线」);DevicePickerSheet 的滤己是 App 内弹层语义,扩展面板自 T11 起未滤,不随本轮扩面。
- sim 上 live 路**不可实证**(sim 组容器路径超 UDS 预算,直发/早开恒降级=R10 既有形态,真机形态专属)——状态机语义 Kit 钉死 + 真机轮以 os_log 三行核(live loaded count=N)。

### 项 ② 扩展设备行去 › 对齐 App(用户:还有左边蓝色箭头)

根因 = R5b #3(R4 真机轮 #2「设备行不显 ›」)只改了 App 侧行组件(DevicePickerRow/设备页),扩展自有行 `ShareDeviceRow`(SharePanelParts,T11 mockup .drow 形态)漏网。修:

- 去 `chevron.right`(行首即平台图标,与 App 行同构:图标 22pt/30 列 → 名 17 → 副文 12.5 灰 → 状态区)。
- 按压反馈 `ShareRowPressStyle`(按下行 opacity 0.6,主 App `PressableRowStyle` 同语言;独立命名——SharePanelParts 在主 App target 双编译,同名撞)。`.plain` 时代无任何按压反馈,与 R5b 前主 App 同课。
- idle 状态点 9pt **裸点右贴** 20pt 内缩(去 18 列居中帧,与 App DevicePickerRow 状态点同位像素对齐;busy spinner / launched 绿勾形大仍占 18 列)。
- 置灰/busy/launched 三态语义零变化;DEBUG 状态页(共享件同源)自动随改(截图帧)。

### 项 ③ 文件夹左滑(用户:文件夹要支持左滑,设备里面的和常用目录的,除了收件箱)

**引擎目录操作支持面核查(R11 先行,纯源码核查,零 Rust 改动)——三动作目录语义俱全**:

| 动作 | 引擎面 | 核查点 |
|---|---|---|
| 发送(本地目录) | `flow.rs collect_items` | **递归展开整树**(walk_dir 深度优先,read_dir 按名排序;symlink 跳过/空目录拒/`.mtdss` 保留名拒),rel = `目录名/内部相对路径`——接收端 intake `create_dir_all(parent)` 落盘同构目录树 |
| 下载(远端目录) | `jobs.rs start_download` | orchestrate → **源端 upload 驱动** = 同一 collect_items 展开(操作端只收 paths);回带 rels 全树回填 |
| 转发(远端目录) | `jobs.rs remote_copy_driver` | 预扫 `ctrl.stat`(顶层 is_dir 在案,顶层粒度策略既有)→ from 端 orchestrate = **源端 upload 驱动同一 collect_items 整树**;数据不经操作端 |
| 删除(本地/远端目录) | `core/fs.rs delete_paths` | force=**remove_dir_all 整树删**;非 force=trash 回收(目录受支持);逐条失败表不弃批;卷根/`.mtdss` 逐条拒(ios 恒永久删=cfg 既有口径) |

→ **四动作全部支持目录,零动作剔除,零新契约**(整树 = 单路径入帧,Swift 侧 paths 语义原样)。

- **Kit 动作集** `BrowseSwipeAction.actions` 去 `!entry.isDir` 空集臂:**目录与文件同集**——本地=[发送,删除]/远端=[下载,转发(+删除门)]。钉测 `testFolderSwipeActionsMatchFileSets` 五臂(本地/远端门关/远端门开/远端门关显式/本地门开无涉);原 `testSwipeActionsByRowType` 保留文件臂。
- **收件箱根行恒不可滑(「除了收件箱」)**:根行非 entry 不在 visibleRows 内(与 R5b 选择面同一结构面)→ 动作集函数天然不及根;视图侧根行自绘 Button 也不挂 `NativeSwipeActions`(双保险)。钉测 `testHomeRootRowNeverSwipeable`(展开后 visibleRows 永不含 rootPath)。
- **接入面零新线**:BrowseView/FavoritesView 行滑收口(handleSwipe/pickedForSend/deleteRequest/downloadEntries)对目录原样工作——发送/转发=单条目进选设备弹层(PickedSelection[DirEntry]),下载=直达收件箱,删除=确认弹窗;整树语义随 paths 透传引擎。
- **删除确认文案(文件夹整树点明)**:`DeleteRequest` 增 `folders` 计数——**单目录形标题=「删除文件夹及内容?」**;多选含目录标题保持「删除 N 项?」(目录计 1 项,既有计数机制);含目录副文=「所选文件夹及其全部内容将被永久删除,不可恢复。」;**纯文件形与 R5b 逐字同**(零回归面)。BrowseView/FavoritesView 四构造点(选择×2+滑删×2)随行传目录计数。
- **UITest 翻转**:R6 评审 M-1 空集钉 `testDirectoryRowSwipeHasNoActions` 的前提(目录=空集)随启用翻转 → 正向全链钉 `testDirectoryRowSwipeRevealsFolderActions`:左滑 → [发送,删除] 钮现(删除贴缘帧断言)→ 点删除 → **「删除文件夹及内容?」弹窗现** → 取消零副作用。**本轮实跑绿(11.8s,sim 合成触点)**。
- DEBUG 截图钩子 `MTDSS_DEBUG_DELCONFIRM` 目录优先(单目录形文案面;无目录回退文件 ×2)。

## 1. Commits(顺序)

| # | commit | 内容 | 项 |
|---|---|---|---|
| 1 | `50c5f8a` | feat: 扩展设备列表=引擎实时(ShareDeviceList 状态机+liveDevices+早开机+boot 去重+面板换入) | ① |
| 2 | `1c41c88` | feat: 扩展设备行去 › 对齐 App 行语言(ShareRowPressStyle 按压反馈) | ② |
| 3 | `72bbe90` | feat: 文件夹左滑(Kit isDir 臂+引擎核查+DeleteRequest 文件夹文案+UITest 翻转) | ③ |
| 4 | `a42967d` | chore: DELCONFIRM 截图钩子目录优先 | ③ 截图面 |
| 5 | `69a950e` | fix: idle 状态点裸点右贴(App 同位像素) | ② 收尾 |
| 6 | (本条) | docs: spec §4.1/§4.2/§4.7 + 本报告 + progress + 两帧截图 | 收尾 |

## 2. 验证矩阵(全绿)

| 面 | 结果 |
|---|---|
| Mac `swift test` | **216/216 零失败**(R10 末 211 + ShareDeviceList 3 + 文件夹集 5 臂 1 + 根行不可滑 1) |
| XCUITest | **BrowseListUITests 9/9 全绿零 skip**(文件夹滑新钉 11.8s;远端滑序/门双臂对活对端 44d97f96 直连=Windows dev daemon 本轮起用;传输/相册两套 UITest 与 R11 改动面无涉未复跑) |
| Mac sim 构建 | `build.sh app` **BUILD SUCCEEDED**(app+extension 双 target;早开机链编译实证) |
| Mac device 编译 | `build.sh device` **BUILD SUCCEEDED**(CODE_SIGNING_ALLOWED=NO;扩展吃 device slice) |
| sim 截图 | 两帧 `artifacts/mtdss-r11-sharestates.png`(共享件设备行新形态:无 › + 状态点右贴)/ `mtdss-r11-folderconfirm.png`(「删除文件夹及内容?」弹窗) |
| WSL/Rust | **零产品改动**(引擎目录操作=源码核查,diff 无 crates/;未跑 cargo) |
| 清场 | Windows dev daemon 4108 杀停(生产 57700 未触碰);Mac 收件箱夹具(IMG_×2/Codex/README.md)遗留无害(历轮同形);无临时文件残留 |

## 3. 真机 checklist(R11 增量;R10 报告 §3 九条仍全部有效)

1. **扩展 live 设备列表**:主 App 登录→后台 ≥5min(缓存过期)→ 相册分享 → MTDSS → 设备行应为**新鲜在线态**(App 内设备页同源;非全置灰);os_log panel 类应见 `live devices loaded count=N`;关掉主 App 重登/登出后分享,列表应随会话同步翻转。
2. **分享面板开窗即在线(R10 #2 的窗口扩注)**:早开机使引擎在线窗=面板开→收口(原=点选→收口)——Windows 端 `mtdss devices` 观察 iPhone 在线起点提前到面板打开即在线,收口后短暂离线、主 App 重开恢复,无僵尸在线。
3. **文件夹左滑全链**:首页收件箱展开→子目录行左滑[发送,删除]→发送=对端收**整树目录**(对端 inbox 见 `目录名/内部结构`);删除=「删除文件夹及内容?」确认→整树消失;设备页远端目录行左滑[下载,转发](门开+删除)→下载=本机收件箱落整树;转发=对端 B 收整树。
4. **扩展设备行观感**:无 › 图标+按下行变暗+状态点右贴——与 App 内选设备弹层行同语言。

## 4. Concerns(按重要性)

1. **live 设备列表的真机实证缺位**(sim 结构性不可达):直发/早开在 sim 恒降级(R10 既有 sun_path 形态),live 路只有 Kit 状态机钉 + 编译面;真机 checklist #1 三形(后台过期/登出/重登)为下轮必核面。风险低:devices RPC 是 App 内设备页同源主面,引擎 boot 链 R10 sim E2E 已实证。
2. **早开机的窗口面扩两处**(裁定内,记录防误判):①引擎在线窗=面板开→收口(R10 #2 checklist 起点提前);②R10 concern 3「对端来件撞扩展窗口」(fs.publish 无应答方)同步扩到面板开窗秒级——窗仍秒级+罕见,维持 v1 接受;若真机可复现,候选=boot 期间拒收或页脚提示,再票。
3. **远端目录删除的确认面=单条目文案**:远端文件夹滑删(门开)标题「删除文件夹及内容?」同本地形;远端**整树删=对端 daemon 按对端平台语义执行**(桌面=回收/永久按 force;iOS 对端恒永久)——与远端文件删除同口径(R5b #9 确认面+R9 #3 门),无新增风险面;真删远端目录的端到端留真机轮一次性测试目录(同 R5b concern 6 纪律)。
4. **文件夹发送的接收端冲突策略=引擎顶层粒度既有面**(remote_copy 预扫注记 V1 限制):同名顶层目录交接收端引擎安全网(改名);upload/download 同源语义,非 R11 新增面。
5. **收藏树深层目录展开后滑删父目录**:删除父目录=整树,已展开的子树缓存随 reload 收口(performDelete→vm.reload 既有链);实测留 sim/真机手感面。
6. **「iOS 模拟器」账号噪声**(历轮同形):sim 登录每轮新增离线残项,产线账号 24 台+;本轮 no-op。

## 5. 产物清单

- Kit:`Sources/MTDSSKit/Share/DeviceCache.swift`(ShareDeviceList 新)+ `Ui/BrowseViewModel.swift`(BrowseSwipeAction isDir 臂)
- 扩展:`MTDSSShare/{DirectSendEngine(liveDevices/boot 去重),SharePanelView(状态机换入),SharePanelParts(行对齐)}.swift`
- App:`Browse/{BrowseView,FavoritesView(滑删目录+文案),SelectionBar(DeleteRequest.folders)}.swift`、`Debug/`(DELCONFIRM 目录优先经 BrowseView)
- 测试:`ShareQueueTests(+3 设备列表状态机)`、`BrowseViewModelTests(+2 文件夹集/根行钉,1 改)`、`BrowseListUITests(文件夹滑正向钉,1 翻转)`
- 文档:spec §4.1(行左滑目录同集)/§4.2(删除确认文件夹形)/§4.7(扩展设备列表实时+行无 ›)+ progress + 本报告
- 截图:`artifacts/mtdss-r11-sharestates.png`、`artifacts/mtdss-r11-folderconfirm.png`
