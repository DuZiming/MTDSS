# T13b R9 功能波报告(20261103.1 设备轮三项:#1 滑风格统一原生 / #2 设备页面行左滑(解释性) / #3 远端删除设置门)

状态:**三项全交付,Kit 184/184 全绿 + sim 构建绿 + XCUITest 11/11 全绿零 skip(本支首次,含两条远端触点钉复活)+ 五帧截图** / 基线 474e137 → HEAD(代码+测试修正 6 commits,docs 收尾另计)
执行日期:2026-11-03。执行面:Windows(编排+自审+对端 dev daemon)+ Mac mini(swift test + build.sh app + xcodebuild UITest + 截图)。**WSL/Rust 零改动**(diff 仅 ios/ 与 docs/,传输契约无涉)。

## 1. Commits(顺序)

| # | commit | 内容 | 项 |
|---|---|---|---|
| 1 | `38f47d1` | feat(ios): 传输列表迁 List 原生 swipeActions(SwipeableRow 撤除,SwipeActionSpec 迁 ListRows;TransfersListUITests 全链钉) | #1 |
| 2 | `32f159f` | docs(ios): 设备行无左滑裁定注记(DevicesView 头) | #2 |
| 3 | `75d7116` | feat(ios): 远端删除 App 侧设置门(Kit 动作集单源双表面 + AppModel 持久开关 + 设置页开关 + 浏览/选择消费 + 门双臂 UITest) | #3 |
| 4 | `f07c7d9` | test(ios): 远端滑序钉测对端 id 重钉 44d97f96(原 c87f79e0 home 已不存在) | 验证面 |
| 5 | `9dc3216` | fix(ios-tests): 远端触点测激活设备标签(AUTOPUSH device 钩子挂 DevicesView,非本标签不跑) | 验证面 |
| 6 | `3b3efad` | fix(ios-tests): 远端滑序期望钉正「转发贴缘」(M-4 原期望笔误,首对活对端实证即纠正) | 验证面 |
| 7 | `748a09a` | docs(spec): §4.1/§4.2/§4.4/§4.5/§4.6 五处随修 + 六帧截图 | docs |
| (本条) | docs: 本报告 + progress 状态行 | 收尾 |

## 2. 逐项对账

### 项 #1 滑风格统一原生(传输列表迁 List)——**SwipeableRow 全 App 撤除**

- **迁移**:TransfersRootView 列表体 `SinkingFooterList{...}footer:{...}` → `BrowseList{...; ItemCountFooter(...)}`(R6 浏览页同容器同法:行级修饰 Group 挂载、.plain 通栏、行背景透明、系统分隔线隐藏、FullBleedRow 自绘 0.5 细线不变);TransferRowView/ShareFailureRowView 自绘 `SwipeableRow` 包装 → 裸 FullBleedRow 内容 + `.modifier(NativeSwipeActions(...))`。
- **保留面逐一**:行内容布局逐字节不变(徽章/名/方向/状态/进度条/百分比/绿勾;进度条在 List 行内渲染正常,截图钉);最新在最上(VM 面未动);「N 项」尾注 = List 普通末行(= R6 BrowseList 沉底取舍,传输页同政策);状态动作集 `TransfersFormat.actions` 未动(进行中[取消]/中断·失败[重新发起,清除]/完成·已取消[清除]/空集不示能——会话终态与同签名在途抑制臂同语义,R5b Ruling K 钉测原样);动作钮规格(swipeSpec)与语义收口(onAction → VM cancel/reinitiate/clear)零变化。
- **撤除**:`Common/SwipeableRow.swift` 整文件删(末位消费者随迁);`SwipeActionSpec` 迁 `Common/ListRows.swift` 承袭(浏览/传输 swipeSpec 出口不变);全仓 grep 实证仅历史注释提及(记录迁移动因,有意保留)。
- **视觉增量声明**:①原自绘行「动作在途钮半透明」不迁——原生 swipeActions 无此挂点,**防连点语义不变**(VM `control()` busy 重入守卫在);②短列表尾注不再沉底(R6 既有取舍,传输页现同);③传输页原无下拉刷新(前台回归+事件增量承担对账),迁移不加新面。
- **UITest 钉**(新 `TransfersListUITests`,R6 合成触点同法):`testTransferRowNativeSwipeRevealsClearAndFires`——`MTDSS_DEBUG_TAB=transfers`+`MTDSS_DEBUG_SKIPROW=1` 合成 done 行(动作集恰[清除])→ 左滑「清除」钮现(原生 swipe 在传输行生效)→ 点按 → VM clear → 引擎对合成 id 拒「任务不存在」→ 页内错误横幅现(滑动→钮→动作链全触达,零真任务副作用)。**绿(9.4s)**。

### 项 #2 设备页面行左滑(解释性项)——**解释裁定:远端浏览行已有且本轮实证;设备列表行按裁定不挂滑面**

**解读**:用户 #2「设备页面,设备里面,也要支持行左滑操作」两个候选所指:①「设备」标签推入某设备后的**远端浏览行**——R5b 动作集(远端[下载,转发])+ R6 List 原生 swipe 已在;②「设备」标签根页的**设备列表行**(每行=一台设备)——从无滑面。**证据与结论**:

- **①已验(本轮实测强化)**:远端触点此前在本环境恒 skip,本轮起两条远端钉测对活对端跑绿——`testRemoteRowSwipeOrder`(滑序)+ `testRemoteRowSwipeDeleteGate`(门双臂,#3)全绿=远端浏览行原生左滑真事件全链实证;`artifacts/mtdss-r9-remote-inbox.png` = 对端 inbox 浏览页实照(页内文件行均挂原生滑面)。设备轮可直接向用户演示。
- **②不做的依据(与任务预置结论一致,已落码注)**:设备行唯一动作 = 点按进浏览(在线;本机/离线行禁点);**无任何按设备粒度的第二动作**——无单设备刷新(devices 仅整表 RPC)、无改名(登录实名契约面)、无解除配对(服务端账目面,客户端无此 API);行左滑「浏览」只重复点按。iOS 文件 App「位置」行同形态(无滑面)。有动作的是推入后的浏览行(①)。裁定注记落 `DevicesView.swift` 头(`32f159f`),`mtdss-r9-devices.png` = 设备列表行实照(在线/离线/本机行,无滑示能)。
- **更优动作否决记录**:未找到廉价有意义的按设备动作——[连接]/[刷新]不存在对应契约面;[发送]无内容选择语义不成立。若用户明确要,「解除配对/改名」需先服务端契约增量,非 App 单侧可做。

**待控制者向用户复认**:若用户所指为①,答复「已有,本轮已实证+截图」;若坚持②,上述为不做的依据(或回炉需新契约面)。

### 项 #3 设置「允许删除远端文件」门(R9 #3)——**两表面单源,默认关=今日集**

- **Kit(动作集单源,宿主可测)**:`BrowseSwipeAction.actions(entry:isRemote:allowRemoteDelete:)` 增门参(缺省 false=今日集);远端文件行门开=[下载, 转发, 删除](簇末=原生贴缘),门关=[下载, 转发];本地=[发送, 删除]恒不设门;目录行恒空集。新 `BrowseSelectionAction.actions(isRemote:allowRemoteDelete:)`(选择底栏出集,与滑面同一门):本地 [分享,删除] 两态同;远端门关 [下载,转发] / 门开 [下载,转发,删除]。钉测 +4/+1 两测(`testSwipeActionsByRowType` 扩门臂 + `testSelectionActionsByRemoteDeleteGate` 四臂)。
- **App**:`AppModel.allowRemoteDelete`(UserDefaults `mtdss.browse.allowRemoteDelete`,默认关,didSet 持久;DEBUG 钩子 `MTDSS_DEBUG_ALLOWREMOTEDELETE` **0/1 双态生效**——门要钉两态,持久开关须显式清,与 showHidden 单边钩子形态不同,注记在案);设置页开关(「允许删除远端文件」/「开启后,浏览其他设备时可对文件执行删除」,与显示隐藏项同段同形,a11y id `allow-remote-delete-toggle`);BrowseView 行滑/选择底栏 + 首页子行/底栏全部改走 Kit 单源(`SelectionActionBar.ActionKind` 增 Kit 映射 init)。**门只收 App UI 面**(远端滑删/选择删除的显隐);引擎 ctrl delete 契约不变;删除确认弹窗(R5b #9)原样覆盖新入口。
- **截图两态**:`mtdss-r9-settings-off.png`(默认关)/ `mtdss-r9-settings-on.png`(开,绿档)。
- **UITest 门双臂(活对端跑绿)**:`testRemoteRowSwipeDeleteGate`——门开臂左滑「删除」现且帧断言贴缘;门关臂回今日集(转发贴缘)+「删除」钮不现;**门开臂在前、门关臂收尾置回 0**(门钩子直写持久开关,收尾复位保 M-4 前置假设)。

### 验证面两处既有测试债修正(本轮跑通远端触点连带发现,R6 起 skip 真因全解)

1. **对端 id 失效**(`f07c7d9`):远端钉测原钉 c87f79e0(kk-compute),其 home 已不存在——R6 起「环境 skip」根因之一。现钉 44d97f96 = `C:\mtdss-t6a` dev daemon(R7 同款产线信令真登录;`MTDSS_HOME=C:\mtdss-t6a mtdssd` 一条命令即起,session.json 取 id)。
2. **设备标签未激活**(`9dc3216`):AUTOPUSH device 钩子挂 `DevicesView.task`,TabView 惰性挂载下非本标签不 appear 钩子不跑——远端测未注入 `MTDSS_DEBUG_TAB=devices`,**R6 起 M-4 远端滑序 skip 的另一半根因(此前只疑对端可达性)**。
3. **M-4 期望笔误**(`3b3efad`):原期望远端 [下载,转发]「下载贴缘」,从未实证(恒 skip)。实跑钉正:**动作集末位恒贴滑出簇最右**(NativeSwipeActions reversed 声明=首声明者贴缘,即数组末位)——本地删除贴缘、远端**转发**贴缘、门开删除贴缘同律;与 R5b 自绘行 HStack 实际渲染(转发贴缘)一致,R6 注记「R5b 远端动作集序=下载贴缘」系转写笔误,spec 随修。

## 3. 验证矩阵(全绿)

| 面 | 结果 |
|---|---|
| Mac `swift test` | **184/184 零失败**(R8 末 183 + 门出集新测 1;含 #3 门臂 +4 断言) |
| Mac sim 构建(`build.sh app`) | 绿(xcodegen 重生成含新 UITest 文件与撤除文件) |
| XCUITest | **11/11 全绿零 skip**(BrowseList 9 含远端滑序+门双臂、TransfersList 1、GrantPhotos 1;远端两例对活对端 44d97f96 直连,daemon 日志 relay=false 直连实证) |
| sim 截图 | 五帧:`mtdss-r9-{transfers,devices,remote-inbox,settings-off,settings-on}.png` |
| WSL/Rust | 零改动(diff 无 crates/) |
| 清场 | dev daemon(44d97f96)已杀停(临时进程,本轮起用);sim 门开关联钩子复位 0;Windows 临时文件(诊断截图/daemon 日志)已删;产线 57700 未触碰 |

## 4. Concerns(按重要性)

1. **远端触点钉的环境前置**:两条远端钉测需对端 daemon 在线(`MTDSS_HOME=C:\mtdss-t6a mtdssd`,Windows 构建盒;本轮验证后已杀停,复跑先起)。对端离线=skip 不判失败(原语义保留);但**本轮起 skip 即损失真覆盖**(此前恒 skip 无损失),CI/设备轮跑前留意起对端。
2. **#2 解释待用户复认**(见逐项):远端浏览行左滑已有且已验;设备列表行按「无按设备动作」裁定不挂滑面——若用户坚持设备行本身要滑,需先立按设备契约面(解配/改名),单侧 [浏览] 钮重复点按不建议。
3. **门开关联 persisted 残留面**:门钩子直写 UserDefaults(0/1 双态);UITest 收尾复位+本轮 sim 已复位,但外部手工以 =1 启动过 App 后,后续普通启动保持门开(与 showHidden 钩子同性质,DEBUG 面不涉产线)。设置页开关即用户可见复位口。
4. **传输页在途动作无视觉反馈增量**:防连点守 VM(control 重入守卫),原自绘行在途钮半透明不迁(原生 swipeActions 无挂点);连点/慢网下的重复发起防护语义不变,仅观感差异。若真机反馈要反馈面,候选=行内 spinner 再票。
5. **短列表尾注沉底取舍蔓延到传输页**(R6 既有政策的自然延伸,非新裁决):List 化后「N 项」随内容不贴底;transfers 截图帧可见。若用户要沉底,候选=safeAreaInset 条件挂(仅短列表)再票。
6. **设置页门开关落位**:设置页无「与设备/安全」分组(实有三段:身份/照片图库/信任目录)——落身份段「显示隐藏项」下方(两个浏览面开关同段同形)。若控制者要独立「安全」段,移动为一行改动。
7. **账号目录噪声(既有,非本轮)**:设备列表截图可见多条历史「iOS 模拟器」陈旧离线条目(R5b concern 7 同源);本轮 dev daemon(T6-Windows-A)在线窗亦会留一条在线历史,无害。

## 5. 产物清单

- App:`Common/ListRows.swift`(SwipeActionSpec 迁入+注释)、`Common/SwipeableRow.swift`(删)、`Transfers/TransfersView.swift`、`Browse/{BrowseView,FavoritesView,SelectionBar}.swift`、`Profile/SettingsView.swift`、`Devices/DevicesView.swift`(裁定注记)、`MTDSSApp.swift`(门开关+钩子)
- Kit:`Sources/MTDSSKit/Ui/BrowseViewModel.swift`(BrowseSwipeAction 门参 + BrowseSelectionAction 新出集)
- 测试:`UITests/{BrowseListUITests(远端 id 重钉+标签激活+滑序钉正+门双臂),TransfersListUITests(新)}.swift`;`Tests/MTDSSKitTests/BrowseViewModelTests.swift`(门臂+选择集四臂)
- 文档:spec §4.1/§4.2/§4.4/§4.5/§4.6 + progress 状态行 + 本报告
- 截图:`artifacts/mtdss-r9-{transfers,devices,remote-inbox,settings-off,settings-on}.png`
