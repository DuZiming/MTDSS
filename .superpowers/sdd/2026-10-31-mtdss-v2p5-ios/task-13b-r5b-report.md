# T13b R5b 功能波报告(#1 首页树选择 / #3 设备列本机行 / #7 行左滑 / #8 选择栏垃圾桶 / #9 删除一律确认)

状态:**五项全交付,Kit 155/155 全绿 + sim 构建绿并装机 + 五帧截图** / 基线 8911b5a → HEAD(docs 收尾前 = f126c3e,9 commits)
执行日期:2026-11-02。执行面:Windows(编排+自审)+ Mac mini(swift test + build.sh sim + 截图)。**WSL/Rust 零改动**(diff 仅 ios/ 与 docs/,crates 无涉;远端删除走既有 ctrl delete 契约,golden 已钉)。

## 1. Commits(顺序)

| # | commit | 内容 | 项 |
|---|---|---|---|
| 1 | `4cc3bba` | feat(ios): 首页常用目录选择模式:「···」/子行长按双入口,根行不可选,底栏分享/删除+居中确认,标签栏隐藏;PickedSelection/DeleteRequest 提共选件 | #1 |
| 2 | `1d0d67b` | feat(ios): 设备列表显示本机行:去 T7 本机滤除,「平台 · 本机」+禁点;DeviceIcon 提 Kit + DevicesRowFormat 纯函数 | #3 |
| 3 | `c83d57b` | feat(ios): 行左滑动作:SwipeableRow 自 T9 通用化迁 Common,浏览文件行 本地[发送,删除]/远端[下载,转发];Kit BrowseSwipeAction+deletePaths 拆分;首页子行同款;删除确认改居中 alert | #7 |
| 4 | `ea123b2` | fix(ios-tests): XCTUnwrap 不收 await(编译修) | 测试修 |
| 5 | `e534246` | feat(ios): 远端选择底栏补删除:动作集 下载/转发/删除,deleteSelected 按 vm.deviceId=对端收口;Kit 钉远端删除帧 | #8 |
| 6 | `ba0dbed` | fix(ios-tests): JSONValue 对象解构去多余 `?` | 测试修 |
| 7 | `0194564` | fix(ios-tests): 远端删除钉测伪 daemon 根卷给真实条目 | 测试修 |
| 8 | `a48e306` | fix(ios): 信任目录移除确认改居中 alert(全 App 删除路径确认统一,dialog 归零) | #9 |
| 9 | `f126c3e` | chore(ios): MTDSS_DEBUG_DELCONFIRM 截图钩子(DEBUG 面,真机/产线不编译) | 截图面 |
| 10 | (本条)docs: 本报告 + spec 三处修订 + progress 状态行 + 五帧截图 | 收尾 |

逐项提交;BrowseView 跨 #7/#8/#9 三项、FavoritesView 跨 #1/#7 两项——按项拆分编辑,最终态与逐项叠加态一致(两次中间态均过 swift test)。

## 2. 两个 brief CHECK 问题的调查结论(先查后建)

### #3 CHECK:daemon `devices` RPC 是否含本机 → **恒含,零合成**

逐环实证:server `/devices`(server/signaling/src/api.rs list_devices)= `SELECT … FROM devices WHERE account_id=?` **全账号设备,无请求方排除**;daemon `flow.rs fetch_devices` 原样透传(devices 无 self 过滤);T7 的「本机滤除」纯是 iOS 客户端侧(`DevicesViewModel.project(excluding:)`)。→ **不做 status 合成行**,去滤 + 标记即得(设备数据单源,不造平行真源)。

### #7 CHECK:browse 是 ScrollView+LazyVStack,swipeActions 需 List → **复用 T9 SwipeableRow(通用化)**

现状确认:SinkingFooterList = ScrollView+LazyVStack(R3/R4 树投影/展开动画依赖自定义布局);中波迁移 List 风险大且 T9 已有自绘左滑先案(TransfersView SwipeableRow)。**裁定:把 SwipeableRow 通用化迁 Common/SwipeableRow.swift**(泛型动作集 + `SwipeActionSpec` 规格:图标/色/无障碍标签出集),传输页两调用点改注规格零行为变化,浏览行接入同组件。实现细节:
- **展开态点行 = 只收动作**:内容 `.disabled(open)` 收口——浏览行内容是整行 Button(ancestor tap 手势竞争不过子 Button,不能靠外挂 tap 拦),禁用后点按落到行面 onTapGesture 只收;闭态零影响(子 Button 照常胜出)。
- drag/onTapGesture 提到 ZStack 层(动作钮区域也可继续拖);`onChange(of: actions)` 保留(选择态翻空集即收)。

## 3. 逐项对账

### 项 #1 首页常用目录选择模式

- **复用裁定(R4 先案延续)**:首页 VM 就是 `BrowseViewModel`(isSelecting/selection/selectedEntries/confirmSelection 全量既有,零新状态、零重复);**根行不可选是结构面**——收件箱根行非 entry,不在 `visibleRows` 内,全选/收集天然不及根(Kit 钉 `testHomeSelectionExcludesRootRow`)。
- **结构重排**:`FavoritesRootList` 收口自己的顶栏(选择态换 `SelectionNavBar`,否则 `BrowseNavBar` 含菜单)——菜单「选择」按钮与 VM 同居一组件,双入口(`enterSelection` 一口)零接线;iOS 26 TabView 的 fallback 分支(bootError/加载)留素顶栏(无可操作项,菜单空)。
- 交互:子行点按=选择态切换选中/常态目录进整页;子行 `LongPressGesture(0.4s)` 进选择(BrowseView 同款);底栏 `SelectionActionBar([.share, .delete])`——分享=先捕获后退出(Ruling O)进 DevicePickerSheet → `SendService.send`(files=收件箱内绝对路径,引擎上传源守卫按授权根收口,与本地浏览页发送同形);删除=捕获路径入居中确认 → `deleteSelected`(退选+原地重载在 VM)。选择态 `.toolbar(.hidden, for: .tabBar)` 在首页生效(**截图钉**:homeselect 帧底部只有动作条,无标签栏)。
- Kit 钉测:`testHomeSelectionExcludesRootRow`(全选不含 rootPath、收集序=行序、确认只退出)。

### 项 #3 设备列表显示本机行

- `DevicesViewModel.project` 去滤(online-first 稳定分区保序,本机按 daemon 序自然落位——在线分区即与在线设备同列);`DevicesRowFormat.subtitle(platform:isSelf:online:)` 纯函数:本机=「平台 · 本机」(徽章即副文),对端=「平台 · 在线/离线」(弹层行不变)。
- 视图:`DevicePickerRow` 增 `isSelf`(默认 false,弹层零改动)——副文走 RowFormat、`.disabled(!online || isSelf)`;DevicesView 按 `deviceId == vm.selfDeviceId` 判定;「本机」不可进=禁点(brief 选 disabled-with-徽章案)。
- `DeviceIcon`(SF 平台图标/平台显示名)自 App SelectionBar **提入 Kit**——本机副文格式化与 App 行渲染同源单份(Kit 无 SwiftUI 依赖,返 SF 名字符串)。
- Kit 钉测:`testLoadKeepsSelfRowAndOrdersOnlineFirst`(本机在列+分区序)、`testSubtitleMarksSelfRow`(四臂:iOS·本机/Windows·在线/macOS·离线/未知·本机)、事件面同不滤(改期望)。
- 已知收敛窗:selfDeviceId 未投影时(AppModel status 晚到)本机行暂按对端样,onChange 即标记——与 T7 时代「先显示后滤除」同类短窗。

### 项 #7 行左滑动作

- **动作集纯函数**(Kit,宿主可测):`BrowseSwipeAction.actions(entry:isRemote:)`——本地文件=[.send,.delete];远端文件=[.download,.forward];**目录=空集不可滑**(两档同)。Kit 钉 `testSwipeActionsByRowType` 四臂。
- 接入:BrowseView/FavoritesRootList 行包 `SwipeableRow`(选择态翻空集不挂手势;首页仅本地集);钮规格 extension(spec):发送/下载/转发蓝(square.and.arrow.up/down)、删除红(trash)——与选择底栏动作语言一致。
- 单条目动作与选择模式**同流**:发送/转发=单条目捕获进 DevicePickerSheet(远端转发 excluding 来源设备);下载=直达本机收件箱(`downloadEntries`,toast 同语言);删除=入 `DeleteRequest` 居中确认 → `deletePaths`。
- Kit:`deleteSelected` 拆出 `deletePaths(paths)`核(device=VM 浏览目标,ctrl delete {device,paths,force:true};失败置 lastActionError;**不退选不重载**——收口归调用方);`deleteSelected` 语义原样(退选+reload)。钉 `testDeletePathsSendsDeleteFrameWithoutSideEffects`(帧形状恰一发+无副作用)。
- DEBUG 截图钩子:`MTDSS_DEBUG_SWIPE=1` 首个可滑行初始展开(`debugOpen`,sim 无滑动手势)。

### 项 #8 选择模式左下垃圾桶(含远端)

- 本地底栏本就有 trash(T6);**远端动作集 下载/转发 → 下载/转发/删除**(trash 红排左簇末位)。
- 远端删除零新路径:`deleteSelected` 按 `vm.deviceId` 收口=ctrl delete **{device=对端 id}**,paths/force 同形——与本机删除同一 VM 面(daemon 侧 T6 已带远端 device 契约)。Kit 钉 `testRemoteDeleteSelectedUsesPeerDeviceId`:删除帧 device=对端、成功后退选、原地重载 list 仍打对端(恰 2 发)。
- 截图:remoteselect 帧=真远端 DESKTOP-VG14605 浏览选择态,底栏 下载/转发/删除(红)在列,标签栏隐藏。

### 项 #9 删除一律确认

- **全 App 删除路径居中 alert 统一**(「删除 N 项?」/「所选项目将被永久删除,不可恢复。」破坏性红钮):浏览选择删除(原 confirmationDialog 底部 sheet→alert)、浏览行滑删(新)、首页选择/滑删删除(新)、信任目录移除(SettingsView confirmationDialog→alert,文案原样)。`confirmationDialog` 全 App 归零(grep 钉)。退出登录确认 R5a #10 先案同款。
- 收口件:`DeleteRequest`(Identifiable,条数入文案)在 BrowseView/FavoritesRootList 共用;**无确认不删**——所有删除执行都在 alert 破坏性钮回调内(DEBUG 钩子只呈现不执行,删除仅在用户点钮后跑)。
- 边界注记:传输列表「清除/清除已完成」=记录清除非文件删除,不在 #9 面内(与 brief 所列路径一致),维持原样未扩。

## 4. 验证矩阵(全绿)

| 面 | 结果 |
|---|---|
| Mac `swift test`(f126c3e) | **155/155 零失败**(R5a 末 151:+1 首页选择 +2 本机行/副文 +1 事件面改期望 +2 滑删/动作集 +1 远端删除;中间 1 失败=钉测伪 daemon 根卷空列表,已修) |
| Mac `ios/build.sh sim` | 绿(引擎静态库 → xcodegen → 构建装机启动;真登录 kk@test.com @ 产线,20 台真目录,真远端浏览 DESKTOP-VG14605 全链) |
| sim 截图 | 五帧 `artifacts/mtdss-r5b-{homeselect,deviceself,swipe,delconfirm,remoteselect}.png` |
| WSL/Rust | 零改动(diff 无 crates/;未跑 cargo——无涉面) |
| 清理 | 截图测试树(find 列证后逐文件删+rmdir,复核 CLEAN-OK);/tmp 脚本移除;worktree 零残留 |

## 5. Concerns(按重要性)

1. **滑动/长按的真机手感未验证**(sim 无触点驱动):左滑吸附(过半 0.4/predicted 阈值,沿 T9 先案参数)、展开态点行只收动作(`.disabled(open)` 收口)、首页子行长按 0.4s 进选择、滑删后列表刷新——均留真机复测。SwipeableRow 手势已从内容层提到 ZStack 层(动作钮区可续拖),传输页行为等价性也属真机确认面。
2. **SwipeableRow 展开态 `disabled(open)` 的可达性**:展开期间行内容 a11y 元素短暂禁用(点行只收)——iOS 惯用行为(Files App 同),未做特判;若 VoiceOver 复测有碍再票。
3. **本机行收敛窗**(selfDeviceId 未投影时暂按对端样,onChange 即标):与 T7 时代同类短窗,未加骨架特判;真机首开若可见「本机未标」闪帧再票。
4. **#1 首页选择的两处次要语义裁定(记录)**:(a) 选择态根行点按=无操作(不导航不勾选,根无圈不进集);(b) 根行展开钮选择态保留可用(展开便于子行勾选);(c) bootError/加载 fallback 页顶栏菜单为空(原 disabled 排序钮不再显示——引擎未就绪无可操作项,选择也无处落)。若用户反馈 fallback 也要菜单形,一行补回。
5. **传输列表「清除」无确认**(记录裁定):#9 面内路径(选择/滑删/信任目录/既有本地删除)全部居中确认;传输记录清除=列表治理非数据删除,brief 未列,维持原样——若用户期望同确认面,加法一致(alert 套壳)易补。
6. **远端删除的 daemon 实证面**:契约 delete {device,paths,force} golden 已钉(ContractGoldenTests),远端 device 形由 Kit 钉测收口;真删远端文件的端到端(ctrl → 对端 core fs)未在 sim 驱动(避免对真设备删文件)——真机复测时用一次性测试文件走全链。
7. **设备目录噪声**(真机无此面):测试账号目录 13 条历史「iOS 模拟器」陈旧条目(sim 反复 erase+login 产物,恒离线)在本轮截图中沉底可见,与 #3 无关;账号治理题外。

## 6. 产物清单

- Kit:`Sources/MTDSSKit/Ui/{BrowseViewModel,BrowseViewModel(滑删/动作集),DevicesViewModel}.swift`(BrowseSwipeAction/deletePaths/DeviceIcon/DevicesRowFormat/project 去滤)
- App:`Browse/{BrowseView,FavoritesView,SelectionBar,DevicePickerSheet}.swift`、`Common/SwipeableRow.swift`(新,自 TransfersView 迁+通用化)、`Devices/DevicesView.swift`、`Profile/SettingsView.swift`
- 测试:`Tests/MTDSSKitTests/{BrowseViewModelTests(+5),DevicesViewModelTests(改 2 增 1)}.swift`
- 文档:`docs/superpowers/specs/2026-10-31-mtdss-v2p5-ios-design.md`(§4.1 行左滑 / §4.2 首页选择+远端删除+删除一律确认 / §4.4 本机在列)+ progress 状态行
- 截图:`artifacts/mtdss-r5b-{homeselect,deviceself,swipe,delconfirm,remoteselect}.png`
