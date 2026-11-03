# Task 5b 报告 — 契约与语义四件套(Ruling H/J/K;执行中裁定并入)

日期:2026-11-01 · 分支 `worktree-worktree-mtdss-v2p5-ios` · 基线 a56d69f · **commits eb4fd97..9574513(11 个)**

**状态:complete** — 四件全部落地:①list 目录项 children 计数三端一次做(Ruling H);③delete golden 三端绑定;②unix 收端哨兵回退(Ruling J,红→绿 e2e 实证);④transfer.clear 放开 done + T9 完成行动作恢复(Ruling K)。三端验证全绿(细节见 §4);模拟器冒烟过(目录行「N 项」+ 清除已完成全链,截图三帧)。

## 0. 四件落法(对应 brief Step 1)

| 件 | 引擎 | 契约/golden | Kotlin | Swift/iOS |
|---|---|---|---|---|
| ① list children | proto `DirChildren{count}` 包裹消息(DirEntry 字段 6,只增不改号);core `fs::list_dir` 对目录项 read_dir 一次计数(**全量含 dotfiles/`.mtdss` 保留名——文件系统事实,显示过滤归 UI**;无递归;读失败 None 不编造;stat 单查恒不计数) | 构造帧样本 golden `list.resp.entries.json`(目录项带 children / 文件项整键缺省,同 status.resp.android 的桌面构造手法);REGEN 零既有漂移 | `DirEntry.children: Long?` + `asLongOrNull`;形态样本测试消费 | `DirEntry.children: Int64?` + `optInt64`;形态样本测试消费;**BrowseView 行副文接线 `subtitle(childCount: entry.children)`**——「日期 - N 项」(T5 预留形收口);android 仅契约面,UI 零变化 |
| ③ delete golden | 契约场景增 delete req/resp(对端离线 code 3 错误契约形态,与 list/stat 同理;参数面 device/paths/force req 帧逐字节钉死) | `delete.req.json`/`delete.resp.json` 入 REGEN | 声明**不移植**(安卓无远端删除面,沿 remote_copy 先例) | **表单化绑定**(MtdssRepository.delete 字节对齐;本机删除 BrowseViewModel.deleteSelected 同一门面) |
| ② unix 收端哨兵回退 | `DaemonOpts.inbox_publish`(缺省 = android/iOS)→ DaemonState 单一真源;flow 常驻接收哨兵路由加发布面资格门——unix 桌面(Linux/macOS)`dir="inbox"` 按非法回退默认收件目录(与 Windows 同语义),不再进暂存/发布查询超时路径 | 无协议变更(Ruling J「引擎侧统一」,惠及全部发送方) | 无(安卓缺省 true,零变化) | ios-bridge 注入 true(iOS 哨兵路由 InboxDestIo 不变) |
| ④ clear-done | jobs `clear()` 摘「已完成任务无可清除面」门——终态均可清,残留回收与既终态同规则(done upload 对端臂离线仍 PeerOffline 保留可重试);live 态门原样;transfer.list 记录保留直至清除(无自动清) | transfer.list/clear golden 零漂移(fixture 无 done 记录;id=2 cancelled 清除面不变) | 无(Android 面零变化,另行任务) | `clearableStates += done`;完成行左滑恢复 [清除];「···」清除已完成 = 全部有记录面终态;VM 测试按新语义反转 |

## 1. 交付面(11 commits)

| commit | 件 |
|---|---|
| eb4fd97 | feat① list children 计数——proto+core fs+ctrl+api+契约场景+Kotlin/Swift 契约面+BrowseView 接线+三端测试 |
| 0a5dd13 | feat③ delete golden——daemon 场景 + Swift 表单化绑定 + Kotlin 不移植声明 |
| 4c88537 | chore(contract) **REGEN 独立提交**:`list.resp.entries.json` + `delete.req/resp.json`(既有 golden 零漂移) |
| 543eb68 | wip② TDD 红态脚手架:inbox_publish 字段 + 桌面回退 e2e(未过门,红) |
| 3c01b8a | fix:children 用消息包裹在场语义(protoc 3.12 兼容——WSL 打包工具链 proto3 `optional` 需 ≥3.15,本机 WSL 3.12.4) |
| d89c3f5 | feat② 哨兵路由资格门(flow.rs)+ 全测试台 opts 显式化 + 新 e2e 绿 |
| 6918041 | feat④ clear-done 引擎松绑 + T9 完成行动作恢复 + VM 测试反转 |
| 4122d6d | fix:Swift 形态样本测试 ListResult(.object(result)) 包裹(字典→JSONValue) |
| 229896a | fix:Kotlin JUnit4 断言序(message 恒首位)+ 模拟器冒烟截图三帧 |
| 40ad0cb | fix:DaemonState::load too_many_arguments allow + inbox_publish unix 门外 allow(dead_code)(CI clippy -D warnings 对齐) |
| 9574513 | fix① 根卷条目不计 children(网络映射盘 read_dir 停顿回归,详见 §6.1) |

## 2. 关键设计/语义决策

- **children 的 proto 形态**:brief 口径「optional 字段」落为**包裹消息** `DirChildren{count}`——proto3 标量 `optional` 需 protoc ≥3.15,本机 WSL 3.12.4(打包轮的安卓/Linux 产物构建环境)直接编译红;消息字段天然带在场语义,prost 侧同样是 `Option<u32>`,Rust/JSON/客户端面与 optional 方案逐字节同形。空目录 = `Some(0)` 在场(「0 项」照显);文件/未知 = 缺省。
- **children 计数口径(裁定留痕)**:read_dir **全量计数,含 dotfiles 与 `.mtdss`/`.mtdss-part` 保留名**——文件系统事实,与 list 显示面的保留名过滤刻意不同口径;「N 项」的显示过滤归 UI 侧。核心 fs 单测钉死(4 件目录含 `.mtdss` 计 4,无递归)。
- **②的「无 App 发布面」判定**:编译期缺省(android/iOS = 有面,其余 = 无)+ `DaemonOpts` 显式注入——P4a/T8 的 Linux-host 收件箱套件(伪 App 发布桩)显式 `inbox_publish: true` 钉移动语义,新 e2e 以 `false` 钉桌面回退语义;生产 main 随编译目标。零 env 旋钮(沿 start_with_force_relay/inbox_staging 注入先例,不用 MTDSS_TEST_* 进程全局面)。
- **④ done 清除的残留语义**:done 记录与 failed/cancelled 走**同一**清除路径——download 本地清 `.mtdss`/`.part`(用户已收文件不触碰);upload/remote_copy 仍需对端 cleanup_outbox 回执 Ok(done 的对端残留通常已消费,cleaned=0 照常 Ok;**离线仍 PeerOffline 保留可重试**——与 spec「各清各的」一致,不因 done 特殊化)。
- **「download-inbox 桌面拒收」golden/发启侧零变化**(brief 明示):动的是收端解析,`transfer.start.download` 的 dir 校验(inbox_dir_ok)原样。

## 3. TDD 轨迹

- ①:红 = 形态样本 golden 缺失(三端测试引用即红)→ REGEN 后绿;core fs 计数单测 + api 投影单测先红后绿(Kotlin `assertEquals` JUnit4 message-首位序、Swift `.object(result)` 包裹两处编译红即修)。
- ③:红 = daemon 契约测试缺 golden + Swift 孤儿守卫(声明消费面文件缺失)+ Kotlin notPorted 声明缺;REGEN 后三端绿。
- ②:**行为红实证**(WSL):`sentinel_to_desktop_receiver_falls_back_to_recv_dir` 未过门时 95s 红——来件走暂存/发布路径卡终败(发布查询超时);过门后 6.3s 绿——recv_dir 正身逐字节一致、卡 done 无终因、暂存区零记录。
- ④:红 = jobs `clear_gates_done_missing_and_empty_rels`(旧钉 done 拒清)+ Swift `testActionsByStateThreeWays`/`testClearCompletedClearsOnlyEndedClearableRows`(旧钉 done→[]/3 面)→ 按新语义反转(done upload 离线 = PeerOffline 保留;done 空残留直接删;done 行 [清除];clearCompleted 5 面)。

## 4. 三端验证(Step 2)

| 端 | 面 | 结果 |
|---|---|---|
| Windows | `cargo test -p mtdss-daemon` 全套 @9574513(终跑,含契约 golden/REGEN 稳定) | **全目标绿**;daemon_e2e 24/25 在套件内,唯一败面 `restart_marks_interrupted` 为进度预算型抖动——两平台(Win/WSL)隔离复跑均绿(Win 33.3s / WSL 26.5s),非回归 |
| Windows | `cargo clippy -p mtdss-daemon --all-targets` @40ad0cb | 0 warning(先修 2 处 dead_code + 1 处 too_many_arguments) |
| Windows | `cargo test -p mtdss-core --test fs` | 24/24(含 children 计数/根卷 None 新钉) |
| WSL | 全 daemon 套件 @9574513 | **全目标绿,零失败**(§6.3) |
| WSL | 新 e2e `sentinel_to_desktop_receiver_falls_back_to_recv_dir` | 绿(6.3s;红态 95s 实证走暂存保存失败) |
| WSL | inbox_staging_e2e + inbox_layout_e2e(T8/P4a 收件箱套件,`inbox_publish: true` 钉面) | 全绿(全量套件内含) |
| WSL | relay_e2e 隔离 | **13/13 绿**(全量并行跑时 2 败均为握手超时型,隔离复跑全过) |
| WSL | daemon_e2e `restart_marks_interrupted` 隔离 | 绿(26.5s;并行跑败为进度预算型,复跑全过) |
| WSL | ios-bridge `cargo test -p ios-bridge` | 10/10 绿(两 face) |
| WSL | `cargo check --workspace` | 绿 |
| Kotlin | `gradlew :app:testDebugUnitTest`(Windows 本机) | **BUILD SUCCESSFUL**(ContractGoldenTest 全量 + 形态样本 + delete notPorted) |
| Mac | `swift test` @229896a | **104/104,0 failures**(其后 3 commits 均为 Rust/测试面,iOS 面零 delta) |

## 5. 模拟器冒烟(Step 3;iPhone 17 Pro Max sim 319AAA16,Mac mini)

复用 T9 登录态 home(`~/mtdss-t9/home`,seed 两枚 done download 记录 + Documents 目录夹具;engine 为本分支构建):

- **目录行「N 项」**:`MTDSS_DEBUG_TAB=favorites + MTDSS_DEBUG_AUTOPUSH=inbox` → 收件箱列表「素材库 **2026-11-01 - 2 项**」(children 接线实证)/「说明.txt 2026-11-01 - 6 B」(文件行对照)——`artifacts/t5b-browse-children.png`。
- **清除已完成可用**:传输列表 2 done 行(绿勾,`t5b-transfers-before.png`)→ `MTDSS_DEBUG_T9=clear`(与「···」同一 VM 入口 clearCompleted)→ 列表清空(`t5b-transfers-cleared.png`);**引擎侧落盘核对:`tasks.json == []`**——T9 UI → transfer.clear → 新 clear() 全链过。冒烟后 home 已还原(T9 记录回填),sim App 已停。

## 6. 过程发现与遗留

### 6.1 ⚠️ 本任务引入并已修:根卷 children 的网络盘停顿(commit 9574513)

①首版把 child_count 也铺到根卷条目——本机实测 `read_dir("Z:\\")`(SMB 映射盘)**6.5s/次**,根卷 `""` 列表的既有语义只有 metadata 试探(毫秒级),每盘多一次枚举后累计可间歇性撞 ctrl 15s 超时:`ctrl_e2e list_remote_roundtrip` 4 跑 3 败;基线 a56d69f 同跑恒绿(5.1s)。修正:**卷条目 children 恒 None**(UI 卷行退化纯日期),子项数只在真目录浏览(`list_dir`)内计——该处共享目录本就要整体枚举,child_count 是同量级追加,固有成本随裁定接受。

### 6.2 proto3 `optional` 工具链红线(commit 3c01b8a)

本机 WSL protoc 3.12.4 不支持 proto3 标量 `optional`(需 ≥3.15)——children 落为**消息包裹** `DirChildren{count}`(在场语义同 Option,prost/JSON/客户端面逐字节同形)。**后续 proto 增量一律沿「消息包裹/缺省值」既有手法**,否则打包轮(WSL 安卓/Linux 产物)编译红。

### 6.3 WSL 终跑追记

全 daemon 套件 @9574513(空闲机):**全目标绿,零失败**——含 contract_golden、新 e2e、inbox_staging/inbox_layout(收件箱两套)、daemon_e2e(24+1)、relay_e2e(13)、daemon_basic/jobs_e2e/ctrl_e2e/login/online/tray_logic/upload_bridge 等,与 Doc-tests 收口;此前并行重载下的 5 处失败(握手超时/进度预算型)全部消失,证实为负载抖动非回归。

### 6.4 其他 Concerns / 携带

1. **ios-bridge clippy 预在红(非本任务引入)**:`cargo clippy --workspace` 于 `mtdss_ios_ipc_path`(lib.rs:293,`not_unsafe_ptr_arg_deref`,deny-by-default)报 2 error——a56d69f 基线已存在(T2 代码,本机 clippy 1.99 触发);T5b 面零贡献。建议另开票:`unsafe fn` 化或局部 allow,否则 CI clippy --workspace 恒红。
2. **Android「清除已完成」面未随 K 放开**(brief 口径:iOS T9 恢复,Android 另行任务)——引擎已可清 done,安卓 TasksReducer 的 done→无动作/清除面待后续任务对齐,否则双端行为分叉。
3. **spec 文字携带**:spec §4.1「日期 - N 项」已可全量兑现(children);§5.2「完成 | 无操作入口」与 §4.5 done 无动作表述待按 Ruling K 修订(与 T9 报告 §3 留票合并处理)。
4. **网络目录浏览的固有成本**:真目录浏览的 child_count 对每个子目录多一次 read_dir——慢共享(Z: 级)上大目录列表延迟同量级放大(裁定固有成本,v1 iOS 不涉网络盘;桌面端如遇可后续做「计数上限/懒计数」票)。
5. **`daemon_e2e restart_marks_interrupted` 预算偏紧(预在抖动,放大面)**:30s 进度预算在套件并行 e2e 负载下两平台均可抖红(本任务终跑 Win 套件内 1 次;隔离恒绿)。与 T5b 无关(send 路径零 delta),建议另开票:预算放宽或预算内重试窗。
6. T9 home 的模拟器冒烟使用了临时 seed(两枚 done download 记录),已还原;`~/mtdss-t9/Documents/{素材库,说明.txt}` 留作后续冒烟夹具(无害)。
