# Task 11 报告 — 分享扩展(方案 A:缓存设备列表 + 队列 + 自动拉起主 App)

日期:2026-11-01 · 分支 `worktree-worktree-mtdss-v2p5-ios` · 基线 1519642 · **commits 2d080d5..1f57971(14 个)**

**状态:complete** — Kit swift test **121/121 绿**(15 新,先红后绿);`ios/build.sh` **app+extension 双 target 构建过**(MTDSSShare.appex 随包、双端 entitlements 签入);**消费侧 sim E2E 全链真传**:DEBUG 种子 → 队列 → 消费(校验→暂存→发送)→ Windows dev daemon A 收件 **md5 逐字节一致**;失败路径(盘损截短)→ 「分享转交失败」行实证;设备缓存/队列清空/App Group 落盘全验证。**扩展 UI 本体与真机分享入口归 T13**(sim 无分享入口无触点驱动,简报已裁定 DEFER)。

## 0. 核心裁定:消费形态 = 暂存收件箱 + 路径形态上传(bridge 形态不用,记录人裁定依据)

简报要求调查 bridge 上传(transfer.start `source:"bridge"`)。对契约真源(crates/daemon/src/jobs.rs `SourceResolver` + crates/ipc/src/fdpass.rs)核对结论:**bridge 形态需 App 侧 fd 提供机制——fdpass `0x01 source_fd`(SCM_RIGHTS sendmsg ancillary)+ `fs.request_source` 事件监听 + ref→文件注册表生命周期**,Swift 侧均未存在(现有 `FdPassChannel` 仅 0x04/0x05 无 fd 上行)。按简报认可的「极简等价」路线实现:

- 消费 = 逐文件**校验实际尺寸对声明** → **move 进收件箱 Documents**(撞名 ` (n)` 顺延,InboxPublisher 同语言;跨容器 rename 失败回退 copy)→ `SendService.send(paths:to:)`(T6 链,Ruling I 哨兵口径)→ 受理成功清队列。
- **副利**:路径形态上传若传输中途失败,引擎 `reinitiate` 可用(源路径仍在收件箱)——bridge 形态的 reinitiate 引擎必拒(T9 concern #3 已证),失败恢复链更完整。
- **代价**:分享文件在收件箱可见一份(Files App「MTDSS」区)——用户文件可寻回,与「不静默丢」裁定同向;非静默副作用,已记票。
- 后续上 bridge 形态的工连面:Kit `FdPassChannel` 加 0x01+fd 上行(sendmsg SCM_RIGHTS,~150 行 POSIX 面)+ EventHub 订阅 `fs.request_source` + ref 注册表;契约面(`transferStartUploadBridgeParams`)已备。

## 1. 交付面

| 件 | 落点 |
|---|---|
| 队列存储/消费(Kit,宿主可测) | `ios/package/Sources/MTDSSKit/Share/ShareQueue.swift`(`ShareQueueStore`[begin/copyFile 两步/finish/scan/markStaged/markFailed/retryOnce]+ `ShareQueueConsumer`[consumeOnce/retryOnce]+ `ShareQueueMeta` 蛇形 Codable + `AppGroup` 常量 + `ShareFormat.bytes`) |
| 设备缓存(Kit) | `ios/package/Sources/MTDSSKit/Share/DeviceCache.swift`(`SharedDeviceCache`[write/read/readWithStaleness]+ `SharedDeviceEntry`;**>5min 过期口径**,300s 整不过/301s 过) |
| 扩展写入面 | `ios/app/MTDSSShare/QueueWriter.swift`(`ShareQueueWriter`:NSItemProvider `loadFileRepresentation` 回调内拷临时 → 点设备两步拷入队列 + finish;半途失败不落坏项) |
| 扩展 UI(SwiftUI) | `ios/app/MTDSSShare/SharePanelView.swift`(抓手+图标+「发送到 MTDSS」+「N 个项目」+ 问号文档清单 + 说明行 + 缓存设备行[在线可选/离线置灰/不支持平台「暂不支持接收」/过期全置灰+提示];点设备=入队→open() 拉起→成功即收口/失败降级「已排队,打开 MTDSS 即自动发送」注记;open 3s 竞窗兜底)+ `ShareViewController.swift`(壳,NSExtensionPrincipalClass) |
| 主 App 消费装配 | `ios/app/MTDSS/ShareQueue/ShareConsumer.swift`(`ShareCenter`:消费触发门[登录态]、发送闭包[目标设备现拉 + SendService Ruling I]、设备缓存写入、失败行投影) |
| App 接线 | `MTDSSApp.swift`(`AppModel.share`:boot/event.login(true)/前台三触发;`.devices` 事件→缓存重写;`realignOnActive` 缓存+消费) |
| 传输列表失败行 | `TransfersView.swift`(`ShareFailureRowView`「分享」灰徽章 + 红因「分享转交失败 · <原因>」;左滑 [重试, 清除];重试=重选目标弹层复用 `DevicePickerSheet`;随 全部/发送 分段;计数入「N 项」) |
| 工程面 | `project.yml`(双 target `CODE_SIGN_ENTITLEMENTS` + App Group `group.top.mtdss.shared`;扩展 MTDSSKit 依赖[纯 Foundation 面,引擎 .a 不进扩展];激活规则 File/Image/Movie ×20[SupportsText 移除——纯文本/URL 不亮 MTDSS];主 App `CFBundleURLTypes` mtdss scheme)+ `MTDSS{,Share}.entitlements` |
| 测试 | `MTDSSKitTests/ShareQueueTests.swift`(15 例,先红后绿) |
| 截图 | `artifacts/mtdss-t11-share-failed.png`(传输列表双行实证:「分享」失败行红因 + 上一轮 E2E 完成行绿勾) |

## 2. 队列工程与失败语义(实测)

- **两步拷贝**:meta 先行(`.copying` + 逐文件声明 size,原子写)→ 每文件 `.part` 临时名拷贝 → 同目录 rename(`.part` 不在场 = 完整)→ 全落定翻 `.ready`。写入口尺寸守卫:源尺寸≠声明即拒拷(扩展侧早失败,不入坏项)。
- **不静默丢(评审 #4 裁定落地)**:`.copying` 项宽限窗外(120s,防主 App 消费撞仍在拷的窄竞窗)与孤儿目录(无 meta)→ scan 折算 `.failed`(「文件拷贝中断」)并**持久化 meta**;消费时逐文件复验(实测:盘损截短 → `status=failed, error=文件校验不符`,**未发坏字节**,队列项保留)。
- **staged 断点**:暂存完成但发送未受理(进程被杀)→ `staged_paths` 记录 → 重启续消(测试钉死);发送失败 → 文件留暂存 + failed 行,重试走 staged **不重拷**(目标可覆盖更新)。
- **重试语义**:失败行重试 = 重选目标即重发(用户动作,**不自动重发**——failed 项 consumeOnce 跳过);校验不符/拷贝中断型重试如实再失败(残件不可复原——源字节在已死的宿主 App),只能清除后重新分享。

## 3. 验证证据(Mac mini / Windows 执行)

1. **TDD 红**(2d080d5):`swift test` 编译红(`ShareQueueStore/Consumer/SharedDeviceCache` 不存在)→ 绿:实现后 **121/121,0 失败**(106 既有 + 15 新;终版 1f57971 复跑确认)。绿化途中自纠六则(fileManager 作用域、metaURL func、Date epoch 秒策略[JSONEncoder 默认 deferredToDate 的 978307200 偏移被 schema 测试抓住]、retryOnce 入口、markFailedQuietly 合并写回[防 stagedPaths 被扫描时点内存副本回滚]、撞名测试脚手架)。
2. **构建**:`ios/build.sh app` exit 0——**app + extension 双 target**;产物 `MTDSS.app/PlugIns/MTDSSShare.appex` 在场;codesign entitlements 双端含 `com.apple.security.application-groups=[group.top.mtdss.shared]`;app Info.plist 含 `CFBundleURLSchemes=[mtdss]`;扩展 Info.plist 激活规则 File/Image/Movie ×20 + NSLocalNetworkUsageDescription(plutil 实核)。
3. **设备缓存 E2E**:sim 真登录产线后 App Group 容器实落 `cache/devices.json`(在线目录快照,蛇形键)——扩展读取面数据源实证。
4. **消费 E2E 全链(产线信令,sim → Windows dev daemon A = T6-Windows-A `44d97f96…`,home C:\mtdss-t6a)**:
   - 种子:`MTDSS_DEBUG_SHARE_SEED=<src>|<A-id>`(boot 钩子走**同一 ShareQueueStore 写入面**)→ 3MiB 随机文件入队;
   - 消费:引擎 transfer.list = upload(id=1,`share-src.bin`,**done**,dst=`44d97f96…:inbox` 哨兵,src=`~/mtdss-t11/Documents/share-src.bin` = 暂存落位实证);
   - **md5 `d4622960b541f7079f1af96b375fba93` 源=A 收件全对**;App Group `queue/` **空**(消费即清);收件箱暂存件在场。
5. **失败路径 E2E**:`:badsize` 变体(拷后截短 8 字节)→ 消费校验拦下,meta 落盘 `{"status":"failed","error":"文件校验不符",...}`,传输列表「分享转交失败 · 文件校验不符」红因行 + 同屏完成行(截图);引擎侧零作业(坏字节未上线)。
6. **收尾**:dev daemon A(pid 35168,**按可执行 PATH 匹配**)已杀;`C:\mtdss-t6a\inbox\share-src.bin`、Mac `~/mtdss-t11/share-src.bin`+暂存件已删;**用户产线 mtdssd(pid 11752,AppData 路)未触碰**。dev home `~/mtdss-t11`/`C:\mtdss-t6a` 保留(历次先例)。

## 4. T13 真机首验清单(逐项,扩展 UI 与分享入口本任务 DEFER 的全部余量)

前置:iPhone 装包(TestFlight 或 Xcode 直装);**开发者账号注册 App Group `group.top.mtdss.shared` 并挂到 `top.mtdss.ios` + `top.mtdss.ios.share` 两个 App ID 重签**(sim 免签,真机必需——T13 签名装载的活)。

1. **激活规则**:相册选 3 图 → 分享 → MTDSS 在面板亮;文件 App 选 pdf → 亮;**备忘录纯文本/ Safari 纯链接 → 不亮**(SupportsText/URL 未声明)。
2. **扩展页**:MTDSS 图标 + 「发送到 MTDSS」+「3 个项目」+ 项目清单(名/尺寸)对 mockup flows ②;设备列表 = 主 App 登录态 ≤5min 内的缓存(在线可选绿点/离线置灰 0.45/Linux·macOS「暂不支持接收」)。
3. **过期置灰**:杀主 App 后 >5min 再分享 → 全置灰 + 「设备列表已过期」提示;从未登录过 → 「暂无设备缓存」提示。
4. **open() 拉起(iOS 26 实机行为,spec 4.7 风险注的本意验证)**:点在线设备 → 主 App 是否自动拉起?拉起则传输列表应见进行中/完成行(「打开时任务已在传」);**若失败** → 扩展应现「已排队,打开 MTDSS 即自动发送」注记(3s 竞窗兜底)→ 手动打开 → 自动发送。记录 open() 实际返回,作为绕法(responder chain,带审核风险)是否立项的依据。
5. **端到端完整性**:3 图 HEIC 直传 Windows/安卓,接收端 md5/开图验收;多文件「· N 项」显示。
6. **拷贝中断注入**:分享大文件后立即上划杀扩展(或锁屏)→ 稍后打开主 App → 传输列表出「分享转交失败 · 文件拷贝中断」行 → 重试(如实再失败或成功)→ 清除出列;确认队列项无静默消失。
7. **生命周期回归**:传输中切后台回前台(列表收敛/亮屏,携 T9 §5.5);完成通知横幅呈现(T9 遗留);isIdleTimerDisabled(T6 遗留)。
8. **本地网络弹窗**:主 App 首连弹 NSLocalNetworkUsageDescription 授权(spec §5.6;扩展自身无网络,不该弹)。

## 5. Concerns(诚实记录)

1. **收件箱可见副作用(建议记票)**:方案(暂存消费)使分享文件在 Files App「MTDSS」区可见一份。数据可寻回(与裁定同向)但属产品语义新增面;若产品要「不落收件箱」需上 bridge 形态(§0 工连面已列)。
2. **重试的诚实失败**:校验不符/拷贝中断型重试必然再失败(源数据不可复原)——行保持 failed + 原因刷新;清除是唯一出口。产品若要「可修复」需分享源重递(超出 v1)。
3. **扩展 UI 零 sim 驱动**:分享入口需宿主 App + 触点,headless sim 不可达——UI 按 mockup ② 实现+编译验证,交互/像素/真机 open() 行为全部归 T13(§4);逻辑面(队列/消费/缓存过期/格式)Kit 15 测试钉死。
4. **多文件 meta.source_name = 首项名**(显示面;简报 schema 单名,files[] 承载多文件声明)。
5. **open() 3s 竞窗**:completionHandler 在部分系统形态可能不回调——超时按未拉起走降级注记(队列保留,无损);真机实测后可校准。
6. **产线账号新增一台「iOS 模拟器」离线残项**(全新 home 即新设备,历次同形态无害);dev A 的 inbox 里 `t7-*`/`e2e-*` 历史件未动。
7. **DEBUG 钩子报备**(#if DEBUG):`MTDSS_DEBUG_SHARE_SEED="<源文件绝对路径>|<目标设备id>[:badsize]"`(boot 种子,同写入面+消费链;badsize=拷后截短走失败路径)。
8. 携带:`TransfersView` 分享行 busy 恒 false(重试经弹层,无连点面);`ShareDeviceRow` 平台图标/名 = 主 App DeviceIcon 的扩展侧复制(app 目标代码不进扩展,两处须同步改,已注释)。

## 6. 提交清单(1519642 → 1f57971,14 commits)

| commit | 内容 |
|---|---|
| 2d080d5 | test: ShareQueueTests 先行(红) |
| 8e7bae7 | feat: Kit ShareQueue(两步拷贝/蛇形 schema/消费面 staged 断点)+ SharedDeviceCache + 裁定注 |
| d82ce8c / a256cf4 / 301dd83 | 绿化编译修(fileManager 作用域/metaURL/测试脚手架) |
| 713b63a | fix: Date=epoch 秒策略 + retryOnce(failed 不自动重发)+ 测试对齐 |
| 0908995 / ed19838 | fix: entry meta var(重试覆盖)/ markFailedQuietly 合并写回 + schema 测试同策略 |
| c5606a3 | feat: 扩展 UI + ShareCenter/AppModel 接线 + 失败行 + entitlements/激活规则/URL scheme |
| e6918cd / 3499985 | fix: NSString 桥 / filter 显式收参 |
| fca7905 | fix: badsize 种子改拷后截短 |
| 1f57971 | fix: open() 3s 竞窗兜底 |

净 diff:14 files,+1666/−40(Kit 2 源 + 测试 1 + app 4 + 扩展 3 + entitlements 2 + project.yml 1)。
