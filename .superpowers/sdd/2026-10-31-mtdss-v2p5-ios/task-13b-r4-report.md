# T13b R4 交互波报告(常用目录行内展开 / 设备行去箭头 / 选圈钉缘动画 / 下拉刷新 / 远端标题组合)

状态:**五项全交付 + §7 回炉修正,Kit 147/147 全绿 + sim 构建绿 + 四帧截图** / 基线 3a04688 → HEAD = b8cbdfe + docs,9 commits
执行日期:2026-11-02。执行面:Windows(编排+自审)+ Mac mini(swift test + build.sh sim + 截图)。**WSL/Rust 零改动**(diff 仅 ios/ 与 docs/,crates 无涉)。

## 1. Commits(顺序)

| # | commit | 项 |
|---|---|---|
| 1 | `55d4b41` feat(ios): 设备行去蓝箭头(R4 #2):DevicePickerRow 只留图标/名称/副文/状态点,设备页与弹层同组件一致 | #2 |
| 2 | `095458d` feat(ios): 远端浏览标题=设备名\|目录名(R4 #5):composeTitle 纯函数+deviceName 注入,本地单段不变,顶栏中间截断 | #5 |
| 3 | `c391ad9` feat(ios): 选圈钉行首固定缘(R4 #3):固定列宽全行一致,进出选择圈左移淡入+内容右移让位,与树展开动画分值 scoped 不串扰 | #3 |
| 4 | `65c5e0c` feat(ios): 常用目录根行行内展开(R4 #1):BrowseViewModel 全量复用为首页 VM,根行箭头展开/收起 collapseAll 世代收口,子行点按直入子目录整页(openOnAppear) | #1 |
| 5 | `6145f2c` feat(ios): 下拉刷新(R4 #4):favorites/devices/browse 三面 .refreshable 接既有 reload/load 单口,展开态原地重载语义保持;钉 reload 恰一发当前目录测试 | #4 |
| 6 | `7fa7b51` fix(ios): FavoritesRootList 的 BrowseView 实参顺序对齐成员序(编译修) | 编译修 |
| 7 | `55df3ec` fix(ios): FAVTREE 截图钩子等待口径去 !loading(DEBUG 钩子修,非产线码) | 截图修 |
| 8 | (本条)docs: 本报告 + spec 四处修订 + sim 截图四帧 | 收尾 |

逐项提交;BrowseView 跨 #3/#4/#1 三项——按项拆分编辑,最终态与逐项叠加态一致。

## 2. 逐项对账

### 项 1(R4 #1)常用目录根行行内展开

- **复用裁定(记录)**:**不另起 FavoritesViewModel**——首页 VM 就是 `BrowseViewModel`(浏览根 = 收件箱路径;根列表 = `entries`;懒加载缓存/世代守卫/缩进/排序管线全量单源,与浏览页同一实现)。根行本体不是 entry,其**展开态 = 页面侧一个 bool**——数据早已随根拉取就位(进页 `loadRoot()` 一次,本地收件箱一次轻 list),展开/收起 = 状态翻转零拉取;**收起走 `collapseAll()`**:世代 +1 作废在飞子项拉取、子树缓存释放(R3 评审 I-1 同款收口——收起不得留迟到应答复活面),Kit 钉 `testHomeRootCollapseReleasesSubtreeAndRefetchesOnReExpand`(收起平面/根列表不动/再展开重拉恰 3 次 list)。
- **View**:根行 chevron = 展开钮(› 展开 ∨ 旋转,触区外扩 28pt 负 padding 还原,同 BrowseEntryRow 手法),行点按仍 = 进整页;子级 = `BrowseEntryRow` 复用(`depth + 1`,根行占 depth 0;首页无选择面恒 selecting=false);子级目录行箭头 = 同一 `toggleExpand`(懒加载/世代/缩进整链);动画 = `inboxExpanded` 与 `expandedPaths` 两条值 scoped `.animation`。
- **子行点按直入子目录整页**:`BrowseView` 新增 `openOnAppear`(在场则开页 `open(子目录)`,单一开页入口防两处并发首拉竞窗)——历史 = [根, 子],返回/前进同浏览页;`FavoritesRoute` 值路由(.root/.sub)承载。
- 搜索收起:首页无搜索框(题面允许跳过)——子级目录行内展开的搜索收起语义随浏览页,不受本项影响。
- 进页拉根失败不打断首页:展开后行内「无法加载目录 + 重试」小行(reload 承担)。
- 截图:`mtdss-r4-favtree.png`(收件箱 ∨ 展开 → 01 工作区(depth1,∨)→ 内层(depth2,›),逐级缩进,副文齐)。

### 项 2(R4 #2)设备行去蓝箭头

- `DevicePickerRow` 删 › 图标(设备页列表 + 选设备弹层**同组件**,一处改两面一致);行 = 平台图标 + 名称 + 「平台 · 在线/离线」+ 右侧状态点;按压反馈(PressableRowStyle)/离线置灰禁点/整行触区全保留。可点性不再靠箭头暗示(弹层行同形,题面裁定一致)。
- 截图:`mtdss-r4-devicerow.png`(设备页 19 台真目录装载:在线 4 台在前,离线灰显沉底,无 ›)。

### 项 3(R4 #3)选圈钉行首固定缘 + 滑入动画

- 结构:行内容从「HStack(if selecting { 圈 } …)padding(leading, indent)+horizontal(20)」改为 **`HStack(spacing:0){ 选圈列; 内容 HStack.padding(leading, indent) }.padding(trailing, 20)`**——选圈列固定宽:**非选择态 20**(恰原行首内缩,非选择布局与 mockup 逐像素同)、**选择态 56**(20 内缩 + 24 圈 + 12 间距),圈 `padding(.leading, 20)` 钉行首固定缘。
- 全行统一列宽 → 展开子级行(depth 0/1/2)选圈同一 x,无随缩进漂移、无跳面;内容保持原缩进,选择时整体右移 36pt 让位(iOS 文件 App 手感)。
- 动画:圈 `.transition(.opacity + .offset(x: -12))` 左移淡入/出;列宽展开收合同拍;列表 `.animation(value: vm.isSelecting)` 与既有 `.animation(value: vm.expandedPaths)` **各自值 scoped,互不串扰**(树展开动画不吃选择翻转面)。
- 截图:`mtdss-r4-select-circle.png`(展开树选择:01 工作区#0/内层#1/Archive#2 三行勾选,圈同一 x 钉列,内容缩进保持;文件行同样有圈——列全行一致)。

### 项 4(R4 #4)下拉刷新

- 三面 `.refreshable`:**浏览页**(本地+远端同模板)= `vm.reload()`(原地重载:不动历史,展开态保持、子缓存逐个现拉刷新——R3 reload 契约直接复用);**设备列表** = `vm.load()`(与「···」刷新同口);**常用目录** = `model.refreshStatus()`(根数据源)+ `vm.reload()`(收件箱列表/展开子级同收口)。
- VM 面钉 `testReloadRefetchesCurrentDirectoryFrame`(刷新恰一发当前目录 list、数据面翻新);设备面既有 `testRefreshRepullsDevicesFrame`。
- sim 无拖拽触点驱动,刷新指示器视觉面留真机(见 Concerns)。

### 项 5(R4 #5)远端浏览标题 = 设备名|目录名

- Kit:`BrowseViewModel.composeTitle(rootTitle:path:rootPath:deviceName:)` 纯函数(宿主可测)——根 = rootTitle 单段(远端 = 设备名);深于根:deviceName 在场 = 「设备名|目录名」(远端 Windows 反斜杠同切末段),deviceName nil(本地收件箱)保持原单段口径。VM 存 `deviceName`(`init` 尾参缺省 nil,既有调用点零改动);`RemoteBrowseDestination` 传 `target.name ?? target.deviceId`。
- 顶栏 `BrowseNavBar` 标题 `.truncationMode(.middle)`(长标题中间截断,头尾都可见)。历史/返回前进零改动。
- 测试:纯函数七臂 + `testRemoteVMTitleSwitchesWithDevicePrefix`(根单段/进目录双段/返回恢复);既有本地标题钉面(`testBackForwardHistoryAndTitleSwitch`)零改动通过 = 本地口径未动。
- 截图:`mtdss-r4-title.png`(真远端:DESKTOP-VG14605 浏览 C:\Users,标题「DESKTOP-...14605|Users」中间截断,分隔符 | 在)。

## 3. 验证矩阵(全绿)

| 面 | 结果 |
|---|---|
| Mac `swift test`(55df3ec) | **145/145 零失败**(R3 末 141:+2 标题 +1 首页收口 +1 刷新恰一发) |
| Mac `ios/build.sh sim`(MTDSS_SIM_KEEP=1) | 绿(引擎静态库 → xcodegen → 构建 → 安装 → 启动;真实引擎装载,device="self" 本机 + 真远端浏览全链) |
| sim 截图 | 四帧 `artifacts/mtdss-r4-{favtree,devicerow,select-circle,title}.png` |
| WSL/Rust | 零改动(diff 无 crates/;未跑 cargo——无涉面) |

## 4. Concerns(按重要性)

1. **三面动画/刷新的真机手感未验证**(sim 无触点/拖拽驱动):选圈滑入+内容右移的动效、下拉刷新指示器与展开态并存的手感,截图只证静态对齐(钉缘/缩进/列宽),动效与刷新交互留下一轮真机验证。
2. **首页进页即拉一次收件箱 list**(本地 UDS,轻):换取根行展开零等待;失败不打断首页(行内重试)。若真机反馈「进页不该有拉取」可改回懒拉(展开首拍触发,VM 已有 loading/error 面)。
3. **首页根行展开态是视图侧 bool(非 Kit 状态)**:裁定记录——根行非 entry、数据随根拉取就位,收起走 `collapseAll()` 世代收口已钉 Kit;若后续首页出现搜索/多根(Ruling N 翻案),把 bool 提进 VM 再议。
4. **chevron 嵌行 Button 的点按分流**(首页根行同 R3 浏览行先案):内层 Button 胜出是 SwiftUI 标准模式,R3 已报「sim 无触点驱动,点按分流留真机」——本波首页根行同面,真机若双触发同修法(分区手势/提出行外)。
5. **标题分隔符 = 半宽 `|`**(题面「[设备名]|[目录名]」字面);设备名取目录投影 name(name 缺省回退 deviceId——远端目录名缺 name 时标题可读性降级,既有数据面)。
6. ~~子行直入整页历史=[根,子],顶栏返回回收件箱页~~ → **已回炉(§7 用户裁定)**:单目的直入,子页以子目录为浏览根,返回直达首页;首页展开现场原样保留。
7. **截图噪声与排障记录**(真机无此面):dotfiles 可见 = sim UserDefaults「显示隐藏项」ON(T10 残留);通知授权弹窗系 sim 陈年 pending 提示,**simctl reboot 清除**(本波起截图前先 reboot);**引擎 inbox 平铺 home 口径** = 覆盖 `MTDSS_HOME` 时 recv_dir = `{home 父}/Documents` = Mac ~/Documents(EngineLayout 派生)——测试树须建在那,不在 App 容器 Documents(首轮误建已清)。`SIMCTL_CHILD_MTDSS_HOME` 直接 relaunch 时必须带(容器路径超 sun_path 104 → helloTimeout,build.sh 档自动注入)。
8. **FAVTREE 钩子等待循环 bug**(55df3ec 修):`!vm.loading` 条件在 loading 翻转(先于应答)时提前出循环——对齐 BrowseView `waitForEntries` 口径(只看 entries/error)。DEBUG 钩子面,产线不编译。

## 5. 产物清单

- 代码:`ios/app/MTDSS/Browse/{BrowseView,BrowseNav,DevicePickerSheet,FavoritesView}.swift`、`ios/app/MTDSS/Devices/DevicesView.swift`
- Kit:`ios/package/Sources/MTDSSKit/Ui/BrowseViewModel.swift`(composeTitle/deviceName)
- 测试:`ios/package/Tests/MTDSSKitTests/BrowseViewModelTests.swift`(4 新钉)
- 文档:`docs/superpowers/specs/2026-10-31-mtdss-v2p5-ios-design.md`(§4.1 常用目录展开+下拉刷新 / §4.2 选圈钉缘 / §4.3 弹层行无箭头 / §4.4 设备行无箭头+标题格式)
- 截图:`artifacts/mtdss-r4-{favtree,devicerow,select-circle,title}.png`

## 7. 回炉修正(2026-11-02 用户裁定:子行返回直达首页)

**裁定**:首页展开树点子目录进整页后,按返回必须**直达首页标签页且展开现场原样保留**——不得落在中间收件箱浏览页。此裁定同时消解评审 Minor(原 `[.root, .sub]` 双推入的中间页冗余 loadRoot 拉取,FavoritesView.swift:71)。

**实现**(commit `b8cbdfe`):
- **单目的直入**:`onOpenPath` 只推 `.sub`(原 `[.root(收件箱), .sub(子路径)]` 双推入废止);`BrowseDestination` 于 `initialPath` 在场时 **VM 以子目录为浏览根**(`rootPath = initialPath`,`rootTitle = 子目录名`——进页 path==rootPath → canGoBack=false → 顶栏返回/边缘右滑 = `onPopToRoot` = 弹回首页;标题 = 子目录名,与原深级口径一致);页内再进 = 正常页内历史(open 截断追加,返回收口在子目录根)。
- `openOnAppear` 机制随之中止删(无消费方);`BrowseView` .task 还原单一 loadRoot 入口。
- **展开态保留(结构面)**:`FavoritesRootList`(持 `@StateObject vm: BrowseViewModel` + `@State inboxExpanded`)挂在首页 NavigationStack 根——推入期间根页保持挂载、视图身份稳定(同一 `rootList` 分支),@StateObject/@State 随身份存活,弹回即原样;复现于 `.task` 重触发也被 `entries.isEmpty` 守卫吞掉(不重拉)。 Kit 层钉「无交集」契约双测:
  - `testChildRootedVMStartsFreshAtChild`:子页 VM 进页 canGoBack=false、标题=子目录名;open 更深 → canGoBack=true → goBack 回子目录根再收口(页内历史正常);
  - `testHomeExpansionUnaffectedByChildPageNavigation`:首页/子页独立 VM,子页 load/open/back 全程后,首页展开集原样、可见行原样、**首页连接零新帧**(弹回不重拉)。
- spec §4.1 常用目录行同步改(「单目的直入…返回直达首页,展开现场原样保留」)。

**验证**:Mac `swift test` **147/147 零失败**(145 + 2 新钉);`ios/build.sh sim`(KEEP=1)绿;首页 favtree 复检截图无回归(展开树照常渲染)。真机面(返回手势手感/弹回动画)留下一轮真机验证。
