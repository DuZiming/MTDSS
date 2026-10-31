# Task 9 报告 — 传输列表页 + 本地通知(v2-P5 M3)

日期:2026-11-01 · 分支 `worktree-worktree-mtdss-v2p5-ios` · 基线 7b9d114 · **commits 6cba8d3..cbac8e2(10 个)**

**状态:complete** — Kit swift test **87/87 绿**(15 新,先红后绿);sim 档构建过;真传输 E2E 全链(接收/发送/中断/续传/清除/来件重投)三份 md5 全对;截图七帧入 `artifacts/`;dev daemon 已杀净,用户产线 mtdssd(58416)未触碰。

## 0. 简报问句逐条裁定

- **通知时机**:完成即通知,**不区分前台/后台**——前台呈现交系统(`willPresent` → banner+sound,与后台同一呈现语言,零自绘);spec §3「传输完成 → 发本地通知」无前台豁免。
- **授权时机(least intrusive,已裁定)**:**首次真正要发通知时**请求一次(`UNLocalNotifier` 内实现;拒绝缓存、不再骚扰)。E2E 实证:首个完成瞬间弹授权框(`artifacts/mtdss-t9-done-notify-prompt.png`)。
- **pause/resume 不上 UI**(简报核对结论:spec §4.5 进行中只列「取消」)——进行中左滑 = `[取消]` 单动作,VM 测试钉死。
- **中断红字**:引擎 error 字段即原因,error 空 → 兜底「已中断」;E2E 杀 App 重启实测走的是兜底路径(引擎 load 标 interrupted,error 为空)。
- **完成时间**:契约不带时间戳(transfer.list/event.transfer 均无,逐一核对 golden)——只对**现场观测到 done 翻转**的行记本地时间(会话内);种子历史完成行只显绿勾不编造时间(task-7 设备页「最近时间不编造」同先例)。

## 1. 交付面

| 件 | 落点 |
|---|---|
| 传输页视图模型(Kit,宿主可测) | `ios/package/Sources/MTDSSKit/Ui/TransfersViewModel.swift`(`TransfersViewModel` + `TransferFilter` 四段 + `TransfersFormat` 呈现/动作投影 + `TransferRowAction`) |
| 完成通知判定面(Kit,宿主可测) | `ios/package/Sources/MTDSSKit/Transfer/CompletionNotify.swift`(`LocalNotifier` 协议 + `NoopLocalNotifier` 缺省 + `TransferCompletionAnnouncer`[仅 done、每作业每会话恰一次、boot/前台对账收口]) |
| UN 绑定(App 目标) | `ios/app/MTDSS/Support/LocalNotify.swift`(`UNLocalNotifier`:首次通知时请求授权恰一次、前台 willPresent banner;Kit/host 测试零通知权限依赖) |
| 传输列表页 | `ios/app/MTDSS/Transfers/TransfersView.swift`(分段 Picker、徽章行[接收蓝/发送绿/调度 indigo]、细进度条+速度+百分比、红因、绿勾+时间、自绘左滑 `SwipeableRow`、`···`清除已完成、「N 项」沉底、动作失败横幅) |
| 标签接线 | `AppTab.swift`(`.transfers` → `TransfersView()`) |
| App 级装配 | `MTDSSApp.swift`(AppModel:notifier 注入[缺省 UNLocalNotifier]、monitor 环挂 announcer、`announceCompletion`[transfer.list 现拉任务名组文案]、**realignOnActive**) |
| **T6-minor3 携带** | `realignOnActive()`(scenePhase .active → transfer.list 快照重置 TransferActivityTracker 校正 transferActive + 通知去重集对账);`MTDSSApp` scenePhase onChange 接线;TransfersView 同拍重拉列表 |
| 测试 | `MTDSSKitTests/TransfersViewModelTests.swift`(15 例,先红后绿) |
| 截图 | `artifacts/mtdss-t9-{transfers-empty,progress,upload-percent,done-notify-prompt,interrupted,resumed-done,clear-refusal-banner}.png` |

## 2. 关键设计

- **数据面**(镜像 android TasksReducer/TasksViewModel 已裁定语义):transfer.list 全量重置(daemon 快照 job_seq 升序 → **反转即最新在最上**)+ event.transfer 增量(原位合并,name/src/dst 事件帧不带保留前值;**终态粘滞**迟到帧丢弃;**会话行[incoming]终态即摘行**;新行插顶并触发补拉补 name/src/dst)。
- **分段过滤**:全部/接收(upload 除外)/发送(upload)/调度(remote_copy);「接收」含被动 incoming(android FILTERS 同口径);顺序按 spec §4.5 全部/接收/发送/调度。
- **左滑动作集按状态**(VM `TransfersFormat.actions`,测试钉死):进行中(queued/running/paused/publishing)→ `[取消]`;中断/失败 → `[重新发起, 清除]`;已取消 → `[清除]`;**完成 → 无动作**(见 §3 契约发现);会话行终态无动作(防御,行本已自摘)。**同签名在途抑制**(同 kind+src+dst 在途行存在 → 终态动作隐藏,防双发坏对端 .part/毁续传位图;android 同口径)。
- **左滑自绘**:T5 `SinkingFooterList` 是 ScrollView+LazyVStack,系统 `swipeActions` 仅 List 可用 → 轻量自绘 `SwipeableRow`(内容层拖拽位移、过半吸附、动集变化复位、busy 置灰)。钮色:重新发起蓝/清除红/取消灰(mockup transfer-list)。
- **通知文案**:标题「传输完成」;body = 任务名(事件帧不带 name → 完成时 transfer.list 现拉一次,取不到回退「传输任务」;多文件「 · 共 N 项」)。
- **方向设备文案**:src/dst 设备段解析(upload dst=`{to}:{dir}`、download src=`{dev}:{paths}`、remote_copy 两侧、incoming src=裸 id;首冒号前切段,Windows 盘符冒号不受影响)+ 设备目录名解析(event.devices + 一次现拉喂入,缺名回退裸 id,缺面「…」);本机恒「本机」。
- **速度/进度**:`speedText`(0 → 「0 B/s」;<1024 整数;1024 进制一位小数,android speedText 同口径);`progressRatio` bytesTotal≤0 → nil(下载/来件契约恒未知 → **不确定态细进度条 + 状态词**,上传有预扫总量 → 徽章色填充条 + 百分比,E2E 双形态均有截图)。
- **DEBUG 钩子报备**(全部 `#if DEBUG`,simctl 无触点驱动):`MTDSS_DEBUG_T9="resume"|"clear"`——等中断/可清行出现后调 `vm.reinitiate`/`vm.clearCompleted`,**与左滑「重新发起」/「···」清除已完成同一 VM 入口**;手势与菜单本身无模拟器驱动,动作链路经此实跑。

## 3. ⚠️ 契约发现:引擎拒清 done(spec §4.5 需收敛,建议记票)

简报/swipe 映射初版按字面实现「完成/取消终态 → [清除]」「清除已完成 = 清 done」。**E2E 实测引擎 `transfer.clear` 对 done 记录直接拒绝**:

```
已完成任务无可清除面: 9fc5d29d223de9559101b40527167a83:1
```

(crates/daemon/src/jobs.rs `Jobs::clear` 首门:`rec.state == "done"` → Err。语义:完成记录=历史面,收件终件属用户文件不可经「清除」触碰;清理面只对中断/失败/已取消开放。desktop/Android actionsFor 的 done 本就无动作。)

**本实现已按引擎契约收敛**(91bcb69):done 行无左滑动作;「···」清除已完成 = 清**已结束且有清理面**的行(中断/失败/已取消),无可清行时菜单项禁用;完成行是永久历史。E2E:中断行经清除出列,done 行原样保留。错误面上浮横幅实测有效(`mtdss-t9-clear-refusal-banner.png` 即初版发 done 清除被拒的真实横幅)。

**留票建议**:①spec §4.5 文字按引擎语义修订(「清除」限中断/失败/已取消);②产品缺口——done 记录永久累积无处安放(引擎可考虑「删记录不触文件」的 done 清除,或 UI 侧历史折叠/筛选)。

## 4. TDD 轨迹与验证证据

1. **红**(6cba8d3):15 例先行 → 编译红(TransfersViewModel 不存在)。
2. **绿**(653fa4e..cd582d7):实现落地;`swift test` = **87/87,0 失败**(72 既有 + 15 新;终版 91bcb69 复跑确认 87/87)。
3. **sim 档**:`ios/build.sh sim` exit 0(引擎静态库同源构建;xcodegen;xcodebuild;安装启动)。
4. **真传输 E2E**(产线信令;Windows dev daemon A = T6-Windows-A 44d97f96,home C:\mtdss-t6a 复用已登录态,收尾已杀;sim = iPhone 17 Pro Max 319AAA16,fresh 设备 + `SIMCTL_CHILD_MTDSS_HOME=~/mtdss-t9/home`,smoke-login 真登录 → device 9fc5d29d):

| # | 流 | 证据 |
|---|---|---|
| ① | A→sim 下载 600MiB(t9-big.bin) | 进行中行:接收蓝徽章 + 「T6-Windows-A → 本机 · 151.7 MB/s」+ 不确定态细进度条 + 「进行中」(`mtdss-t9-progress.png`);终态 done;**md5 e4d6540f… = A 源**;完成行绿勾 + 会话内观测时间「11-01 06:44」(`mtdss-t9-done-notify-prompt.png`) |
| ② | sim→A 上传 500MiB(t9-send.bin) | 发送绿徽章 + 「本机 → T6-Windows-A」+ 百分比行(`mtdss-t9-upload-percent.png`);done;**md5 d8b61b2c… = Mac 源** |
| ③ | **杀 App 中断**(下载 600MiB 传中 `simctl terminate`,简报指定的可控中断法) | 重启 → 中断行:红字「**已中断**」(引擎 error 空 → 兜底文案,简报语义实跑)+ 最新在最上 + 与 done 行同屏(`mtdss-t9-interrupted.png`) |
| ④ | **重新发起续传**(DEBUG_T9=resume,同左滑入口) | 中断记录翻新 → 位图续传 → done;**md5 e4d6540f… = A 源**(续传后逐字节一致);三行全勾(`mtdss-t9-resumed-done.png`) |
| ⑤ | **清除已完成**(DEBUG_T9=clear,同「···」入口) | 新中断行(t9-big3)清除出列,done 行不动;A 侧重投的来件卡 queued→done 后**自行摘行**(会话行终态即消失的活体实证),md5 e4d6540f… = 源 |
| ⑥ | 通知授权流 | 首个完成瞬间弹「MTDSS 想给你发送通知」授权框(= 设计的首次通知请求时机,截图 ①) |
| ⑦ | 中继角标 | relay=true 行副文附「· 中继」(事件面实测出现在 A 侧中继兜底时段,无新 UI 元素) |

5. **空态/分段**:空列表 ContentUnavailableView + 四段原生 segmented(`mtdss-t9-transfers-empty.png`);分段过滤逻辑 VM 测试钉死(模拟器无触点,同 T5 先例)。

## 5. Concerns(诚实记录)

1. **通知横幅视觉未在模拟器取证**:授权框无法无触点应答(headless sim 无 GUI 窗口、ssh 无辅助功能权限、TCC.db 预插在 iOS 26 sim 对 usernoted 无效、重装/抹机均清不掉 pending 提示)——授权后横幅呈现(banner+sound,前后台同语言)留 **T13 真机**验证,与 T6 isIdleTimerDisabled 同先例。判定面/请求时机/去重面已 Kit 测试 + 授权框截图覆盖。
2. **spec §4.5 与引擎契约分歧(§3,建议记票)**:「清除(终态)」「清除已完成」按引擎语义收敛实现;done 记录永久累积的产品缺口待裁(引擎 done 清除面 or UI 历史折叠)。
3. **bridge 形态上传(分享扩展源)的重新发起引擎必拒**(src_paths 为 `bridge://{ref}`,daemon 预扫全 missing → 「无可传内容」,原记录保留)——错误经横幅上浮;Android 有 ReinitiateRouter App 侧重建,iOS v1 未做(票候选;T12 分享扩展落地后更相关)。
4. **左滑手势本体**无模拟器驱动(代码评审 + VM 动作集测试 + DEBUG 钩子同入口实跑);真机交互归 T13。
5. **scenePhase 重对齐**(T6-minor3)代码面完成;headless 无法驱动前台切换,真机 T13 顺手过(传输中切后台回前台看 transferActive/列表收敛)。
6. **通知去重粒度 = 会话级**:每 App 会话每作业至多一条;跨会话的历史完成不通知(boot 对账);「每次进入前台不重提」同口径。若产品要「每条完成都提示(含重启后补)」需另裁。
7. 「N 项」沉底计数 = 分段过滤后的可见行数(与列表一致;spec 未细分)。
8. 模拟器冒烟中两次 `simctl erase`(清系统残留的未应答授权提示,重装无效)——共享 dev sim(319AAA16)上 T6-T8 遗留 app 状态被清,无依赖(各任务均自带安装流程);产线账号新增一台「iOS 模拟器」离线残项(9fc5d29d,与历次同形态,无害)。
9. dev 物料收尾:A(pid 56976)已杀;`C:\mtdss-t6a` 与 `~/mtdss-t9` dev home 保留(同 T6/T7/T8 先例),大测试文件双侧已删;**用户产线 mtdssd(pid 58416)全程未触碰**。

## 6. Self-review 记录

- 初版按简报字面实现 done → `[清除]`,E2E 撞上引擎拒清 → 全链修正(VM/视图/测试)并留票(§3);教训:swipe 映射这类「引擎可拒绝」的动作集,brief 阶段就该对 crates 侧资格门。
- `announce` 的 boot 对账:先按「引擎 boot 重放存量帧」假设写注释,nc 实测**连接不重放**(0 帧)→ 注释改为竞窗防御语义,代码保留(reconcile 对迟到通道仍是正确防线)。
- 测试脚手架三轮编译修(嵌套类 static 引用、pattern 可选性、ScriptBox 数组便利构造);`clearCompleted` 首版测试数据全 nil src/dst,误触同签名在途抑制——**抑制行为本身是对的**,改测试数据带签名,顺势多钉一条异签名不抑制断言。
- `TransfersFormat.stateText("failed")` 保留「保存失败」词面分叉(android 同源,发布失败场景 T8 面)。
- 相对 BASE 净 diff:7 files,+1467/−2(含 430 行测试)。

## 7. 提交清单(6cba8d3..cbac8e2)

| commit | 内容 |
|---|---|
| 6cba8d3 | test: VM 测试先行(红) |
| 653fa4e | feat: Kit TransfersViewModel + CompletionNotify |
| 1734fb1 / 22bbaae / af1d8ea / cd582d7 | 绿化途中的编译/脚手架修正(hasInFlightSibling 标签、ScriptBox 便利构造、外层 static 引用、签名化测试数据) |
| 160e8d2 | feat: 传输页视图 + UN 通知 + scenePhase 重对齐 + 标签接线 |
| cb0f303 | fix: 完成通知 boot 快照对账(历史完成不通知) |
| 91bcb69 | fix: **引擎契约收敛**——done 不可清;清除已完成指向中断/失败/已取消 |
| cbac8e2 | docs: boot 对账注释修正(连接不重放帧) |
