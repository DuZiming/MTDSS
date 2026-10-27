# MTDSS v2-P3 设计:Android 客户端

- 日期:2026-10-25
- 状态:实现完成(12 任务+终审+设备场 MIX 3 实机验证;执行裁定与实机发现见 plan 同目录与 .superpowers 台账,F3/F4/F5 立票 P4)
- 上游规范:[2026-10-24-mtdss-v2p2-task-manager-design.md](2026-10-24-mtdss-v2p2-task-manager-design.md)(P2 已收官,main@633af31)、[2026-10-22-mtdss-v2p1-relay-design.md](2026-10-22-mtdss-v2p1-relay-design.md)(中继兜底)
- 子项目定位:v2 总序 P0 → P1 中继(已收官)→ P2 传输任务管理(已收官)→ **P3 本设计** → P4 Windows UI 打磨 → P5 iOS

## 1. 意图与为什么

MTDSS 首批真实用户画像 = Windows + iPhone + 安卓,PC↔手机是主战场。P3 交付安卓客户端,让安卓机成为 MTDSS 网络里的一等设备:登录同一账号、出现在设备列表、可浏览对端、可收发、有任务管理。

产品定位(brainstorm 定语):**应急、方便、实用的数据传输与远端浏览**,不是长时间挂机搬数据的工具。这一定位贯穿全部裁定——生命周期、续传、存储模型都从它推出。

分发形态:**APK 直发起步**(VPS /dl 静态路径,与 M4 更新渠道同源),但设计以**应用商店上架为长期目标**——存储模型一开始就按商店合规做,直发与上架用同一个二进制,不做双形态。

## 2. 目标 / 非目标

| 目标 | 内容 |
|---|---|
| 安卓原生客户端 | Kotlin + Compose 完整原生 App;登录/设备列表/浏览对端/收发/任务管理/设置 |
| 引擎零改动复用 | mtdssd 以子进程形态打进 APK,信令/打洞/中继/QUIC/任务存储/对账续传与 PC 同一份代码 |
| IPC 契约移植 | crates/ipc 的 JSON-RPC + 事件流移植为 Kotlin 客户端;防漂移守护 |
| 存储统一模型 | 发送=系统选择器+自定义授权根;接收=固定收件箱;引擎文件 I/O 经 fd 桥(§5) |
| 任务管理一等化 | 任务列表为一等界面:三类型、中断态、手动重新发起=断点续传(P2 语义原样) |
| 应用内更新 | 检查更新(M4 manifest 同源)→ 下载 → 拉起系统安装器,用户确认才装 |
| CI | 新增 Android job:aarch64 mtdssd 交叉编译 + APK 组装 |

| 非目标(明确不做) | 说明 |
|---|---|
| 常驻/前台服务/通知挂载 | 用户裁定:前台=在线,后台交还系统进程管理,死活随缘 |
| 自动续传 | 用户裁定:回前台不自动重发;续传永远是用户手动点「重新发起」 |
| 全盘浏览 | 商店合规模型下本机树只含收件箱+用户授权根;不申请「所有文件访问」 |
| 静默自装 | 更新只到「拉起安装器」,装不装用户定 |
| iOS | P5 另立;本设计只保证**不制造安卓特例**(存储模型/契约/iOS 预留) |
| 桌面 UI 移植 | 不复用 React UI(用户裁定原生);UI 双份维护是已接受的成本 |
| 系统分享之外的发送入口自动化 | 无自动调度、无后台任务队列 |

## 3. 核心裁定(brainstorm 定稿)

1. **无常驻**:App 在前台正常运行且网络正常 = 设备在线;退后台/熄屏后死活随系统。客观校准(设计已吸收):后台死掉通常**断网先于进程回收**(熄屏后数分钟内系统掐后台应用网络,进程冻结随后),「没被回收还能跑」的实际窗口是秒到分钟级——够应急传输,不当后台下载器用。
2. **不自动续传**:任务列表只如实陈列状态;中断任务的续传=用户手动「重新发起」(P2 既有语义:翻新同记录换新 job_id + 对账续传),协议零新增。中断在安卓是高频场景,任务列表因此是一等界面。
3. **原生 UI**:安卓 Kotlin + Compose,iOS 将来 Swift。理由:移动端原生胶水躲不掉,各平台用各自语言换最佳体验;代价 = IPC 契约移植 + UI 双份维护,已接受。
4. **引擎不重写**:mtdssd 子进程,App 启动拉起、退出随杀,零 FFI;Kotlin 不碰 Rust 函数,而是 crates/ipc 本地套接字上的又一个客户端。当年「core 为 FFI 复用而生」的伏笔以「复用 IPC 契约」兑现。
5. **存储统一模型(商店合规为长期目标)**:发送=系统文件选择器(SAF)+ 用户自添加的授权根(OPEN_DOCUMENT_TREE 持久授权);接收=固定收件箱(安卓 `Downloads/MTDSS/`,iOS 将来 App Documents/文件 App);引擎文件 I/O 经 fd 桥,不持有真实路径假设。**不申请 MANAGE_EXTERNAL_STORAGE**(商店必卡;直发虽可用,但同一二进制两条渠道都合规是硬要求)。
6. **传输中屏幕常亮**:活任务存在时 `FLAG_KEEP_SCREEN_ON`(无权限、无通知、不是服务)——用户本来就在看着传,屏幕别自己灭。用户已认可此默认。
7. **版本基线**:minSdk 29(Android 10)、targetSdk 36、compileSdk 37(执行期校准:随实装工具链 platform 37;targetSdk 36 为 2026 商店主主流;评审裁定⑤:由原 minSdk 26 上调——测试机 MIX 3(Android 10)是验证下限,≤28 的传统存储分支无设备可验,与其背一条验证不了的承诺不如把支持面收窄到能验的下限;用户过目可否决回 26 并接受「≤28 分支文档化未验」)。
8. **分享入口 v1 带上**:注册 SHARE intent,系统「分享到 MTDSS」直达选设备发送——安卓最地道的发送入口,成本小。**平台事实(评审裁定③)**:ACTION_SEND 给的 URI 无持久授权,中断后「重新发起」按 P2 语义落在「源已不存在」——分享件即一次性;**可续传的重发以文件选择器/授权根为正统**(选择器当场 takePersistableUriPermission,树根按 rel 重建 URI,重发确定性成立)。此退化记录进 UI 文案与验收。

## 4. 架构

### 4.1 进程模型

```
┌─ Android App 进程(唯一进程)─────────────────────┐
│  Kotlin/Compose UI(全部界面与交互)               │
│  ───────── 本地套接字 JSON-RPC(与 PC 同构)────── │
│  mtdssd 子进程(引擎:信令/打洞/中继/传输/任务存储) │
└──────────────────────────────────────────────────┘
```

- **拉起**:Application/首 Activity 启动时以子进程方式启动 mtdssd(二进制按 jniLibs 约定打进 APK,文件名 `libmtdssd.so` 以随 native 库解包,从 nativeLibraryDir 启动)。传 Android 侧 `--home`(app 私有目录,真实路径,daemon 直接可用)。
- **生命周期**:App 起则起,App 亡则亡。注意:Android 上应用进程被杀时**子进程不会自动连带死**(reparent 到 init)——引擎侧 android cfg 加 `prctl(PR_SET_PDEATHSIG)` 三行保「App 亡引擎即亡」(评审裁定⑥:这是 cfg 门内的引擎小改,不违反「引擎不重写」的本意),Kotlin 侧 onDestroy destroy() 覆盖正常退出。App 存活期间 mtdssd 意外退出 → 拉起重启(任务状态自 tasks.json 恢复,running→「中断」,P2 语义兜底)。
- **日志**:mtdssd stderr/日志文件 → logcat。
- **桌面专属模块 cfg 门**:tray / uilaunch / update(Windows NSIS 编排)/ 通知全部在 android 目标下编译剔除。安卓的更新检查不走 daemon,见 §6。
- **网络**:信令/QUIC/STUN 出网只需 INTERNET 权限,无平台特例;打洞与中继兜底行为与 PC 逐字同源。

### 4.2 IPC 契约移植与防漂移

- 契约真源 = Rust 侧 `crates/ipc`(api.rs 暴露面)与 `ui/src/lib/daemon.ts` 的既有对齐物;Kotlin 侧手写等价客户端(socket 连接、JSON-RPC 请求/响应、事件流订阅、`RpcError` 线格式)。
- **防漂移守护**:契约快照测试——对每个 op 固定 golden JSON(请求/响应/事件各一),Rust 测试与 Kotlin 测试共用同一份 golden 文件,双端解析比对。新增/改动 op 而未同步两端 → CI 红。
- 移植范围(v1 触达面):daemon_hello→`hello`/`status`/`devices`/`login.request_code`/`login.verify`/`login.logout`/`list`/`stat`/`transfer.start`(安卓只用 upload-bridge 与 download 两形态)/`transfer.list`/`transfer.pause`/`transfer.resume`/`transfer.cancel`/`transfer.wait`/`transfer.reinitiate`/`transfer.clear` + 事件 `event.login`/`event.devices`/`event.online`/`event.transfer`/`event.notify`。**不移植**:autostart.*/update.*/set_recv_dir/send/ctrl 写操作(delete/move/copy/local_copy/remote_copy)——安卓 v1 只读对端+收发,不做远端文件管理。
- **收件箱哨兵(评审裁定②)**:安卓端 download 的 `dir` 固定传 `"inbox"` 哨兵串;daemon 在 android cfg 下接受该哨兵并路由到 BridgeDestIo(桌面侧不认识,照常 INVALID_PATH)。`status.recv_dir` 安卓回 `"inbox"`(UI 显示「收件箱 Downloads/MTDSS」);`set_recv_dir` 安卓回业务错误「安卓端收件箱固定」。
- golden 契约双形态:核心集(上列可移植 op)双端逐字节同源;少数平台分歧 op(`status` 的 recv_dir 字段等)以 `.android.json` 后缀变体文件并列。

### 4.3 安卓端 UI 形态(设计意图,细节归计划)

- 主界面 = 设备列表 → 设备页(浏览对端目录树 + 发送入口)。
- 任务列表一等界面(非抽屉角落):三类型、状态、进度、中断/失败行的「重新发起」「清除」。
- 本机侧:「收件箱」+「已授权根」列表(可添加/移除授权根,持久化 SAF 权限)。
- 设置:账号信息、收件箱位置(展示)、检查更新、关于。

## 5. 存储与文件访问桥(本设计最大新工程)

### 5.1 统一存储模型

| 动作 | 安卓映射 | iOS 映射(P5 预留) |
|---|---|---|
| 选文件发送 | SAF `ACTION_OPEN_DOCUMENT`(多选) | document picker |
| 添加自定义根 | `ACTION_OPEN_DOCUMENT_TREE` 持久授权,入「已授权根」 | 文件夹导入+安全作用域书签 |
| 浏览本机 | 收件箱 + 已授权根(Kotlin 侧原生枚举,不经 daemon) | 同左 |
| 接收落盘 | 收件箱 `Downloads/MTDSS/`(API 29+ 经 MediaStore 自有贡献零权限;API ≤28 传统读写权限+直路径) | App Documents(UIFileSharingEnabled,文件 App 可见) |

被毙方案存档:①「所有文件访问」(直发可用,商店必卡,且 iOS 无对应物);②「选完拷进私有目录再传」(零改动但大文件双倍占用双倍耗时,与应急定位拧着)。裁定=fd 桥。

### 5.2 引擎文件 I/O 抽象

引擎(与 PC 同一份代码)不再假设源/目标是真实路径,文件访问抽象化:

- **源(上传)——惰性按需 fd**:bridge 形态的 `transfer.start` 只带条目元数据(`ref`/`rel`/`size`/`mtime`,ref 为客户端不透明串);daemon 构建清单与发送时逐文件发 IPC 事件 `fs.request_source {request_id, ref}`,Kotlin 打开 SAF 文档(fd)后经 fd 通道回传;fd 在该文件哈希+送完即释放(免大目录句柄耗尽,重发时按 ref 重新解析 URI 再开)。
- **目标(接收)——请求式 fd**:daemon 经 IPC 事件 `fs.request_dest {request_id, rel, size, resume}`,Kotlin 创建/重开 MediaStore 条目回传 fd;完成时 daemon 发 `dest_commit {request_id, done}`(Kotlin 置 IS_PENDING=0 或清 pending 项)。10 秒无 fd 应答 → 该文件失败(前台语义的必然)。
- **枚举(授权根)**:Kotlin 侧完成,条目元数据随 start 提交,daemon 不走 fs。

引擎内抽象:manifest 构建与发送侧的「打开源」从 `PathBuf` 泛化为 `SourceFile { Path(PathBuf), Fd(OwnedFd) }`;接收侧 `PartWriter`/`reconcile` 参数化为 `DestIo`(桌面 `FsDestIo` = 今行为逐字节,安卓 `BridgeDestIo` = fd 直写 + 记账留 home + commit)。**cut(发送后删源)安卓 v1 不暴露**(评审裁定④:恒 false,UI 不给选项,fd 桥不加 delete 原语——SAF 删除权限按 provider 行为不一,退化路径多,P4 再议第 4 原语)。

### 5.3 fd 过桥机制

- 通道 = 现有本地 IPC 的 Unix socket,**SCM_RIGHTS** 传 fd(协议小扩展:握手/请求帧带上 fd 附带数据与元数据 JSON)。
- **续传语义在 fd 目标上保持**:对账所需的 `.mtdss`(偏移+bitmap)落 app 私有 `--home`(真实路径,daemon 可直接读写);payload 直写收件箱 fd;重开续传 = 平台层重开 fd + seek。原子替换语义(临时名→rename)在 fd 形态下改为「收件箱内最终名直写 + 完成标记」,半文件清理由收件箱管理承担。
- 枚举(ls_source)走 IPC 消息(Kotlin 枚举、发送条目数据),不走 fd。

## 6. 应用内更新(直发渠道)

- Kotlin 侧拉取 M4 同源 manifest(ed25519 签名与 sha256 校验照用),比对本机版本;manifest 基址经 Gradle 属性注入(与服务器 CI 的 MTDSS_DL_BASE 同源)。
- 有新版 → Kotlin 自行下载 APK(HttpURLConnection 直下,进度在设置页内呈现)→ FileProvider 拉起系统安装器 → 用户确认安装(需要「未知来源安装」的单应用授权,MIUI 同)。
- 下载失败重试=设置页「重试」按钮。**不进任务列表**(评审裁定①:撤回原「经 daemon download 任务下」——daemon 下载作业只支持从对端设备拉取,为 APK 新增 URL 源作业类型会使引擎/契约/任务语义全套膨胀,与应急工具定位不成比例)、不做静默自装、不做 Range 断点续传(数十 MB 重下可接受)。

## 7. 工程与工具链

- **开发环境(本机缺口,计划第一步)**:Android Studio + SDK + NDK + cargo-ndk(aarch64-linux-android 目标;国内镜像装)。测试机 MIX 3:开开发者选项+USB 调试;MIUI「USB 安装」可能要求登录小米账号;MIUI 双层权限弹窗(原生+安全中心)按真实环境验。
- **打包**:mtdssd 以 `libmtdssd.so` 名入 jniLibs;APK 签名密钥新立(与 ed25519 更新签名密钥同规格管理:私钥用户离线保管、CI 用 secret)。
- **CI**:新增 Android job(交叉编译 mtdssd + assemble APK + **JVM 单测档**:Kotlin 逻辑测试 + 契约 golden 比对;模拟器不进 CI——fd 传递/SAF/MediaStore 行为由 §8 MIX 3 实机冒烟兜底,评审裁定⑤ 同源);门禁口径延续双平台 test+clippy。
- **16KB page 对齐**:NDK 新版默认满足,记入打包 checklist,不为 MIX 3(Android 10)所迫但为上架留。

## 8. 测试与验收(实机冒烟口径)

登录 → 设备列表(对端 PC 在线)→ 浏览对端目录 → 从对端拉文件到收件箱 → SAF 选本机文件发对端 → 分享入口发一次 → 大文件传输中熄屏(任务中断)→ 回前台任务列「中断」态 → 手动重新发起=断点续传成功 → 清除含对端残留 → 检查更新全链(测试 manifest)→ 退出 App 对端立即见离线。

## 9. 后续移交(P4/P5 债)

- P5 iOS:存储模型(§5.1 右列已预留)、IPC 契约 Swift 移植、SCM_RIGHTS 等价(文件提供者 XPC/URL 书签)、前台才能收的 UX 叙事与安卓拉平。
- P4:桌面「按设备开放范围」与本设计「授权根」语义合流;任务记录两端列表口径统一。
- 上架前置:商店开发者账号(与 P0 Apple 账号并行)、隐私政策、隐私权限声明文案(INTERNET/存储的最小化表述)。
