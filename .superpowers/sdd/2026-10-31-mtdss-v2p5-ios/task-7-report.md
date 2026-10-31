# Task 7 报告:设备页(列表根 + 远端浏览)+ 远端动作 + Ruling I 实装

**BASE 573727a → HEAD 66bf713;12 commits;Kit 49/49 绿;E2E 三方(Windows A + WSL B + 模拟器)实传全过(见 §5)。**

## 0. Ruling I 裁定执行 + NEEDS_CONTEXT(控制者必读)

Ruling I(progress.md 口径)预设「桌面 = **devices 携带的**绝对 recv_dir」。**实证:契约不携带**——
- `crates/ipc/contract/devices.resp.json` golden:每台仅 `{device_id,name,platform,online}`;
- `crates/daemon/src/api.rs` devices 注释原文「每台恰含 device_id/name/platform/online」;
- 全 RPC 面核查:`status`/`set_recv_dir` 均只及**本机** recv_dir;无任何 RPC 暴露**对端** recv_dir;transfer.start upload/remote_copy 的 dir 预检 `remote_absolute_form` 要求非空(daemon jobs.rs 1177/2115),「空串=对端默认目录」的捷径在契约面不可达;
- platform 值域 = 服务器注册时 `std::env::consts::OS`(crates/daemon/src/account.rs:92)→ windows/linux/macos/android/ios。

按 Ruling I「do NOT invent」条款,**v1 落地 = documented fallback**(ruling 已列的两个现实选项之一),结构按平台拆分、升级点收口在一个函数:

```swift
SendService.peerInboxTarget(platform:) -> PeerInboxTarget  // .sentinel | .unavailable
// android / ios / windows  → .sentinel(移动=对端 App 应答发布;Windows=对端回退默认收件目录,T6 E2E 实证)
// linux / macos / 未知/nil → .unavailable(哨兵对 unix 桌面必挂发布查询;recv_dir 无从取得 → 宁拒勿挂)
```

send/forward 组帧前收口:`.unavailable` → `RpcError(code:1, message:"该设备系统暂不支持免选目录接收(Linux/macOS 桌面对端将随后续版本开放)")`,**transfer.start 帧不上线**(单测钉死)。

**NEEDS_CONTEXT(请控制者定夺,候选记 T5b 票池):**
1. **契约加对端 recv_dir**(Ruling I 原案):需对端把 recv_dir 上报目录/信令(注册或握手带出),server+daemon+三端契约增量,非 iOS 单侧可做。落地后 `peerInboxTarget` 换绝对路径形态,单测改预期即可(升级点已收口)。
2. **引擎侧哨兵统一**(T6 报告建议 #1,progress.md 已并入 T5b):unix 桌面无 App 应答时回退 recv_dir(同 Windows 现状)→ iOS 侧可全线哨兵,`peerInboxTarget` 全 `.sentinel`。**此案 iOS 零改动即可解锁 linux/macOS 对端**。
3. **维持现状(v1 已交付)**:linux/macOS 对端拒发 + 明确文案;Windows/移动对端全通。代价 = ruling 已认的「对 Linux/macOS 用户发送失败的 UX 缺口」。

E2E 影响见 §5:app 级转发 A→B(B=WSL)按设计拒发(链路证据 `[T7-E2E] AUTOFWD FAIL: RpcError ...`);A→B 数据面经 RPC(smoke,绝对 dir,T6 同口径)done + md5 对。

## 1. 交付物

| 项 | 文件/位置 |
|---|---|
| 设备列表根(视图) | `ios/app/MTDSS/Devices/DevicesView.swift`(DevicesView + DevicesRootView + RemoteBrowseDestination;RemoteBrowseTarget 导航值) |
| 设备列表 VM(Kit,宿主可测) | `ios/package/Sources/MTDSSKit/Ui/DevicesViewModel.swift` |
| Ruling I dir 解析 | `ios/package/Sources/MTDSSKit/Transfer/SendService.swift`(PeerInboxTarget + peerInboxTarget + send/forward 换 DirectoryDevice 签名) |
| 远端动作挂接(T6 点位启用) | `ios/app/MTDSS/Browse/BrowseView.swift`(isRemote 档:底栏 下载/转发、转发弹层、startForward/downloadSelection) |
| 转发弹层变体 | `DevicePickerSheet`(标题「转发 N 项到…」;`excluding` 滤来源 A——daemon 拒 from==to 前置) |
| T6-minor1(圆点灰显) | `SelectionBar.swift`(hasMore=false → 禁用且 systemGray3,不再误用主题蓝) |
| AppTab 接线 | `.devices` → DevicesView();传输/我的仍占位 |
| 设备页事件收敛(T6 缓办 #5 携带) | AppModel.subscribeEvents 句柄 + DevicesRootView 消费 event.devices → VM.apply |
| 单测 | `MTDSSKitTests/DevicesViewModelTests.swift`(7)+ `SendServiceTests.swift`(+4 Ruling I) |

**注**:brief 写 DevicesViewModel 在 `ios/app/MTDSS/Devices/`,但测试面在 MTDSSKitTests(宿主可测要求,T5 BrowseViewModel 先例)→ VM 落 Kit `Ui/` 组;视图落 app 侧 `Devices/`。

## 2. 关键设计

- **selfDeviceId 实时面**:VM 捕获 init 时的 self id 会在「status 晚于首拉」时漏滤本机(sim 实测复现)→ 改为 `@Published var selfDeviceId` + `devices` 计算投影(rawDevices 滤除+在线在前),AppModel.refreshStatus 晚到经 onChange 重滤。测试钉 `testLoadWithoutSelfIdKeepsAllDevices`(nil 不误删)。
- **远端浏览树根** = `list {device, path:""}` 空串 = 对端根卷列表(daemon ctrl 面;android BrowseScreen 同口径);标题 = 设备名(根)→ 文件名(深于根),BrowseViewModel 的 rootTitle/title 机制零改动复用。对端 dotfiles 过滤走 T5 showHidden 默认面(无新码)。
- **远端首拉超时**:BrowseViewModel.callTimeout 10s→30s——远端 list 要建对端连接(信令 punch/中继,实测冷启 ~25-35s),10s 必超时;30s 与 SendService.callTimeout 同口径。
- **stale 注记**:daemon devices 现拉失败回退缓存(stale=true)→ 设备页尾注「N 项(缓存)」;event.devices 到达即清。
- **离线设备行**:复用 DevicePickerRow(置灰 0.45 + disabled + 灰 ›),点击 guard online 不进;spec §4.3/§4.4 行语言同源。
- **spec §4.4「平台 · 在线/最近时间」的最近时间**:契约未携带(§0),渲染「平台 · 在线/离线」(同 picker,不编造)。
- **DEBUG 钩子报备**(全 `#if DEBUG`,simctl 无触点;产线不编译):`MTDSS_DEBUG_TAB=<tab>`(切标签)、`AUTOPUSH=device[:platform]|device:id=<id>`(推远端浏览;id= 钉单台)、`REMOTE_OPEN=<abs>`(直开子目录)、AUTODL/AUTOSEND(平台后缀 `|<platform>`,缺省 windows)、**AUTOFWD=<to_id>|<to_platform>**(转发全链,Ruling I 拒发同样打标)。

## 3. TDD 轨迹

1. **红**(0ee0b48):DevicesViewModel 7 测 + SendService Ruling-I 4 测(Mac swift test 编译红:类型不存在)。
2. **绿**(0d5aea1):实现后 48/48;中途两修(SendServiceTests String.init 二义 936f547;测试脚本错误队列吃应答槽 9a434d7)。
3. **终态**:**49/49,0 失败**(38 既有 + 6 DevicesViewModel + 1 事件收敛 + 4 Ruling-I;HEAD 复跑 13.4s)。
4. Ruling-I 断言面:平台表(android/ios/windows→sentinel;linux/macos/nil/freebsd→unavailable;大小写归一)、发送/转发帧 dir 逐字段对 golden、拒发路径**零 transfer.start 帧**、拒发文案非空。

## 4. sim 档证据(Mac mini)

- `build.sh sim` BUILD SUCCEEDED;smoke-login **SMOKE OK**(真登录产线,sim device_id 348ff57f…)。
- 截图(`artifacts/`,对表 mockup device-pages ②):
  - `mtdss-t7-devices.png`:设备根 = 蓝›+显示器/手机图标+名称+「平台 · 在线/离线」+绿/灰点;**本机已滤**(12 台→11 项);在线 7 台在前(T6-Windows-A/T6-WSL-B 绿),离线残项置灰沉底;根页「···」=刷新。
  - `mtdss-t7-remote-root.png`:推入 T6-Windows-A 远端浏览根 = **对端真实卷列表**(C:/E:/G:,蓝文件夹+›),标题=设备名。
  - `mtdss-t7-remote-inbox.png`:深于根 = A 真实 inbox 目录(5 个真实文件,「日期 - 大小」副文),标题=inbox(文件夹名)。
  - `mtdss-t7-remote-select.png`:远端选择态 = 全选/「2 项」/蓝圈白勾;行选圈;**标签栏隐藏**;底栏 = **下载+转发**(spec §4.2 远端集);更多圆点**灰显一致**(T6-minor1)。

## 5. E2E 实传(产线信令,三方:Windows dev daemon A + WSL dev daemon B + 模拟器)

A = T6-Windows-A(44d97f96…,recv_dir C:\mtdss-t6a\inbox,pid 15496);B = T6-WSL-B(392e2388…,/root/mtdss-t6b/inbox);两 dev daemon 均复用 T6 home(已登录),收尾已杀,**用户产线 mtdssd(pid 58416)未触碰**。

| 流向 | 路径 | 证据 |
|---|---|---|
| 浏览 A 真实目录 | sim → A 卷列表 + inbox 目录 | §4 截图两帧(远端 UI 全链,非 RPC 直驱) |
| **下载 A→sim** | AUTODL(t7-dl.txt) | sim 收件箱 md5 **c8552273ad23c00239074880958e7152 = A 源**;a.txt 亦下(md5 abf88f9a… = 源) |
| **发送 sim→A**(Ruling I:windows→哨兵) | AUTOSEND(inbox 2 项→确认→直发) | transfer.start upload dir=**哨兵**,作业 **done 2/2 files, 55/55 B**;A 侧落 recv_dir(回退口径):t7-dl (1).txt **md5 c8552273… = 源**(撞名改 (1));同内容 a.txt 被对端记录去重跳过(skipped 语义,记录 sha 相符) |
| **转发 A→B app 级**(Ruling I:linux→拒) | AUTOFWD(B) | `[T7-E2E] AUTOFWD FAIL: RpcError(code:1, 该设备系统暂不支持免选目录接收…)` ——**帧不上线,文案即§0 v1 缺口** |
| **转发 A→B 数据面**(RPC 口径=T6) | smoke-transfer 手帧 remote_copy(from=A `C:/mtdss-t6a/inbox/t7-fwd.txt` to=B dir=B 绝对收件路径) | job …:5 **终态 done**;B `/root/mtdss-t6b/inbox/t7-fwd.txt` **md5 89de4433333e04eb46181ea2ac8f8fb4 = A 源** |

Ruling I 双向均实证:sim→A 走哨兵(windows);A→B app 级拒发(linux)。传输列表(P9 面)任务卡均可见(kind upload/download/remote_copy 齐全)。

## 6. 缺口与注记(concerns)

1. **NEEDS_CONTEXT §0**(本报告头等):linux/macOS 对端发送的 v1 拒发缺口,T5b 二选一(契约补 recv_dir / 引擎哨兵统一)后解锁;iOS 侧升级点 = `peerInboxTarget` 一函数 + 单测表。
2. **devices 页快照面收敛已完成**(event.devices 秒级跟线);弹层(DevicePickerSheet)仍为打开时快照(离线置灰兜底,T6 注 #5 的「收敛」只落在设备页)。
3. 远端浏览根 = 卷列表,path="" 在**移动对端**(android/ios 引擎 roots guard)会 INVALID_PATH 拒(v1 不可浏览移动对端目录,错误态+重试呈现;产品面 v1 远端浏览主要指向桌面,与 android 同现状)。
4. 下载动作不退出选择模式(分享/转发经确认语义退出)——保留选择便于连发;无 spec 反对,记录在案。
5. smoke-transfer.sh 的 Windows 反斜杠路径经多层 shell 会吃 `\t` 为 TAB(os error 123 实证一次)——用正斜杠路径(C:/…)即稳;T6 报告样例即正斜杠。已留意,不改脚本(非本任务面)。
6. DEBUG 钩子清单更新(§2):TAB/AUTOPUSH(id= 钉台)/REMOTE_OPEN/AUTOFWD 为本任务新增,全部 `#if DEBUG`。
7. sim 档 relaunch 必须带 `SIMCTL_CHILD_MTDSS_HOME`(T4 发现复训一次:漏带 → Documents 容器路径 bind 必败 → helloTimeout 横幅;非回归)。
