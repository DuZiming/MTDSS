# T13b R2 mini-fix 波报告(边缘右滑返回 / Minor 1 blocking_fs / Minor 2 根去重)

状态:**三修全交付,全量绿** / 基线 29b8e59 → HEAD 4af9665(3 code commits + docs)
执行日期:2026-11-02。执行面:Windows(cargo check/suites)+ WSL(daemon 全量 workspace)+ Mac mini(swift test + build.sh sim + 截图)。

## 1. Commits(顺序)

| # | commit | 修 |
|---|---|---|
| 1 | `eade3c2` feat(ios): 浏览页左缘右滑返回(R2):目录级 goBack/浏览根弹回推入页 | R2 右滑返回 |
| 2 | `ede1992` fix(daemon): ctrl 根列表合成落 blocking_fs(R2 评审 Minor 1) | Minor 1 |
| 3 | `4af9665` fix(daemon): RootGuard 构造期授权根去重保首见序(R2 评审 Minor 2) | Minor 2 |
| 4 | docs: 本报告 + sim 截图 | 收尾 |

## 2. 逐修对账

### 修 1(R2 用户请求)浏览页左缘右滑返回

- **呈现面实证(双跳面结论)**:BrowseView 仅两处使用,均为 NavigationStack 推入页(FavoritesView `BrowseDestination` / DevicesView `RemoteBrowseDestination`),且都 `.toolbar(.hidden, for: .navigationBar)` 自绘顶栏——系统 interactivePopGesture 随导航条隐藏而失效(T13b 真机「系统返回不可用」先案,DevicesView.swift:209 注释在案)。故自装边缘手势是本页边缘返回的**独占者**:与系统手势无冲突面、无双跳面。选择:自定义手势同时承担「页内目录级后退」与「页弹回」两段(系统手势不在场,无「只做页内」的必要);若未来 iOS 在 toolbar hidden 下恢复系统 pop,须重审(见 Concerns 2)。
- **语义 = 顶栏返回箭头同一路径**:历史内(`canGoBack`)→ `vm.goBack()`(目录级);已在浏览根 → `onPopToRoot`(`navPath = []` 弹回推入前页面,即无系统手势约定下「自然的 iOS 返回」)。选择态**先行忽略**(选择中滑动 = 滚动列表,不退选——取题面「least surprising」口径)。
- **手法**:SwiftUI `DragGesture(minimumDistance: 10)` + `.simultaneousGesture` 挂整页 VStack(与 List 滚动并存;同页行长按 simultaneousGesture 先例);判定收口 `BrowseGesture.isBack` 纯几何谓词(MTDSSKit,宿主可测):左缘带 30pt 起手 + 水平位移 ≥70pt 且横纵比 2:1 以上——纵向滚动/斜向漂移/短漂移/左滑全滤除。非交互式(完成触发),R2 mini 范围不做跟手动画。
- 测试:`BrowseViewModelTests.testEdgeSwipeBackPredicate` 七臂(标准/斜向主导/出缘带/中部起手/纵向滚动/短漂移/左滑);goBack 分支语义由既有 `testBackForwardHistoryAndTitleSwitch` 钉面,onPopToRoot 分支与顶栏按钮逐字共用。**sim 手势在飞态不可截**(simctl 无边缘滑动触点驱动)——视觉证据降级为:build.sh sim 全链绿 + `MTDSS_DEBUG_AUTOPUSH=inbox` 推入页截图 `artifacts/mtdss-r2-browse.png`(该页类即本修改动面);手势真机行为留下一轮真机验证。

### 修 2(评审 Minor 1)ctrl 根列表合成落 blocking_fs

- `handle_ctrl_request` List 空路径授权臂的 `state.guard().roots_listing()`(逐根 `stat_path` + `child_count`)原样跑在异步 runtime 线程,与本文件「fs 调用一律经 blocking_fs 落阻塞线程池」口径相悖(T5b 教训同款:child_count 的 read_dir 在网络盘实测 6.5s/失联盘更久,fs.rs:816-820;iCloud Drive 类网络型根同理可 stall worker)。
- 修:`let guard = state.guard().clone()` → `blocking_fs(move || Ok(guard.roots_listing()))`,与同臂 list_dir/同文件 Stat/Delete/Move/Copy/CleanupOutbox 各臂同口径;`Ok(None)` 防御臂沿既有回退(enabled 形态恒 Some,不可达,行为不变),`Err` 臂透传 FsError 原码。
- 测试:既有 `roots_guard_root_listing_synthesizes_authorized_roots` 全绿(合成行/根 stat/未配置卷列表不变);WSL 全量绿(该臂走 spawn_blocking 需 runtime,tokio::test 在场)。

### 修 3(评审 Minor 2)授权根构造期去重

- `RootGuard::new`:规范化后与已收根**判等即丢,保首次出现序**。判等沿守卫自身比较语义——`same_root` = 组件级互含(`path_under_root` 双向,Windows 折大小写);**严格前缀不算重复**(根与其子目录两根并存)。信任书签解析收敛到既有根(Documents 自身 = 收件箱)不再产生重复虚拟根行。
- 分层注记:App 侧注入面已有去重(`EngineRootsConf.merge` + `testEngineRootsConfMergeInboxFirstAndDedups`,收件箱恒首位);本修是 **daemon 侧引擎兜底**,覆盖手编 engine-roots.conf 等绕过 App 层合并的注入路径。
- 测试:`duplicate_roots_dedup_keeps_first_occurrence_order`(cfg(unix),WSL 跑):完全重复 + 噪声形态(尾随分隔符/`.`)丢弃、严格前缀并存、内部根集首见序钉面、`roots_listing` 每根恰一行、集内 `check` 照旧/集外仍拒。

## 3. 验证矩阵(全绿)

| 面 | 结果 |
|---|---|
| Windows `cargo check --workspace`(MTDSS_UPDATE_URL unset) | 绿(0 warning) |
| Windows daemon suites | lib 96 / daemon_basic 7 / contract_golden 1 / tray_logic 14 全绿(与基线同数;新去重测 cfg(unix) 不在 Windows 计数) |
| **WSL daemon 全量** `cargo test --workspace`(~/mtdss-ci,工作树源 tar 同步) | **68 个 test-result-ok 全零失败**:daemon lib **92**(基线 91 + 1 新去重测)+ ctrl_e2e 17 / daemon_e2e 25 / daemon_basic 7 / contract_golden 1 / jobs_e2e 21 / relay_e2e 13 / inbox_layout_e2e 5 / inbox_staging_e2e 9 / flow_unit 7 / ops_unit 6 + server(signaling/stun)各臂 |
| Mac `swift test`(eade3c2) | **131/131 零失败**(基线 130 + 1 新谓词测) |
| Mac `ios/build.sh sim` | 绿(引擎静态库 → xcodegen → 构建 → 安装 → 启动);推入页截图 `artifacts/mtdss-r2-browse.png`(AUTOPUSH=inbox 钩子,6 项真实装载) |

## 4. Concerns(按重要性)

1. **手势真机面未验证**:边缘右滑的手感(触发阈值/与列表滚动共存/弹回跟手)是触点驱动面,sim 无法驱动——已用谓词单测 + 编译/装载/截图面兜底,行为留下一轮真机验证。阈值若需调,`BrowseGesture.edgeZone`(30pt)/`minSwipe`(70pt)两常量集中可调。
2. **系统 pop 恢复的双跳风险(记录在案)**:现依据 T13b 真机先案,`.toolbar(.hidden, for: .navigationBar)` 下系统 interactivePopGesture 失效,自装手势独占无冲突。若未来 iOS 版本在隐藏导航条时恢复系统 pop,同一滑动能「系统弹页 + 自装 goBack」双跳——届时自装手势应收窄为仅页内目录级(浏览根弹回交还系统)。BrowseView 注释已钉此依赖。
3. **DragGesture 被 List 抢占的理论面**:`simultaneousGesture` 下若滚动 pan 先行取消拖拽,个别斜向边缘滑的 `onEnded` 可能不触发(表现为偶发不灵敏,不会误触)。真机若确认,升级为 UIScreenEdgePanGestureRecognizer 宿主视图桥(改动面大,不在 mini 范围)。
4. **WSL 验证通路的会话限制**:本会话 worktree 隔离,WSL 内 git 被沙箱拦——`~/mtdss-ci` 以 **tar 文件拷贝**同步工作树源(crates/proto/server/Cargo.toml/Cargo.lock,无 git 操作、无删除),`cargo test --workspace` 对其执行。结论不受影响(源字节与 HEAD 一致);下一会话若跑 git 通路记得先 fetch 对齐。
5. **sim 状态残留**:build.sh sim 缺省清 `~/.mtdss-ios-sim`(登出净 home);通知授权弹窗仍遮截图中部(sim TCC 无法预授,T13b 已报,真机无此面);截图中收件箱 6 项 = device="self" 本机浏览全链(T13b 修#4 面)自然装载。
