# T13b R10 功能波报告(分享扩展直发+表内进度——收官前最后一功能;用户裁定 20261103.2:选设备 → 进度条 → 完成,无排队无拉起无「好」钮)

状态:**全交付,Kit 209/209 全绿 + ios-bridge 7/7 + sim 构建 app+extension 双绿 + device 编译绿 + 会话同步 sim E2E 实证 + DEBUG 状态页直发面截图** / 基线 7e0156e → HEAD(11 commits)
执行日期:2026-11-03。执行面:Windows(编排+自审+workspace check)+ Mac mini(swift test + build.sh app/sim/device + sim E2E + 截图 + 引擎 RSS 实测)。**引擎(Rust)零产品改动**(仅 ios-bridge 测试 +1 枚派生钉;diff 无 crates 产品面,传输契约无涉)。

## 0. 架构落地(brief 四条逐一)

1. **扩展链引擎(brief ①)**:`MTDSSShare` target 增 `Engine/libios_bridge.a` 依赖 + `LIBRARY_SEARCH_PATHS`(project.yml;与主 App 同一份 .a,sim 档吃 sim slice / 真机归档吃 device slice);`EngineBindings.swift` `@_silgen_name` 三符号镜像主 App `EngineHost.swift`,构造 Kit `EngineHost.Bindings` 注入(T4 注入式设计原样复用,Kit 零改动)。**裁定偏离(重要)**:扩展引擎 home 名 = **`mtdss`**,非简报原案 `engine-home`——真机 App Group 容器路径 `/private/var/mobile/Containers/Shared/AppGroup/<UUID>` 已 83 字节,`/engine-home/daemon.sock`(24)= **107 字节超 UDS sun_path 104 上限**(bind 必败);`/mtdss/daemon.sock` = 101 可容。`SessionSyncTests.testDeviceAppGroupHomeFitsSunPath` 以真机字节样例双面钉(正:fits;反:engine-home 不 fits),运行时 `EngineLayout.socketPathsFitSunPath` 复验、超限整体降级队列。
2. **会话同步(brief ②)**:最小素材集经 daemon 源码核对 = **`session.json` + `identity.seed` 成对**(flow.rs:两者 device_id 分歧即启动硬错→缺一整单不同步;settings.json/tasks.json/peers.json 均非必需,peers TOFU 缓存尽力拷贝)。Kit `EngineSessionSync.sync(appHome:extHome:)`:原子写(临时名+rename,0600 同 daemon write_private 口径)、**session.json 恒最后落 = 提交标记**(扩展侧见 session 在场 = 其余素材齐)、ca 证书(session 内为绝对路径)拷入扩展 home 并改写指向、登出/素材不全 = 撤镜像(先 session 后其余)。主 App 三触发:boot / **event.login(登录=拷,登出=撤)** / 前台回归(realignOnActive)。**契约核查两结论(消除了双 home 顾虑,零 Rust 改动的依据)**:① refresh 轮换是无状态 JWT 重铸(server auth.rs,无吊销注册表)→ 两份副本各自自愈;② presence 注册 last-connection-wins(server DashMap insert)→ 主 App 后台僵连接被扩展连接安全接管,扩展收口后设备下线至主 App 再前台。
3. **直发流(brief ③)**:点设备 → 两步暂存 `{group}/send-staging/<批>/`(Kit `SendStaging`,尺寸守卫同队列两步拷)→ `engine-roots.conf` 写暂存根 → `EngineHost.start` boot → `SendService.send`(Ruling I 哨兵)→ EventHub transfer 事件泵逐帧喂 `ShareSendFlow` → 终态 → 停引擎 → 1s 后 completeRequest。批次帽 `ShareDirectSendLimits`(50 件 / 200MB,边界钉测)。**降级阶梯**:未同步/超帽 → 旧拷贝路入队(Kit `enqueueFiles`,T11 语义);boot 失败 → 已暂存批 **rename 收编**入队(`adoptFiles`,同卷零拷贝);受理失败/终态 failed → **failed 双钮**(换设备重发[引擎仍热不重暂存]/转队列);取消 = transfer.cancel → cancelled → 收口清批。
4. **拉起退役(brief ④)**:SharePickFlow + 两级拉起(open()/responder-chain)+ `AppGroup.openHostURL` 整体删除;queued 注记即刻呈现 + 1.5s 自动收口(无「好」钮)。mtdss:// scheme 注册保留(外链兼容,静默吞)。

## 1. Commits(顺序)

| # | commit | 内容 |
|---|---|---|
| 1 | `627d219` | test: 直发三面钉测先行(TDD 红,Mac 实证编译红) |
| 2 | `129dd2f` | feat: Kit 三面——ShareSendFlow/EngineSessionSync/SendStaging + home 名定 mtdss(预算钉) |
| 3 | `61afa10` / `159bbd6` / `c469730` | fix: 测试面三修(IUU 绑定/filesTotal 投影/断言笔误/根目录) |
| 4 | `9649949` | feat: 扩展直发全链(绑定/编排/面板/同步三触发/DEBUG 状态页/工程面) |
| 5 | `c47d6f8` / `68b8b86` / `19496ba` / `9c6768d` | fix/refactor: 编译修×2 + 流状态改 @Observable 盒(见 §4 排障 3) |
| 6 | `856a85e` | test(ios-bridge): 扩展 home Documents 派生钉(引擎零产品改动) |
| 7 | (本条) | docs: spec §4.7/§5.4 重写 + progress + 本报告 + 截图 |

## 2. 验证矩阵(全绿)

| 面 | 结果 |
|---|---|
| Mac `swift test` | **209/209 零失败**(R9 末 184 − SharePickFlow 9 撤 + 直发三面 34;含帽边界 50/51、200MB/201MB、逐文件推导三形、ca 改写/撤镜像/提交序、收编 move/坏单拒) |
| Windows `cargo check --workspace --tests` | 绿,0 error/0 warning(桌面行为不变红线) |
| ios-bridge 测试 | 7/7(含新 `container_documents_app_group_extension_home` 派生钉) |
| Mac sim 构建 | `build.sh app` + `build.sh sim` **BUILD SUCCEEDED**(app+extension 双 target,扩展链接引擎 sim slice) |
| Mac device 编译 | `build.sh device` **BUILD SUCCEEDED**(CODE_SIGNING_ALLOWED=NO 编译验证,扩展吃 device slice;签名归档归既有 device-sign 产线) |
| **会话同步 sim E2E** | sim 真登录产线(build.sh sim + smoke-login,device_id a2bdbc3c…)→ **App Group 容器 `mtdss/` 实落 `session.json`(device_id 一致,ca=null 透传)+ `identity.seed`(64B),双文件 0600**——event.login(true)→同步链实证 |
| DEBUG 状态页截图 | `artifacts/mtdss-r10-sharestates.png`(共享件同源:设备行四态 + 直发逐文件行[完成绿✓/在途 40% 937.5KB·2.3MB/待发]+ 总进度 1/3 项 70% + 取消钮 + 排队注记) |
| 引擎 RSS 实测 | native mtdssd(同引擎码,release,产线信令在连)**RSS ≈ 10 MB**;扩展面板 + 引擎余量充足(iOS 分享扩展限额数十 MB 级起) |
| 回归面 | R9 既有 11 条 XCUITest 未复跑——本轮零触碰其被测视图(浏览/传输/设备/设置页无改动;sim 冷启动+真登录+截图路径全通,app boot 链实证);远端两条需 Windows dev daemon 在线,与本轮改动面无关 |
| WSL/Rust | 引擎产品面零改动(仅 ios-bridge 测试 +1);`cargo test -p ios-bridge` 绿 |

## 3. 真机 checklist(下一设备轮,全部为 sim 不可达面)

1. **直发全链**:相册 3 图 → 分享 → MTDSS → 点在线 Windows/安卓 → 表内进度条逐文件推进 → 「已发送」→ md5 对端验收;多文件「N 个项目」。
2. **同设备双在线形态**:分享面板打开期间 Windows 端 `mtdss devices` 看本机 iPhone 在线(扩展引擎接管);收口后短暂离线、主 App 重开恢复在线——确认无「僵尸在线」观感。
3. **取消钮**:传输中点取消 → cancelled → 收口;对端零残件。
4. **降级三臂**:①杀主 App 后 >5min(缓存过期,全置灰,无直发);②主 App 从未登录(提示打开 App);③>50 件/200MB 大批(排队注记 → 打开 App 自动发送)。
5. **失败双钮**:对端离线时点设备 → 受理失败 → 「换设备重发/转队列」——重发他设备成功;转队列后开 App 自动发出。
6. **扩展限额**:Xcode memory gauge 真机观察扩展进程 RSS(sim 无 jetsam 不作数);发 200MB 大文件压力形。
7. **本地网络弹窗**:扩展引擎首连的 NSLocalNetworkUsageDescription 授权形态(扩展进程弹 vs 主 App 已授权复用)。
8. **任务历史**:直发完成后主 App 传输列表**无**该任务(裁定内;对端有)——向用户复认一次。
9. **通知面**:直发完成无本地通知(通知面归主 App 消费链;扩展进程通知授权为独立面,v1 不做)。

## 4. 排障记录(实证防复发)

1. **`engine-home` 命名超预算(简报假设修正)**:简报预判「App Group 路径短,expected fine」——实数 83B 组路径 + 24B = 107B 超限。 Kit `EngineLayout.socketPathsFitSunPath` 预算模式 + 短名双保险,测试字节样例钉死。
2. **XCTest IUU 陷阱**:`private var root: URL!` 经 `let group = root` 绑定后推断为 `URL?`(IUU 不跨 let 传播)——成员访问编译红;显式 `let group: URL = root` 修。
3. **`@MainActor (inout T) -> Void` 闭包型调用编译退化**:尾随闭包与括号形均报「closure passed to parameter of type 'ShareSendFlow'」——该组合的调用匹配不可用;改 `@Observable` 状态盒(`ShareSendState`,引用语义直改)——顺带更贴代码库模型风格(Kit 四模型同族),视图观察面随 property 跟踪。
4. **SwiftPM 测试根目录**:`Data.write` 不建父目录——setUp 建 root(此前各测试靠 newBatch 的 withIntermediateDirectories 间接建,enqueueFiles 测试直接落根)。
5. **os_log 插值闭包**:OSLogMessage 插值是闭包形态,成员引用须显式 `self.`。

## 5. Concerns(按重要性)

1. **直发失败即弃形(failed 相位直接关面板)**:failed 双钮之外没有第三出口——用户若直接下滑关面板,暂存批被 deinit 清弃,分享内容不落队列(源 App 可重分享)。裁定同向(v1 拒静默排队),但「转队列」钮是唯一保全出口,真机轮留意用户困惑面;若反馈要「关面板自动转队列」,一行改动再票。
2. **任务历史不同显(裁定内,票已记)**:扩展 home 的 tasks.json 与主 App 不通;分享面板完成即终。spec §4.7 已文档化。
3. **对端向本机来件撞扩展窗口**:分享面板打开的数秒内若有对端发文件进本机,扩展引擎的 fs.publish 无应答方(InboxPublisher 只在主 App)——发布查询 30s 超时,来件滞留扩展暂存区、主 App 下次打开不补发布(扩展 home 无消费者)。秒级窗口 + 罕见形,v1 接受;真机若实证可复现,候选=扩展 boot 期间拒收或面板页脚提示,再票。
4. **照片桥探针挂空窗**:扩展引擎 photo_serve=true(桥单参恒注入)但无 PhotoResponder 应答——对端此时列本机照片根吃 10s 探针超时后根隐藏(R7 既有口径)。窗口=面板秒级寿命,可忽略;记录防误判。
5. **Kit 遗留面**:`CallbackAwait.swift`(withCallbackFallback/firstOrTimeout)随拉起退役失去产品调用方,钉测保留(通用工具,5 测试);`ShareEnqueueError.copyFailed` 同形(无调用方)。不删防腐化回退。
6. **设备账目噪声**:sim 真登录每轮新增一台「iOS 模拟器」离线残项(历轮同形,产线账号现 24 台);本轮 no-op。
7. **schem 留守**:mtdss:// URL scheme 与 App Group entitlements 保留——前者外链兼容静默吞,后者本就是共享面根基。

## 6. 产物清单

- Kit:`Sources/MTDSSKit/Share/{ShareSendFlow,SessionSync,SendStaging}.swift`(新)、`ShareQueue.swift`(enqueueFiles/adoptFiles + errorText 公开 + openHostURL 删)、`SharePickFlow.swift`(删)
- 扩展:`MTDSSShare/{EngineBindings,DirectSendEngine}.swift`(新)、`SharePanelView.swift`(直发面板重写)、`SharePanelParts.swift`(+ShareSendRow/ShareOverallBar 共享件)、`ShareViewController.swift`(拉起撤)、`QueueWriter.swift`(装载面收窄)
- App:`MTDSSApp.swift`(会话同步三触发)、`Debug/ShareStatesHarness.swift`(直发面)
- 工程:`project.yml`(扩展 .a 依赖+检索路径+注释)、`crates/ios-bridge/src/lib.rs`(测试钉)
- 测试:`ShareSendFlowTests`(18)/`SessionSyncTests`(10)/`SendStagingTests`(6);`SharePickFlowTests`(删)
- 文档:spec §4.7/§5.4 重写 + progress 状态行 + 本报告
- 截图:`artifacts/mtdss-r10-sharestates.png`
