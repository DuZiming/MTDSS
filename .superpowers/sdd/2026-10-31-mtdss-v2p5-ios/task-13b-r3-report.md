# T13b R3 交互波报告(整行触区 / Ruling O 确认 / 动作 toast / 弹层高度 / 行内展开树)

状态:**五项全交付,Kit 全绿 + sim 构建绿 + 六帧截图** / 基线 cf8cd49 → HEAD(docs 收尾前 = c326d63,7 commits)
执行日期:2026-11-02。执行面:Windows(编排+评审)+ Mac mini(swift test + build.sh sim + 截图)。**WSL/Rust 零改动**(diff 仅 ios/ 与 docs/,crates 无涉)。

## 1. Commits(顺序)

| # | commit | 项 |
|---|---|---|
| 1 | `508934e` feat(ios): 浏览/设备/根行整行触区(R3 #1/#5):contentShape 补全行命中面 | #1/#5 |
| 2 | `512a5ae` feat(ios): Ruling O 确认钮只退出选择模式(R3 #2):无副作用;操作仅自底栏发起(先捕获后退出) | #2 |
| 3 | `3f72c54` feat(ios): 动作发起 toast + 完成行跳过计数可见化(R3 #3/#6) | #3/#6 |
| 4 | `7a6e47d` feat(ios): 选设备弹层 0.65 屏高 + 标题顶距让开抓手(R3 #4) | #4 |
| 5 | `69b9ea7` feat(ios): 文件夹行内展开树(R3 #8):箭头展开缩进逐级/行点击进整页/懒加载缓存/搜索收起/选择跨层级 | #8 |
| 6 | `678b1b7` fix(ios-tests): 展开树断言改「名#depth」拼串(Swift 6 元组非 Equatable) | 编译修 |
| 7 | `c326d63` fix(ios): 行内展开投影根级补搜索过滤(R3 #8 测试暴露) | 测试暴露修 |

实现逐项提交(BrowseView 跨 #1/#2/#3/#8 四项——按项重放编辑拆 commit,最终态与逐项叠加态逐字核对一致)。

## 2. 逐项对账

### 项 1(R3 #1/#5)整行触区

- 成因:行 Button 的默认命中面 = label 内容面,Spacer/留白不在内——真机只有文字/箭头可点。修:`.contentShape(Rectangle())` 补全行矩形,三处同课:**BrowseEntryRow**(浏览行)、**DevicePickerRow**(选设备弹层 + 设备页复用件)、**FavoritesView 根行**(收件箱根条目)。
- 长按进选择(外挂 `simultaneousGesture` LongPressGesture)同吃整行——手势命中跟随同一 contentShape。
- 与 R2 左缘右滑返回共存面:**tap 是点、边缘返回是拖**(DragGesture minimumDistance 10 + 几何谓词),无冲突面;`testEdgeSwipeBackPredicate` 七臂全绿(无回归)。行内展开的箭头钮触区另见项 5。

### 项 2(R3 #2,Ruling O)确认钮语义收口

- **VM**:`confirmSelection()` 反转——`SelectionOutcome` 枚举删除,函数改 `Void`、**只调 exitSelection()**(无论选否、无副作用);旧 T6「确认携带选择集进弹层」废止。
- **View**:底栏动作全部「先捕获 `selectedEntries`,再退出选择」——分享/转发 → `pickedForSend` 弹层;下载 → 捕获 paths 后立即 `exitSelection()` 再发起;删除路径不变(确认弹窗 → `deleteSelected` 原有收口)。
- **测试**:SendServiceTests 两钉面改写——`testConfirmWithEmptySelectionExitsSelectionMode`(0 选退出,无返回值)、**`testConfirmExitsSelectionModeRegardlessOfSelection`(反转面:N 选确认不携带,只退出清集)**;DEBUG 钩子(PICKER/AUTOSEND/AUTOFWD)同律改「先捕获后退出」。
- **spec §4.2 改**:「蓝圈白勾确认=退出选择模式(无论选否,无副作用)」+「操作仅自底栏发起…发起即退出选择模式 + toast」。

### 项 3(R3 #3/#6)动作反馈 toast + 跳过计数可见化

- **toast 组件**(新 `Common/Toast.swift`,App 内首个):`.toast($msg)` 修饰器,贴底黑胶囊(AppTab 横幅同语言),2s 自动消隐,单槽后到覆盖。接线:BroseView `startSend`/`startForward`/`downloadSelection` 发起成功(transfer.start 拿到 jobId)即「已开始 N 项 · 进度见传输列表」;失败仍走 `lastActionError` 弹窗。弹层 `onPick` 先 dismiss、动作面已先退选择——题面「dismiss sheet + 退出选择 + toast」三段齐。
- **跳过计数**:`TransfersFormat.skippedText(_:)`(Kit 纯函数,TDD)——skipped>0 →「跳过 N 项(同名已存在)」,0/负 nil;`TransferRowView` done 态副文追段。数据面 `TransferJob.skipped`/`TransferProgress.skipped` 契约既有,种子+事件帧双测钉保真。重复机制问题自此在传输行直答。
- 截图:`mtdss-r3-toast.png`(胶囊)· `mtdss-r3-skiprow.png`(发送行副文「本机 → … · 时间 · 跳过 2 项(同名已存在)」+绿勾)。

### 项 4(R3 #4)选设备弹层高度

- `.presentationDetents([.medium])` → **`[.fraction(0.65)]`**;头部顶距 8 → 16pt(标题/取消离开抓手与圆角)。before/after 双帧:`mtdss-r3-picker-before.png`(.medium,取消贴角)/ `mtdss-r3-picker-after.png`(0.65,顶距让开,列表区呼吸)。spec §4.3 同步改「0.65 屏高 sheet」。

### 项 5(R3 #8,主项)文件夹行内展开树

- **VM 状态机**(BrowseViewModel,宿主可测):
  - `BrowseRow{entry, depth}` 树投影行;`visibleRows` = 根可见行 + 展开子树逐层扁平(每层同一 隐藏过滤→排序 管线,depth=父+1);
  - `expandedPaths`(页级 @Published)+ `childrenCache` + `loadingChildren`;`toggleExpand`:展开=恰一次 `repo.list`(seq 守卫,迟到整包丢;缓存,收起释放,再展开重拉);在飞点按吞掉;失败静默不展开;
  - **搜索起手收起全部**(`searchText.didSet` → `collapseAll`,清空不恢复——状态清除,题面「state and document」);**导航换页重置**(open/goBack/goForward);**原地重载不收起**(reload 后 `refreshExpandedChildren` 逐个现拉刷新缓存,过期子项不残留);选择跨层级(`selectAll`/`selectedEntries` 走 visibleRows,收集序=行序)。
- **View**:行 chevron 改**展开钮**——› 展开态旋转 90° 成 ∨(蓝不变,.animation 驱动);触区外扩 28pt 方(12pt 字形直点太窄)、负 padding 还原布局占位(视觉像素不变);行点按仍 = 进整页(历史导航);缩进 = depth×`indentPerLevel`(18pt,Kit 常量),分割线同步内缩(`separatorLeadingInset: 20+indent`);子行(文件+目录)同交互同选择。
- **测试**(BrowseViewModelTests 六臂):懒加载恰一次+收起释放缓存+子级同排序管线 / 逐级缩进 depth 0→1→2 + 步长钉 18 / 搜索收起+清空不恢复 / 导航重置 / 选择跨层级(单选/全选/收集序)/ 重载不收起+子缓存刷新。两条修程:`678b1b7`(Swift 6 拒元组 Equatable 断言→拼串)、`c326d63`(测试暴露 visibleRows 根级漏搜索过滤——补齐与 visibleEntries 同口径)。
- **spec §4.1 改**:「文件夹行 = 箭头行内展开(缩进逐级)+ 行点击进入整页(历史导航)…一次展开一次拉取(缓存,收起释放);搜索时收起展开」+「整行可点」。
- **截图**:`mtdss-r3-tree.png`(01 工作区 → 内层 → Archive/设计稿 两级展开,›→∨ 旋转,逐级缩进)· `mtdss-r3-tree-select.png`(展开态选择:depth 0/1/2 三行勾选,圈随层级缩进,底栏动作条在)。

## 3. 验证矩阵(全绿)

| 面 | 结果 |
|---|---|
| Mac `swift test`(c326d63) | **139/139 零失败**(基线 131:−1 旧确认钉面 +1 反转钉面 + 6 展开树 + 2 跳过) |
| Mac `ios/build.sh sim` | 绿(引擎静态库 → xcodegen → 构建 → 安装 → 启动) |
| sim 截图 | 六帧 `artifacts/mtdss-r3-*.png`(tree / tree-select / toast / skiprow / picker-before / picker-after),真实引擎装载(device="self" 本机浏览全链) |
| WSL/Rust | 零改动(diff 无 crates/;未跑 cargo——无涉面) |

## 4. Concerns(按重要性)

1. **箭头钮点按真机面未验证**:chevron Button 嵌于行 Button 内(标准 SwiftUI 内层胜出模式),sim 无触点驱动——截图只证视觉态(旋转/缩进),点按分流(点箭头=展开、点行其余=drill、无双触发)留真机验证。若双触发,备选改 `onTapGesture` 分区或把展开钮提出行 Button 外。
2. **展开态生命周期裁定记录**:搜索起手=状态清除(清空搜索不恢复);导航换页(open/back/forward)=重置;原地重载=保持并逐个刷新子缓存(每个展开目录多一次 list——本地/远端均轻)。全部已钉测试,若用户反馈「清空搜索要恢复现场」再翻案。
3. **重载后子缓存刷新是尽力语义**:`refreshExpandedChildren` 逐个现拉,单个失败该级静默收起(不打断主面);子目录超引擎 10000 项截断时 children 只存前段(契约 truncated 未消费,与本页主列表同口径)。
4. **「N 项」沉底计数=根级**(visibleEntries 口径,不含展开子级)——沿用「当前目录计数」语义;若用户期望含子级再改(一行:改 visibleRows.count)。
5. **toast 只在发起成功后亮**(jobId 回来才算「已开始」);transfer.start 即败走 lastActionError 弹窗——题面「starts」取「传输确已调度」口径,避免假反馈。
6. **sim 截图噪声**(真机无此面):通知授权弹窗遮中部(sim TCC 无法预授,T13b/R2 已报先案);dotfiles(.DS_Store/.localized/.t10-hidden.txt)可见 = 该 sim UserDefaults 残留「显示隐藏项」ON(T10 轮产物,与本波无关);截图用临时测试树(~/Documents/01 工作区)已清,无残留。
7. **触区外扩手法**:chevron 28pt 触区经「+8 padding → contentShape → −8 负 padding」还原布局占位——像素与 mockup 一致,命中面实测(含与 folder 图标 8pt 重叠区的归属)留真机。
8. **edge-swipe 零改动面**:本波未动 BrowseGesture 谓词与页级手势;行 contentShape 增大命中面理论上不改拖拽判定(tap/drag 正交),谓词测试七臂绿。

## 5. 产物清单

- 代码:`ios/app/MTDSS/Browse/{BrowseView,DevicePickerSheet,FavoritesView,SelectionBar}.swift`、`ios/app/MTDSS/Common/Toast.swift`(新)、`ios/app/MTDSS/Transfers/TransfersView.swift`
- Kit:`Sources/MTDSSKit/Ui/{BrowseViewModel,TransfersViewModel}.swift`(BrowseRow/展开状态机/skippedText)
- 测试:`Tests/MTDSSKitTests/{BrowseViewModelTests,SendServiceTests,TransfersViewModelTests}.swift`
- 文档:`docs/superpowers/specs/2026-10-31-mtdss-v2p5-ios-design.md`(§4.1/§4.2/§4.3 三处修订)
- 截图:`artifacts/mtdss-r3-{tree,tree-select,toast,skiprow,picker-before,picker-after}.png`

## 6. 修复波 R3-fix1(评审 I-1,2026-11-02)

**I-1(Important)搜索收起不作废在飞子项拉取**——`searchText.didSet → collapseAll()` 只清状态不 bump 任何代际,慢在飞子项拉取(远端至 30s)迟到后照写 `childrenCache + expandedPaths`,清空搜索即复活展开,违「清空不恢复」钉面。

修(先红后绿):
- **`expandGeneration` 世代计数器**:`collapseAll()` 恒 +1(任何「展开语义重置」同权:搜索收起/导航重置);`toggleExpand` 在飞应答 **seq + 世代双守卫**(do/catch 两臂都守——catch 臂顺带修了「过期拉取失败误删新页合法展开」的边角);`refreshExpandedChildren` 同款双守卫(评审附带项:守 seq 原只孤儿缓存,守世代防孤儿缓存写)。迟到应答整包丢弃,不写任何面。
- **didSet 无条件收口**:搜索起手恒 `collapseAll()`(原 `!expandedPaths.isEmpty` 条件挪除)——展开集空但拉取在飞(点展开即搜索)同样要作废。
- 测试两钉(`fe84c0e` 先红实证:`[0,1]` 复活现场 + 即席展开走缓存;`7ff2ad0` 修后绿):`testSearchCollapseInvalidatesInFlightChildFetch`(展开在飞→搜索收起→迟到应答全丢→清空仍平面→再展开=重新拉取恰 3 次 list 无孤儿缓存);`testRefreshExpandedChildrenDroppedAfterCollapse`(重载刷新在飞遇收起,孤儿缓存不复活)。`09ffdb3` 修测试期望笔误(根单行平面=[0])。

Commits:`fe84c0e`(test,红)→ `7ff2ad0`(fix,绿)→ `09ffdb3`(test 字面修正)。**swift test 141/141 零失败**(139 + 2 新钉)。4 minors 与 c326d63 mislabel 流程注记按评审口径记录不动代码。
