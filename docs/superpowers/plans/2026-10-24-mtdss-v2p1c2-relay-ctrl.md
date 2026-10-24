# MTDSS v2-P1c 实施计划:客户端中继兜底(第二刀·ctrl 面)

> **For agentic workers:** REQUIRED SUB-SKILL: Use superpowers:subagent-driven-development (recommended) or superpowers:executing-plans to implement this plan task-by-task. Steps use checkbox (`- [ ]`) syntax for tracking.

**Goal:** ctrl 两点位(`CtrlPool::punch_via_hub` 发起方 / `respond_session` 响应方)接入第一刀落地的 connect-with-fallback——远端浏览/编排/作业查询在直连穷尽且资格在手时自动走服务端中继;远端拉取(download)全链中继以 e2e 钉死;nat-matrix 增 ctrl 中继场景。

**Architecture:** 零新增抽象:两点位把 punch 段的 `punch_connect` 换成 `fallback::connect_with_fallback`(角色分别 `Initiator`/`Responder`),`ctrl_punch()`(CTRL_TUNING:无 keep-alive + 60s 空闲)原样下传——MTU 1400 覆写与 PunchConfig 整份克隆已内聚在第一刀修复轮的 `fallback::relay_dial`(交接段提醒由此吸收,ctrl 侧无需自置)。错误面沿用 `CtrlErr`:Direct→Punch(今日 punch_err 口径)、RelayDenied→Internal(denied_message 终局文案)、RelayFailed→Punch(可重试)。注错面不新增:ctrl 两侧分别读 `state.send_opts()/recv_opts()` 的 `fault_force_relay`(启动级注入,沿 `start_with_force_relay` 先例)。服务器/proto/ui 零改动。

**Tech Stack:** 既有栈;无新依赖、无 proto 变更、无服务端变更。

**Spec:** docs/superpowers/specs/2026-10-22-mtdss-v2p1-relay-design.md(§2 目标 1/3、§6.1 裁定 C/G/I、§7.3 四点位统一、§7.4 ctrl 场景、§8)

**范围裁定(spec §8 授权两刀,本计划为第二刀):**
- **响应方配速 = 零代码,验证性钉死**:交接段预期「远端拉取场景数据发送方在响应侧需按自身资格配速」,核对代码后成立的是——download 的数据连接由**源端 upload 驱动**发起(`jobs.rs` download 驱动 → orchestrate → 源端 `upload_driver` → `run_send_with` → `send_round`),即数据发送方恒为**传输面 Initiator**,其兜底+按分配速率配速第一刀 T5/T6 已接线;ctrl 响应侧不承载批量数据(浏览 RPC 小帧,不配速)。Task 2 的 download 全链 e2e 即该项的验证载体。
- **MTU/克隆交接提醒已被第一刀修复轮吸收**:覆写在 `relay_dial` 内整份克隆 PunchConfig(端点+拨号双保险),ctrl 接入传 `ctrl_punch()` 即得,不双写。
- **拒 6 可重试**(修复轮裁定)沿用:ctrl 侧 RelayFailed 承接,不走终局。
- ctrl 会话死亡(裁定 C 服务端杀)无客户端新面:QUIC 断连按既有死链路径显形(池 `closed()` 惰性驱逐 → 下次 RPC 整程重拨)。

## Global Constraints

- 门禁:每任务提交前本机 `MTDSS_UPDATE_URL=https://mtdss-update.invalid cargo test --workspace` + `cargo clippy --workspace --all-targets -- -D warnings` 全绿(本机 shell 可能 export 真实 MTDSS_UPDATE_URL,必须前缀覆盖);WSL 双平台 + nat-matrix + ui 由 PR Linux CI job 承担(沙箱拦 netns,既定裁定)
- 值钉死(全部第一刀已落地,ctrl 沿用不自造):响应方等推送窗口 `RELAY_PUSH_WAIT = 5s`;分配超时 10s;`RELAY_MAX_MTU = 1400`(`relay_dial` 内统一覆写);`CTRL_TUNING` 直连面不动(无 keep-alive + 60s 空闲 + 1452)
- 文案钉死:终局 4→「中继未开通」5→「中继配额已耗尽」(经 `fallback::denied_message`);日志锚「中继兜底已建立」复用同一行(nat-matrix grep 锚,ctrl 路径同打)
- ctrl 错误映射钉死:`FallbackErr::Direct` → `CtrlErr::Punch`(flow::punch_err 今日口径,NAT 标签/候选数);`RelayDenied{code}` → `CtrlErr::Internal(fallback::denied_message(code))`(交接段「沿用 CtrlErr 类型化」,RPC 码位映射 `CtrlErr::rpc` 现状不动);`RelayFailed` → `CtrlErr::Punch`(可重试语义:下次 RPC 自然重拨)
- 未开通账号行为与今日一致(资格 disabled → 直连穷尽 → Direct → 今日 punch_err 文案,不误报中继)
- 注错面不新增:ctrl 发起方读 `state.send_opts().fault_force_relay`、响应方读 `state.recv_opts().fault_force_relay`(`Daemon::start_with_force_relay` 启动级注入既有面)
- 服务器/proto/ui 零改动;配额/计费面沿服务端现状(ctrl 会话流量自然计入 relay_usage)
- 注释中文、密度沿 crate 现状;无 clap;nat-matrix 本机不可复跑,`bash -n` 自检 + PR Linux CI job 为准

## Review Focus

1. **ctrl 池内中继连接的死亡检测与驱逐重拨**:无 keep-alive 下死链靠 `conn_alive`/`send_request` 超时显形 → 驱逐 → 整程重试 → 重新兜底遇拒即终局文案。钉:T1 `ctrl_relay_conn_death_then_denied`(disable 杀活跃会话 → 下一笔 RPC 在 30s 内以 code 1 + 「中继未开通」收束)
2. **ctrl 响应侧推送竞态**:响应方订阅必须先于 CtrlAccept 回信(发起方 Accept 之后才可能直连穷尽→分配→推送)。结构性闭合(订阅在 respond_session 开头、Accept 在其后);钉:T1 `ctrl_browse_via_relay`(forced 发起方 Accept 后立即分配,时序最紧形态)
3. **调优语义漂移**:中继路径必须保留 ctrl 的 60s 空闲/无 keep-alive(否则池语义变脸),MTU 覆写不得手工双写(漂移风险)。结构性闭合(`relay_dial` 整份克隆 `ctrl_punch()` 只改 max_mtu);最近钉:T1 复用断言(第二次 list 走池内中继连接)
4. **生产未开通路径**:资格 disabled 时不得误报中继文案、不得尝试分配(资格门在 `connect_with_fallback` 直连穷尽分支内)。生产路径(直连穷尽)loopback 不可复现,结构性由第一刀 fallback 资格门钉住;服务端实时拒 4 面由 T1 denied 用例钉死
5. **计费面**:ctrl 会话流量计入 relay_usage 且不影响容差断言(128KiB 载荷下 ctrl QUIC 开销 ≪ 25% 余量)。钉:T2 `download_via_relay_full` 的 usage ≥ 文件字节且 ≤ 1.25×

---

### Task 1: ctrl 两点位接入 connect-with-fallback

**Files:**
- Modify: `crates/daemon/src/ctrl.rs`(`punch_via_hub` 发起方 + `respond_session` 响应方;`CTRL_TUNING`/窗口常量注释收口;模块头补一行)
- Test: `crates/daemon/tests/relay_e2e.rs`(追加)

**Interfaces:**
- Consumes: 第一刀 `crate::fallback::{connect_with_fallback, denied_message, FallbackErr, FallbackRole}`(`connect_with_fallback(hub, relay_rx, me, peer_id, peer_pk, direct, peer_candidates, punch, role, force_relay)`);`SignalHub::relay_events()`;`DaemonState::{send_opts, recv_opts}`
- Produces(T2/T3 依赖):ctrl 通道全 RPC 面(list/stat/delete/move/copy/orchestrate/cancel_job/job_query——共用 `CtrlPool::request`)在直连穷尽且资格在手时自动走中继;`respond_session` 打通行 `ctrl 会话已打通(from={from}, relay={bool})`

- [ ] **Step 1: 写失败测试**(relay_e2e.rs 追加;RED 判据 = 中继判别面 relay_usage,同机直连必通下唯一能分路的证据)——
  - 辅助 `call_err(client, method, params, t) -> (i64, String)`:限时 IPC 调用断言 Err,返回 RpcError 的 (code, message)
  - `ctrl_browse_via_relay`(`rlyctl@x.y`,Pair::new_forced):B 侧 home 造文件 → `ca` 调 `list`(device=dev_b, path=home_b)返 entries 含文件名;**第二次 `list` 同结果**(池内中继连接复用,死链驱逐语义不动);`stat` 单条查询同通;`wait_until(20, relay_usage_bytes ≥ 1)` 断言确经中继(实现前 ctrl 走回环直连,usage 恒 0 → RED)
  - `ctrl_relay_conn_death_then_denied`(`rlycd@x.y`):browse 成功后 `signaling::admin::relay_set_enabled(db, email, false)`(裁定 C:杀活跃会话)→ 下一笔 `list` 经 `call_err(ca, …, 30s)`:code == 1 且 message 含「中继未开通」(死链 15s 显形 → 驱逐 → 整程重试 → force 分配 → 实时门禁拒 4;实现前直连连接不受 disable 影响、list 仍成功 → RED)
- [ ] **Step 2: `cargo test -p mtdss-daemon --test relay_e2e ctrl_` 确认失败**(两用例按上述判据红;其余既有用例不动)
- [ ] **Step 3: 实现**——
  - `punch_via_hub`:`hub.subscribe()` 后并列 `hub.relay_events()`(None → 既有 PeerOffline 同文案);punch 段 `punch_connect` 换 `fallback::connect_with_fallback(hub, &mut relay_rx, &me, device, &peer_pk, &ep, &theirs, ctrl_punch(), FallbackRole::Initiator, state.send_opts().fault_force_relay)`(hub 即 `state.hub()` 返回的 `&SignalHub`,与 flow.rs 调用同形),错误三臂按全局约束映射,Ok 取 `(ep, o.conn)`(端点照旧入池保活)
  - `respond_session`:`hub.subscribe()` 后并列 `relay_events()`(None → warn 弃会话);punch 段换 `FallbackRole::Responder` + `state.recv_opts().fault_force_relay`;Ok 臂日志 `ctrl 会话已打通(from={from}, relay={})`(o.relay.is_some());Err 三臂(Direct 用 punch_err 全链,解包 `_nat_symmetric`/`mine`)均 warn 弃会话
  - 注释收口:`CTRL_TUNING` 注释改「直连 1452;中继路径由 fallback 的 relay_dial 克隆 PunchConfig 覆写 1400(端点+拨号双保险),keep-alive/空闲语义两路一致」;CTRL_ACCEPT_ATTEMPTS 的 ≤30s 有界注释补兜底最坏上界(直连 10s + 分配 10s + 互拨 10s,值均不动);模块头补一行 v2-P1c 第二刀兜底说明
- [ ] **Step 4: `cargo test -p mtdss-daemon --test relay_e2e` 全绿**(既有用例零改动通过)+ 全仓门禁(clippy 含)
- [ ] **Step 5: Commit** `feat(daemon): ctrl planes connect-with-fallback (relay fallback for remote browse)`

### Task 2: download 全链中继 e2e(远端拉取旗舰场景 + 响应方配速验证)

**Files:**
- Test: `crates/daemon/tests/relay_e2e.rs`(追加)

**Interfaces:**
- Consumes: T1 ctrl 兜底;第一刀传输两点位兜底 + T5/T6 配速 + `Daemon::job_relay` 测试面;IPC `transfer.start {kind:"download"}` / `transfer.wait` / `transfer.list`
- Produces: 交接段「响应方配速」项的验证载体(范围裁定:零代码,源端数据面恒 Initiator 已覆盖;本用例证明全链组合成立并钉死回归)

- [ ] **Step 1: 写验证用例** `download_via_relay_full`(`rlydl@x.y`,Pair::new_forced):B 侧 home 下造 128KiB 文件(fill_random_file 留字节)→ `ca` 调 `transfer.start {kind:"download", device: dev_b, paths:[…], dir: a 侧临时目录, skip:[], overwrite:[], cut:false}` → `transfer.wait(id, 60s)` 断言 done;a 落盘字节一致;a 下载卡 `daemon_a.job_relay(id) == Some(true)`;B 侧回显 upload 卡(cb transfer.list 轮询 15s 至 done)`daemon_b.job_relay == Some(true)`;`relay_usage ≥ 文件字节 && ≤ 1.25×`(ctrl 会话 QUIC 开销计入,128KiB 余量充足;沿第一刀修复轮收紧值)
- [ ] **Step 2: 运行并核对**——`cargo test -p mtdss-daemon --test relay_e2e download_via_relay_full`:组合验证用例(无新实现),**先跑即绿是预期**(ctrl 兜底 + 传输兜底 + 配速均已在位);若红 = 全链有回归,进 systematic-debugging 定因,不放宽断言
- [ ] **Step 3: 全仓门禁绿**
- [ ] **Step 4: Commit** `test(daemon): remote-pull full relay e2e (download over ctrl+transport relay)`

### Task 3: nat-matrix ctrl 中继场景 + 部署文档

**Files:**
- Modify: `scripts/nat-matrix/run_case.sh`(NM_CTRL=1 门控:presence 后、send 前远端浏览步)
- Modify: `scripts/nat-matrix/matrix.sh`(追加用例 + 计数注释)
- Modify: `scripts/nat-matrix/README.md`(NM_CTRL 说明 + 中继节/计数)
- Modify: `scripts/deploy/README.md`(客户端行为行补远端浏览)

**Interfaces:**
- Consumes: T1 行为与「中继兜底已建立」锚;CLI `mtdss ls <device> <path>`;既有 NM_RELAY 门控(lib.sh 不动)
- Produces: 矩阵 ctrl 中继用例(`NM_RELAY=1 NM_CTRL=1 symmetric symmetric ok`,ctrl 兜底路径唯一的真实二进制覆盖)

- [ ] **Step 1: 实现**——run_case.sh 在 ④ presence 后、⑤ send 前插 NM_CTRL=1 步:`timeout 60 ip netns exec $NM_NS_A env MTDSS_HOME=$H_A "$MTDSS_BIN" ls "$DEV_B" "$PAYLOAD_DIR"`(输出落 `$CASE_DIR/ls_cli.log`,失败转储);断言 exit 0 且输出含 `f1.bin`;NM_RELAY=1 时此刻断言 `$LOG_A` grep「中继兜底已建立」(send 未跑,锚行只能来自 ctrl 兜底——钉死远端浏览确经中继);matrix.sh 追加 `run_ctrl_relay`(NM_RELAY=1 NM_CTRL=1 symmetric symmetric ok,置于 run_relay 后),头注与 README 用例计数同步;README 补 NM_CTRL 行;deploy README「客户端行为」行补远端浏览同样兜底
- [ ] **Step 2: `bash -n` run_case.sh matrix.sh 通过 + README/注释与实际用例一致**(本机沙箱不可复跑 netns,以 PR Linux CI job 的 nat-matrix 为准——沿 P1b/T8 既定裁定)
- [ ] **Step 3: Commit** `test(nat-matrix): remote browse relay case (NM_CTRL=1)`
