# T13b R5a bug 修复波报告(#2 改名调查 / #4 下拉刷新 / #5 标题留白 / #6 搜索残留 / #10 居中确认 / #12 显示隐藏接线)

状态:**六项收口:#2 链路实证零改动,#4/#5/#6/#10/#12 五项代码交付,Kit 151/151 全绿 + sim 构建绿并装机** / 基线 5dfa578 → HEAD = 8e47116,6 commits(5 项 + 1 期望修正)
执行日期:2026-11-02。执行面:Windows(编排+自审)+ Mac mini(swift test + build.sh sim)。**WSL/Rust 零改动**(diff 仅 ios/,crates/server 无涉)。

## 0. 调查结论(#2 / #4 先查后修,按 brief 要求报告)

### #2 设备名改名不生效 → **全链路无断点,行为 = 下次登录生效,零代码改动**

逐环实证(客户端 → daemon → 服务器):

1. **iOS 提交的就是 AccountStore 生效名,不是 hostname/nil**:`LoginModel.verify` 上帧 `account.effectiveDeviceName(default:)`(改名 override > 登录实名 > 平台缺省),`Contract.loginVerifyParams` 恒带 `name` 键(LoginModel.swift:117-123;Contract.swift:516)。既有钉测 `LoginFlowTests.testRenameOverridesNameOnNextLogin`(改名 override 逐字段断言上帧)+ `testLoginSuccessFlipsSessionAndStoresIdentity`(缺省注入臂)本轮复跑绿——brief 要求的「Kit 钉 login.verify 带存储名」**已存在,无需新增**。
2. **daemon 透传**:`api.rs login_verify` 读 `name` 参数(缺省才回退 gethostname,api.rs:277-280)→ `account::verify(...)` POST `/auth/verify {name}`(account.rs:87-93)。
3. **服务器 re-verify 即改名**:`auth.rs verify_code` upsert `ON CONFLICT(device_id) DO UPDATE SET account_id, name, platform`(auth.rs:83-88)——同 pubkey 再验证码登录,名字必然更新。

结论:改名经「退出登录 → 验证码重登」后必然对端生效;**不自动生效是设计面**——会话经 session.json 恢复不走 verify,设置页注脚「设备名修改将在下次登录时对其他设备生效」如实。用户所见旧名的可能面:未重登,或对端设备列表未刷新(daemon `devices` 是现拉,刷新即见)。本地「我的/设置」页显示 `effectiveDeviceName`,override 一改**即时**本地可见。真断点排查为空:brief 的「hostname/nil 上帧」「server 不持久」两疑点均排除。「改名即时生效 server 面」维持 T10 票池(需 server 契约面,非本波 iOS 范围)。

### #4 下拉刷新「无法加载目录,请求失败,连接中断」→ **根因 = 任务取消被伪装成传输故障**;两层修复

**文案取证**:「请求失败:连接中断」= `BrowseViewModel.userErrorText` 的兜底分支——非 `RpcError`(传输错误自有文案「ipc 连接已断开」等,业务错误透传 daemon 消息)、非 `RpcGuard.TimeoutError`(「请求超时」)⇒ 只可能是 **`CancellationError`**。

**IpcClient 取消语义本身正确**(brief 疑点排除):`send` 的 onCancel 抛 `CancellationError`、连接不断、其余在飞不受牵连(IpcClient.swift:105-107)——本轮补注记钉死口径;传输错误文案带 `ipc` 前缀,与用户所见不符,进一步坐实非传输故障。

**真断点在 VM 错误映射**:Browse `fetch` / Devices `load` 的 catch 把取消当终端错误——清空 `entries`/`rawDevices` + `errorText=「请求失败:连接中断」`→ 全页错误态(ContentUnavailableView「无法加载目录」)。

**取消来源**:`.refreshable` 任务所有权在 SwiftUI——视图重建/用户离场等即中途取消(refreshable 任务生命周期系于视图,社区/文档同象);**「···」菜单刷新走非结构化 `Task {}` 永不被取消**——这正是「···刷新一直好、下拉刷新报连接中断」的分叉点。首页根「正常」的实相:其错误态是列表**内联行**(整页不换分支),且首段 `refreshStatus()` 静默吞错,即便取消也不呈现全页故障。

**其余疑点排除**(brief 列举):(b) 二连接——各页共用 `AppModel.repository` 单 IpcClient(DevicePickerSheet 同 repo;smoke-login 独立连接仅登录用);(c) load() 路径差——设备页「···」与下拉同口 `vm.load()`,无路径差。

**修复两层**(任一独立即可消症状,叠加兜全):
1. **VM 良性取消收口**:catch 先判 `CancellationError` → 仅还原 `loading`,列表/错误面原地不动(browse + devices 双侧)。
2. **三面 refreshable 非结构化 Task 承载**:`.refreshable { await Task { await vm.reload() }.value }`(browse/devices/首页)——即使 SwiftUI 取消包装任务,刷新也跑完,**下拉与「···」刷新完全同语义**。

**真机具体触发器未钉死**(sim 无拖拽触点驱动 refreshable 手势):候选 = 视图重建/identity churn/离场取消(iOS ScrollView refreshable 家族已知问题类)。两层修复覆盖全部候选路径——无论哪条触发,用户不再见假错误、内容不丢、刷新必然完成。VM 级取消钉测收口(见 §2 项 #4)。

## 1. Commits(顺序)

| # | commit | 内容 | 项 |
|---|---|---|---|
| 1 | `aef5f5e` | fix(ios): 下拉刷新取消伪装「连接中断」并清列表:VM 取消=良性收口,三面 refreshable 非结构化 Task 承载;browse/devices 取消钉测各一,IpcClient 取消口径注记 | #4 |
| 2 | `d06ae19` | fix(ios): 远端标题分隔留白:「设备名\|目录名」→「设备名 \| 目录名」,composeTitle+双测试更新,格式注记同步 | #5 |
| 3 | `6c98708` | fix(ios): 搜索跨级残留:open/goBack/goForward 清空 searchText,原地 reload 保留;导航清搜索钉测 | #6 |
| 4 | `325ef0b` | fix(ios): 退出登录确认改居中 alert(confirmationDialog 底部 sheet → alert) | #10 |
| 5 | `3eb5657` | fix(ios): 远端浏览页显示隐藏开关接线(RemoteBrowseDestination 完全没接 showHidden);Kit 钉根级+展开子级双层级过滤 | #12 |
| 6 | `8e47116` | fix(ios): showHidden 双层级钉测期望漏排根级非隐藏行 notes.md#0(首跑实测修正) | 测试修 |

逐项提交;#2 调查零代码(§0)。

## 2. 逐项交付说明

### 项 #4(两层修复 + 双钉测)
- Kit:`fetch`/`load` catch 首判 `CancellationError` → 仅 `loading=false` 返回(保留内容、不造错误;seq 守卫仍在先,导航竞态不受影响);`IpcClient.send` 注记取消口径。
- App 三面 refreshable 同款包裹(非结构化 Task)。
- 钉测:`testCancelledReloadKeepsContentAndShowsNoError`(在飞 reload 取消 → 内容保留/无 errorText/loading 收口;迟到应答不复活)、`testCancelledLoadKeepsDevicesAndShowsNoError`(devices 同款)。时序:首发即回装载内容,第二发迟到 400ms,取消窗口确定落在应答前。

### 项 #5(标题分隔留白)
- `composeTitle`:`\(deviceName)|\(folder)` → `\(deviceName) | \(folder)`(前后各一空格;题面字面,系统标题栏通用呼吸感;未用 thin space——CJK 语境下肉眼不可辨,留白诉求落不实)。
- 测试:`testComposeTitleDeviceAndFolder`/`testRemoteVMTitleSwitchesWithDevicePrefix` 四臂更新;BrowseNav/DevicesView 格式注记同步。根级单段口径、本地单段口径不动。

### 项 #6(搜索跨级残留)
- `open`/`goBack`/`goForward` 一律 `searchText = ""`(搜索只属当前层);**原地 `reload` 有意保留搜索**(刷新语义不重置用户输入,对照面钉住)。
- 钉测:`testNavigationClearsSearchText` 三导航臂 + reload 保留臂。

### 项 #10(退出登录确认居中)
- ProfileView:`confirmationDialog`(底部 action sheet)→ `.alert("退出当前账号?")` 居中弹窗,「退出登录」红字 destructive 保留。删除类确认(SettingsView 移除信任目录/BrowseView 删除)归 R5b #9,本波未动。

### 项 #12(显示隐藏开关不生效)
- **根因在 App 侧接线,不在 Kit 投影**:`RemoteBrowseDestination`(设备页推入的远端浏览)**完全没有** showHidden 接线——开关恒不生效于远端浏览(dotfiles 恒隐藏)。补 `.task { vm.showHidden = model.showHidden }` + `.onChange(of: model.showHidden)` 活体跟随(与本地 `BrowseDestination` 同款双面)。
- brief 疑点核对:Kit `visibleRows` 每层走同一 `levelEntries(showHidden:)` 管线(c326d63 后已含子级滤)——本轮排查**无缺口**;`FavoritesRootList`(param+task+onChange)与本地页接线原已正确。
- 钉测:`testShowHiddenFlipsRootAndExpandedChildren`(默认:根级+展开子级均无 dotfiles;翻开:双层级一并现形,子级文件夹恒前名称升序)。

### 项 #2(零改动,理由见 §0)
链路四环(iOS/Contract/daemon/服务器)全实证无断点;Kit 钉测已存在;行为=下次登录生效为设计面。真机复测口径:改名 → 退出登录 → 验证码重登 → 对端设备列表刷新后见新名。

## 3. 验证矩阵(全绿)

| 面 | 结果 |
|---|---|
| Mac `swift test`(8e47116) | **151/151 零失败**(R4 末 147:+2 取消钉测 +1 导航清搜索 +1 showHidden 双层级;首轮 1 失败=钉测期望漏排根级非隐藏行,已修) |
| Mac `ios/build.sh sim` | **BUILD SUCCEEDED**,装机启动(pid 23977;iPhone 17 Pro Max sim) |
| WSL/Rust 面 | 零改动(diff 仅 ios/) |

## 4. Concerns / 缓办

1. **#4 真机具体触发器未钉死**:sim 无拖拽触点,refreshable 手势路径无法在 sim 复现;两层修复覆盖全部候选路径(视图重建/离场/identity churn),**留真机复测确认**(预期:下拉刷新任何时序下不再出现「连接中断」,刷新结果与「···」一致)。
2. **#12 远端页 sim 未驱动**:远端浏览需真远端设备在线;接线模式与本地页同款 + Kit 双层级钉测收口,真机复测(翻开关 → 远端浏览 dotfiles 即时现形/隐没)。
3. **server 端 rename-on-reverify 无显式测试钉**:reregister 测试用同名(upsert 改名路径未单独断言)——票池候选;本波 Rust 零改动纪律未扩。
4. **#5 标题截图帧未重拍**:纯字符串改动,Kit 双测试钉;真机/下轮 sim 有远端在线时顺带重拍 r4-title 帧。
5. **DevicesViewModel 并发双拉取消的 loading 闪烁**(A 取消时 B 在飞,loading 短暂回 false):既有家族(VM 无 seq 守卫,改动前同在),未扩面;如真机可见再票。
6. **#2 用户沟通口径**:请按「改名 → 退出登录 → 验证码重登」复测;「改名即时生效」server 面在 T10 票池。

## 5. 产物清单

- Kit:`Sources/MTDSSKit/Ui/{BrowseViewModel,DevicesViewModel}.swift`、`Sources/MTDSSKit/Ipc/IpcClient.swift`(注记)
- App:`Browse/{BrowseView,BrowseNav,FavoritesView}.swift`、`Devices/DevicesView.swift`、`Profile/ProfileView.swift`
- 测试:`Tests/MTDSSKitTests/{BrowseViewModelTests,DevicesViewModelTests}.swift`(新 4 钉测 + 标题四臂更新)
- 文档:本报告 + progress.md 状态行
