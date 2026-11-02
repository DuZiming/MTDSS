# T13b R6 功能波报告(#2 推入页选择标签栏不藏 / #3 行左滑真机死 / #4 分享扩展即时反馈+自动拉起)

状态:**三项全交付,Kit 182/182 全绿 + sim 构建绿装机 + XCUITest 3/3 绿(真合成触点)+ 六帧截图** / 基线 2a18201 → HEAD(docs 收尾前 = 1719e80,15 commits)
执行日期:2026-11-03。执行面:Windows(编排+自审)+ Mac mini(swift test + build.sh sim + xcodebuild UITest + 截图)。**WSL/Rust 零改动**(diff 仅 ios/ 与 docs/;传输契约无涉)。

## 1. Commits(顺序)

| # | commit | 内容 | 项 |
|---|---|---|---|
| 1 | `7b5a9ef` | fix(ios): 选择态标签栏 .tabBar 可见性改条件挂载(推入页/首页根页/图库网格三处) | #2 |
| 2 | `0f24bf9` | fix(ios): 浏览行容器迁 List 原生 swipeActions + EdgeBackStrip 左缘返回带 | #3 |
| 3 | `4427559` | fix(ios): BrowseList 行级修饰挂 Group(容器层不下传,sim 截图钉) | #3 |
| 4 | `608ef2d`/`814aa73`/`6291e03`/`9c729fd`/`78b7dae`/`c4f1a57` | test(ios): BrowseListUITests(合成触点)+ 行查询宽匹配 + 引擎 home 三连修(终=scheme 注入) | #3 验证 |
| 5 | `09db699` | chore(ios): 引擎 boot os_log 诊断(DEBUG) | 排障面 |
| 6 | `094460e` | fix(ios): DEBUG 截图钩子恰一次闸(.task 重入二跑收树) | 截图面 |
| 7 | `f88de81`/`23df51d` | feat(ios): R6 #4 分享扩展全链(Kit SharePickFlow + 面板三态 + responder-chain + os_log) | #4 |
| 8 | `130354a`/`1719e80` | test+docs: 钮序帧断言(删除贴缘实测)+ spec 三处 + 钮序注正 | #3/docs |

(本条)docs: 本报告 + progress 状态行 + 六帧截图。

## 2. 逐项对账(含根因)

### 项 #2 推入页选择不隐藏标签栏 —— **根因:iOS 26 同栈多页同声明 .tabBar 可见性时,根页声明压过推入页声明**

**实证链**:基线 build 以 `MTDSS_DEBUG_AUTOPUSH=inbox + SELECT=1` 在 iOS 26 sim **复现**(mtdss-r6-pre-tabbar.png:首页推入页选择态,选择顶栏+底栏在场而标签栏胶囊同屏可见)。结构对账:设备标签推入页与首页推入页的推入链**逐字节同构**(NavigationStack 直挂 tab 内容 / navigationDestination 挂根内容 / 同一 BrowseView)——唯一结构差 = **首页根页(FavoritesRootList)恒挂 `.toolbar(isSelecting ? .hidden : .visible, for: .tabBar)`(R5b 写法),设备根页从不声明**。R5b 两个截图钉(homeselect=根页自身选择 / remoteselect=设备推入页选择)都恰好是「单声明者」形态,首页推入页(根页 `.visible` + 推入页 `.hidden` 双声明)从未被截图覆盖。→ iOS 26 双声明解析取**根页声明**,推入页 `.hidden` 被根页 `.visible` 压住。

**修**:`tabBarHiddenWhenSelecting(_:)`(SelectionBar.swift)条件挂载——仅选择态挂 `.toolbar(.hidden, for: .tabBar)`,非选择态**零声明**(缺省即可见)。三处同改:BrowseView(推入页)、FavoritesRootList(首页根页)、PhotoGridView(图库网格,同受首页根页竞争)。修后任意时刻栈内至多一个声明者。**验证**:同钩子组合 sim 截图(mtdss-r6-tabbar.png)= 推入页选择态标签栏消失;首页根页选择(mtdss-r6-home.png)标签栏隐藏保持;设备推入页(单声明形态)不受影响。

### 项 #3 行左滑真机无反应 —— **根因:自绘 SwipeableRow 的 DragGesture 与滚动容器 pan 争用,真机恒败;迁 List 原生 swipeActions**

**修**(brief 钦定 List 路线,无硬冲突未走回退):
- **`BrowseList`**(Common/ListRows.swift):浏览页/首页根列表容器自 SinkingFooterList(ScrollView+LazyVStack)迁 List;.plain + 行背景透明 + 系统分隔线隐藏(FullBleedRow 自绘 0.5 细线行首内缩不变)+ 行 insets 归零。**行级修饰必须挂 Group**(Group 透明逐行落;挂 List 层不下传——首版双分隔线+默认内缩,sim 截图钉,`4427559`)。**有意取舍**:「N 项」沉底尾注=普通末行(List 无「不足一屏沉底」平价实现;长列表无差)。
- **`NativeSwipeActions`**:动作集空不挂(选择态翻空集即收,SwipeableRow 语义同形);allowsFullSwipe 关;**reversed 声明=删除贴缘**(iOS 26 实测**首声明者贴缘**,UITest 帧断言 `delete.frame.maxX > send.frame.maxX` 钉)。动作语义/收口零变化(handleSwipe/deleteRequest 同一口)。
- **`EdgeBackStrip` 左缘返回带**:List(UICollectionView 背书)化后页级 `simultaneousGesture(DragGesture)` 不保证达——返回带独立于列表(普通视图无争用,DragGesture 恒达),`BrowseGesture.isBack` 谓词/语义不变(历史内 goBack / 浏览根 popToRoot;选择态不挂带)。代价:左缘 30pt 带内起手的纵向滚动不达列表(系统边缘手势同形态)。
- **范围**:仅 BrowseView + 首页根列表;传输页(无滚动争用面)维持 SwipeableRow,设备/传输/设置/我的列表不动。性能:LazyVStack→List 大目录持平偏优(复用池),可接受。
- **真触点验证(XCUITest,sim 合成触点=真事件系统)**:`BrowseListUITests` 2 例——①行左滑→删除钮现→点按→「删除 1 项?」确认弹窗现→取消(全链触达零副作用);②左缘起手右拖 130pt→弹回首页(照片图库行在场断言)。**两例绿=原生 swipe 与边缘返回在 List 上共存实证**。长按 0.4s 进选择/滑删吸附手感留真机轮(历史遗留面)。

### 项 #4 分享扩展 —— 即时反馈 + 自动拉起 + 自动发送

- **a. 点选即忙(Kit 状态机)**:`SharePickFlow`(MTDSSKit/Share,9 例钉测):idle→pick→busy(device)(行内 spinner 换状态点+整表禁点,3s open() 竞窗全程可见)→ openSettled → launched(open 通或绕法通;绿勾「已发送」0.6s 停留后 completeRequest)/ queued(注记即刻呈现);enqueueFailed→failed;queued/failed 均可换设备重选。行三态渲染在 **`SharePanelParts.swift`(单文件双目标编译)**,主 App DEBUG 状态页(`MTDSS_DEBUG_SHARESTATES=1`+TAB=transfers)同源截图(mtdss-r6-sharestates.png:idle 绿点/busy spinner/launched 绿勾+已发送/离线置灰+降级注记)。
- **b. 两级拉起**:① `ctx.open` 3s 竞窗(T11 原案);② **responder-chain 绕法**(spec §4.7 钦定备选):仅 open 回话 false/超时后走,自 principal VC 沿链命中响应 `openURL:` 的对象即发(@objc 协议转发,不直引 UIApplication——NS_EXTENSION_UNAVAILABLE 编译门);**审核风险注入码**(ShareViewController 类型头)。双败=queued 注记。
- **c. 自动发送**:消费链(boot/event.login/前台三触发)本就存在;sim 实证 **App Group 队列项被冷启动消费**(直写 App Group 的 ready 项 → 冷启动 → 队列排空 + `share-consume` os_log:`consume begin`/`consume done consumed=1`)。**新发现(留真机轮)**:sim 中外部源 custom-scheme 打开宿主被系统确认弹窗(「在"MTDSS"中打开?」)拦一道(sharestates 截图同帧捕获)——扩展发起的 open 是否同样被拦、真机 dialog 形态,真机轮以新 os_log 观察。
- **排障面**:扩展全程 os_log(subsystem `top.mtdss.ios.share`,panel/vc 两类:load/pick/enqueue/ctx.open/绕法/queued/launched/finish);主 App 消费环(`top.mtdss.ios`/`share-consume`:begin/done);引擎 boot(`top.mtdss.ios`/`engine`:home 入参+成败——本轮排障主力,真机轮通用)。

## 3. 验证矩阵(全绿)

| 面 | 结果 |
|---|---|
| Mac `swift test` | **182/182 零失败**(R5c 末 173 + 9 SharePickFlow) |
| Mac sim 构建(app+extension) | 绿(xcodegen regenerate 后;warnings 皆历史存量) |
| XCUITest | **3/3 绿**(BrowseList 2 + GrantPhotos 1;真合成触点) |
| sim 截图 | 六帧:pre-tabbar(复现钉)/tabbar(修后)/list/listselect/home/sharestates |
| 消费链 | App Group 直写 ready 项 → 冷启动消费排空(os_log 钉) |
| WSL/Rust | 零改动(diff 无 crates/) |

## 4. Concerns(按重要性)

1. **真机复测面**:#3 滑动吸附/长按 0.4s 手感、#4 扩展三态真机观感、responder-chain 真拉起效果(openURL: Bool 返回真≠系统一定切 App)、扩展 open 是否触发系统确认弹窗、#2 修在真机的动画形态——均以新 os_log + 截图逐项核。
2. **左缘返回带 30pt 死区**:带内起手纵向滚动不达列表(垂直拖被 DragGesture 认领)。Files App 边缘手势同形态;若真机反馈碍事,候选=UIGestureRecognizer 自定义(只认横向)再票。
3. **沉底尾注取舍**:List 化后「N 项」短列表不再贴视口底(随内容)。mockup 像素口径让位原生左滑;若用户要沉底,候选=safeAreaInset 条件挂(仅短列表)再票。
4. **sim 引擎 home 注入链脆弱**(本轮实测三连坑):sim 进程 HOME/getpwuid/NSUserName 皆沙箱定向,xcodebuild 环境不继承——UITest 引擎 home 终以 **scheme environmentVariables(UITEST_MTDSS_HOME=/Users/kk/.mtdss-ios-sim)** 注入,**换机须同步 project.yml**。手测一律 `SIMCTL_CHILD_MTDSS_HOME=~/.mtdss-ios-sim`(build.sh 缺省),漏带=容器长路径 sun_path 104 bind 失败(bootError helloTimeout,os_log engine 类可一眼判)。
5. **DEBUG 钩子恰一次闸**(.task 生命周期重入):产品路径本幂等(loadRoot/showHidden 有守卫),仅截图钩子二跑会翻态;闸挂 BrowseView/FavoritesRootList 两处。
6. **App Group 队列直写测试残留**:验证用 ready 项已清(uitest2 手删/其余被消费排空);设备缓存 devices.json 留存无害。

## 5. 产物清单

- Kit:`Sources/MTDSSKit/Share/SharePickFlow.swift`(新)+ `Tests/MTDSSKitTests/SharePickFlowTests.swift`(9 例)
- App:`Common/ListRows.swift`(新:BrowseList/NativeSwipeActions/EdgeBackStrip)、`Browse/{BrowseView,FavoritesView,SelectionBar}.swift`、`Photo/PhotoGridView.swift`、`MTDSSShare/{SharePanelParts(新),SharePanelView,ShareViewController}.swift`、`ShareQueue/ShareConsumer.swift`、`Debug/ShareStatesHarness.swift`(新,DEBUG)、`EngineHost.swift`、`AppTab.swift`、`project.yml`(共享件双目标+UITest scheme env)
- 测试:`UITests/BrowseListUITests.swift`(新,2 例真触点)
- 文档:spec §4.1/§4.2/§4.7 三处 + progress 状态行
- 截图:`artifacts/mtdss-r6-{pre-tabbar,tabbar,list,listselect,home,sharestates}.png`
