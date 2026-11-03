# T13b R12 报告(20261103.3 设备轮两项:①扩展设备列表=App 设备列表完全统一 / ②导航内容滞后=旧列表残留)

状态:**两项全交付,Kit 219/219 全绿零 skip + sim 构建 BUILD(exit 0,app+extension 双 target)零 Rust 改动** / 基线 51fa37d → HEAD(4 commits)
执行日期:2026-11-03。执行面:Windows(编排+自审)+ Mac mini(swift test + build.sh sim)。
前置考据:task-13b-r11-report.md + R11 评审 Minor 票(「扩展面板不滤本机为发送目标,T11 先在缺口,需 selfDeviceId 入 App Group 载荷」)+ T5b/Ruling J(引擎侧哨兵统一 **LANDED**,unix 收端 `dir="inbox"` 回退默认收件目录)——**Ruling I 的「v1 期间 iOS 维持拒发」客户端立场确认过时**,本轮退役。

## 0. 逐项对账(brief 两条)

### 项 ① 扩展设备列表 = App 设备列表完全一致(用户:列表不一致/排序不对齐/排除本机;Linux 在线却被置灰)

**a. Ruling I 客户端拒发退役(SendService)**:`PeerInboxTarget` 枚举、`peerInboxTarget(platform:)`、`peerUnsupportedText` 文案删净;`send`/`forward` 的平台守卫臂拆除——dir 恒 = 收件箱哨兵 "inbox",**全平台通发**(windows/android/ios/linux/macos/未知;引擎侧 Ruling J 已统一回退语义,未知平台收端同样按非法回退默认收件目录,零协议变更)。四处陈旧注释同步(SendService 头注/DirectSendEngine/ShareQueue/BrowseView·FavoritesView 的 Ruling I 句)。
**四枚 Ruling-I 钉测**(`testPeerInboxTargetByPlatform`/`testSendToMobileAndWindowsPeersUsesSentinel`/`testSendToUnixDesktopPeerRejectedWithoutTransferFrame`/`testForwardToUnixDesktopPeerRejectedWithoutTransferFrame`)撤,换两枚全平台钉:`testSendToAllKnownPlatformsUsesSentinel`(7 平台含 nil/未知全通发,恰 7 帧 transfer.start 且 dir 恒哨兵——「不得短路少发帧」即拒发退役的反向断言)+ `testForwardToUnixDesktopPeerUsesSentinel`(linux B 端 remote_copy dir=哨兵)。

**b. 排序对齐 App(共享单份)**:Kit 增 `DeviceListOrder.onlineFirst`(在线在前,**组内保序**=daemon 目录原样序,离线沉底;Swift sort 稳定保证)——App 设备页 `DevicesViewModel.project` 委托之(排序体原样迁出,行为零变更),扩展 `ShareDeviceList.rows` 同走之(live/cache 两档同投影)。App 侧排序语义逐字未动,扩展侧从「目录原样序」变为与 App 逐行同序。选设备弹层的在线/离线分列同语义(不动)。

**c. 排除本机(双路径双源)**:
- **缓存路径**:快照增 `self_device_id` 可选字段(蛇形键;Codable 合成 encodeIfPresent/decodeIfPresent——**旧缓存文件缺键解码 nil 不滤,向后兼容**有钉)。主 App 三写点(boot 首写/前台回归/event.devices)随载荷携 `AppModel.selfDeviceId`;`refreshDeviceCache` 在 id 未投影时先 `refreshStatus()` 补拉(boot 首写/事件先到形尽量带齐,nil 时扩展侧不滤=旧形)。
- **live 路径**:`DirectSendEngine.liveDevices()` 改返回 `(entries, selfDeviceId)`——引擎 boot 即主 App 同设备身份,`status.device.deviceId` 即本机行,权威覆写缓存载荷(适配器面一条 os_log 增 self= 字段,真机核对面)。
- **滤除收口**:Kit 状态机 `ShareDeviceList` 增 `selfDeviceId` 存储 + `rows` 计算投影(恒滤 self + onlineFirst)——视图 `SharePanelView.deviceRows` 收敛为 `deviceList.rows`,与 App 选设备弹层(DevicePickerSheet 滤己)同语义。**T11 先在缺口(R11 评审 Minor 票)随此收口**。
- 钉测 ×3:`testDeviceListRowsExcludeSelfBothPaths`(缓存载荷滤+live 权威滤,linux 行保留)/`testDeviceListRowsOrderParityWithAppProjection`(同一目录经 `DevicesViewModel.project` 与 `ShareDeviceList.rows` 逐行同序 parity;App 字面量含本机行=设备页 R5b #3 不滤己语义,扩展侧滤后比对)/`testDeviceCacheSelfDeviceIdOptionalBackwardCompatible`(缺省写入+手写缺键 JSON 双兼容形)。

**d. Linux 置灰撤除(扩展行)**:`SharePanelView.deviceEnabled` 去 `peerInboxTarget` 臂(可选 = 缓存不过期 + 在线,平台不参与),`deviceSub` 去「暂不支持接收」形;SharePanelParts 行组件头注同步(离线/过期置灰不变)。linux 在线行 = 绿点可选可发,与 App 设备页/弹层同语言。

### 项 ② 导航内容滞后(用户:标题先变内容后变,无反馈;返回同)

**根因**:`open()`/`goBack()`/`goForward()` 的 `fetch()` 在应答落地前不清 `entries`(旧目录条目残留;远端冷握手 25-35s 窗内标题已切、内容还挂着旧目录,零反馈)。UI 侧 loading 分支条件 = `vm.loading && vm.entries.isEmpty`——不清列表则 loading 态永远不可见。
**修**:VM 导航三口在 `path` 赋值后即刻 `clearListForNavigation()`(entries/truncated 复位,单口私有函数)——列表区即时切入 ProgressView 转圈分支,「N 项」尾注随空列表隐没;应答落地填充;失败走既有 errorText 收口(seq 守卫/取消良性收口原样)。**`reload()` 不调此函数**——下拉刷新/「···」刷新/删除后重载的原地保内容语义不变(R5a #4 取消良性收口的内容前提随之成立),spec §4.1 既有「原地刷新保留」条目无冲突。
**钉测 ×2**(全路径 300ms 延迟应答脚本驱动,「应答在飞」窗内断言):`testNavigationClearsEntriesIntoLoadingUntilResponseLands`(open/goBack/goForward 三口各自:窗内 entries 空 + loading 真 + title 已切 + itemCount 0,落地填充新目录)/`testReloadKeepsEntriesWhileFetching`(reload 窗内旧内容保留——「只导航清」的对照面)。

## 1. Commits(顺序)

| # | commit | 内容 | 项 |
|---|---|---|---|
| 1 | `72cb5fc` | feat: 扩展设备列表=App 完全统一(拒发退役/排序滤己共享单份/快照 self_device_id/live status 自身 id) | ① |
| 2 | `2871606` | feat: 导航即时反馈=换页即刻清列表进 loading(reload 保内容不动) | ② |
| 3 | `8420284` | fix: R12① 三枚新钉测期望修正(Mac 首跑红转绿;产品面零改动) | ① 钉测 |
| 4 | (本条) | docs: spec §4.1/§4.7 + progress + 本报告 | 收尾 |

首轮实跑 3 失败全为**新钉测期望面笔误**(App 投影字面量漏本机行/live 断言漏离线 linux 行/legacy JSON 缺 platform 键与字面量不同形),修正后全绿——产品代码零返工。

## 2. 验证矩阵(全绿)

| 面 | 结果 |
|---|---|
| Mac `swift test` | **219/219 零失败**(R11 末 216;SendService −4+2、ShareQueue +3、BrowseViewModelTests +2 = 219) |
| Mac `build.sh sim` | **exit 0**(app+extension 双 target;DirectSendEngine/SharePanelView/MTDSSApp 改动面编译实证) |
| WSL/Rust | **零改动**(diff 无 crates/;哨兵统一是 T5b 已落地引擎面,本轮纯客户端) |
| 截图 | **不适用**(两项均为数据/时序语义:扩展 live 行集 sim 结构性不可达[R10 既有];loading 转圈为瞬态帧且 sim 本地拉取即时无窗可截;行观感 R11 帧仍准)——真机 checklist 承担 |
| 清场 | 无临时文件/无 dev daemon 遗留(本轮未起对端);Mac 收件箱夹具遗留无害(历轮同形) |

## 3. 真机 checklist(R12 增量;R10/R11 报告既有条目仍有效)

1. **扩展面板行集三面**(装 20261103.3+ 后):①本机行**不再出现**(此前「iOS · 在线」行在列=R11 评审 Minor 票);②行序与 App 设备页一致(在线在前组内同序,离线沉底);③**Linux 设备在线 = 绿点可选可发**(此前置灰+「暂不支持接收」),发送后对端落其默认收件目录(引擎 Ruling J 回退面,与 Windows 收件一致)。os_log panel 类 `live devices loaded count=N self=<id>` 可核 self 面到场。
2. **App 设备列表发 Linux**(App 内设备页/选设备弹层此前同受拒发守卫——send/forward 守卫拆除全 App 生效):对 Linux 对端发起发送/转发应受理落其默认收件目录,不再出「该设备系统暂不支持免选目录接收」。
3. **导航反馈**:远端设备(冷)浏览进下级/返回/前进——标题切换的同时列表**即刻清空转圈**(不再挂旧目录内容秒级),落地填充;**下拉刷新仍保内容**(原地转圈小面积,不清列表)。
4. **缓存路径滤己**(引擎不可开机兜底形):主 App 后台 ≥5min 后分享,缓存渲染的行集同样无本机行(self_device_id 随载荷;旧版 App 写的缓存[无该键]过渡期不滤=已知兼容形,App 打开一次即重写带 id)。

## 4. Concerns(按重要性)

1. **live 路径多一次 status RPC**(扩展面板装载):boot 后 status(取自身 id)+ devices 两连查,进程内 UDS 毫秒级,面板装载本就是异步态,无感面;失败顺序无新分支(任一抛 = markLiveUnavailable 缓存兜底,原样)。
2. **未知平台收端的落点语义**:全平台通发后,`platform` 无法判定的对端(理论上不出现在服务器目录;目录原样字符串)也发哨兵——落点 = 收端引擎对非法 dir 的回退默认收件目录,与 unix 桌面同语义(Ruling J 统一面),不挂不丢;宁发勿拒与「发送给任意设备默认落对方收件箱」产品原则(spec §3)一致。
3. **缓存路径滤己的旧缓存过渡窗**:20261103.2 之前版本的 App 写的缓存无 `self_device_id` 键 → 扩展缓存兜底形本机行暂不滤(live 可用时无此窗——live 恒权威滤)。主 App 下次打开即重写带 id,窗自愈;真机 checklist #4 可核。
4. **导航清空与首页 `.task` 初载守卫的交互**:BrowseView/FavoritesRootList 初载守卫 `entries.isEmpty && errorText==nil && !loading`——导航清空后 `loading=true` 挡住重入,应答落地 `entries` 非空,无重复 loadRoot 面(源码核查;Kit 钉的转圈窗断言间接覆盖)。
5. **「iOS 模拟器」账号噪声**(历轮同形):no-op。

## 5. 产物清单

- Kit:`Transfer/SendService.swift`(拒发删净+全平台哨兵)、`Ui/DevicesViewModel.swift`(DeviceListOrder 提取+project 委托)、`Share/DeviceCache.swift`(快照 self_device_id/write·readWithStaleness 三元/ShareDeviceList selfDeviceId+rows)、`Ui/BrowseViewModel.swift`(导航三口 clearListForNavigation)、`Share/ShareQueue.swift`(陈旧注释)
- 扩展:`MTDSSShare/{SharePanelView(rows 投影+可选性撤平台臂+load/liveDevices 接线),DirectSendEngine(liveDevices 二元组+status),SharePanelParts(头注)}.swift`
- App:`MTDSS/{MTDSSApp(缓存写点携 id+refreshDeviceCache 补拉),ShareQueue/ShareConsumer(writeDeviceCache selfDeviceId),Browse/{BrowseView,FavoritesView}(陈旧注释)}.swift`
- 测试:`SendServiceTests`(−4+2 全平台哨兵)、`ShareQueueTests`(+3 滤己/parity/兼容)、`BrowseViewModelTests`(+2 导航清空/reload 保留)
- 文档:spec §4.1(导航即时反馈条)/§4.7(列表统一三面:在线在前同序/滤本机/全平台可选)+ progress + 本报告
- 截图:无(见 §2 验证矩阵「截图」行)
