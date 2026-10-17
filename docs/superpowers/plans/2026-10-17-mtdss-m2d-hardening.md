# MTDSS M2d（收官硬化）Implementation Plan

> **For agentic workers:** REQUIRED SUB-SKILL: Use superpowers:subagent-driven-development (recommended) or superpowers:executing-plans to implement this plan task-by-task. Steps use checkbox (`- [ ]`) syntax for tracking.

**Goal:** 清偿 M2c 终审 RIDE ⓪-⑨（掉电安全、协议上限双侧不挂、小债、测试注入面），交付 netns NAT 矩阵 + netem 混沌 harness 并纳入 PR 必跑门禁，建立 srflx 实效断言与发布检查单。

**Architecture:** RIDE 修复全部落在既有模块（intake / transfer / endpoint / flow / cli_e2e），各自独立测试闭环；矩阵与混沌是 bash harness（`scripts/nat-matrix/`）编排五个 netns（pub/gwA/gwB/a/b），跑真实 `mtdss`/`signaling-server`/`stun-server` 二进制，iptables+conntrack+recent 模拟四种 NAT 行为、tc netem 注入损伤；harness 只能在 Linux 跑（本机是 Windows——本地验证仅 `bash -n` 语法级，真实验证全部经 CI 的 `nat-matrix` job）。注入面 = CLI 测试专用环境变量（`MTDSS_TEST_*`），经 `setup_punch_endpoint` 线穿到 accept 侧连接。

**Tech Stack:** Rust 2021（quinn/tokio/serde_json）；bash + ip netns + iptables + tc netem；GitHub Actions ubuntu（sudo 建 ns）。

**Spec:** `docs/superpowers/specs/2026-10-17-mtdss-m2d-hardening-design.md`（计划与其共同进退；④ 已按计划期发现修订为真实子进程硬杀）

## Global Constraints

- 门禁：每任务 `cargo test --workspace && cargo clippy --workspace --all-targets -- -D warnings`；Task 8 起 CI 的 `nat-matrix` job 也是 PR 必跑
- 常量逐字取自 spec：**MAX_PLAN_TOTAL_BITMAP_BYTES = 64 MiB**（③）；fsync 目录集合 = **终文件直接父目录 + `.mtdss`**（①）；`MTDSS_TEST_*` 之名固定为 **IDLE_TIMEOUT_MS / KEEPALIVE_MS / MAX_RECONNECTS / BACKOFF_MS / RECV_WINDOW_MS** 五个，不一次铺全（⑨）
- 混沌用例与硬超时初值逐字取 spec §5：loss 10%（120s）/ delay 200ms±50ms（180s）/ 中途断网 3s（120s）/ 永久断链（120s）；载荷 **4 文件合计约 12 MiB**；时间断言一律以每例硬超时为界，收尾按 ≥3× 实测中位校准（只放宽）
- 矩阵 14 例 = 6 纯锥组合 ×1 + 4 含对称组合 ×2 键序；预期与键序无关；拓扑网段用 **TEST-NET-3（203.0.113.0/24）** 做网关外地址（与私网本地候选天然可区分，srflx 断言可 grep）
- 二进制名：CLI=`mtdss`、信令=`signaling-server`、STUN=`stun-server`（workspace Cargo.toml 实名）
- `MTDSS_TEST_*` 不进用户文档与 `--help`，只记录在 `scripts/nat-matrix/README.md`（标注"测试专用、不稳定、随时可删"）
- 本机（Windows）bash：`export PATH="$HOME/.cargo/bin:$PATH"; export PROTOC="/c/Users/Administrator/AppData/Local/Microsoft/WinGet/Packages/Google.Protobuf_Microsoft.Winget.Source_8wekyb3d8bbwe/bin/protoc.exe"`
- 提交信息 conventional 前缀；代码注释跟随仓库中文风格
- 流帧格式仍不做跨版本兼容（M2c 裁定延续）

## Review Focus

1. **深层 rel 的 fsync 只打 out_dir**——目录发送时 rename 落在 `out/<子目录>/…`，fsync out_dir 在 ext4 上不能使深层 rename 持久，① 的不变式恰在 M2c 新增的目录场景静默失效。→ Task 1：`finalize_sequence_nested_rel_syncs_immediate_parent`（断言 sync-dir 集合含 `<out>/<子目录>` 而非 out_dir）。
2. **cap 拒绝被发送端误判 Network**——发送端把 STREAM_STOPPED 当网络错误进重试循环，对同一条 64MiB+ manifest 反复重发。→ Task 2：`manifest_cap_sender_gets_rejected_not_network` 断言 `SendError::Rejected` 且不消耗多轮。
3. **注入面到不了 accept 侧连接**——`setup_punch_endpoint` 若仍用 `make_dual_endpoint`（默认 30s），env 注入的亚秒 idle 只挂在拨出连接上，接收侧静默死亡检测不到、④ 的时间断言失真。→ Task 5 线穿 `make_dual_endpoint_tuned`；Task 6 用 `elapsed` 上界断言兜底（若检测靠 30s 默认值必超界）。
4. **harness 残留 netns/iptables 污染下一例**——失败例中途退出后 ns 与规则残留，后续全部用例假失败。→ Task 8：`teardown.sh` 幂等 + `setup.sh` 首步强制 teardown（每例先拆再建）。
5. **混沌紧界成 chronic flake**——10% 丢包下完成时间随运气波动，紧界必然间歇红。→ Task 10：唯一时间断言 = 每例硬超时；Task 11 收尾按 CI 实测中位校准（只放宽）。

---

### Task 1: ① intake 完成临界区有序落盘

**Files:**
- Modify: `crates/core/src/transport/intake.rs`（`finalize` 368-414、`flush_record` 337-350；新增 durability 助手与 `flush_record_durable`）
- Test: 同文件新增 `#[cfg(test)] mod tests`（单元——注入钩子依赖 `cfg(test)`，集成测试不可见）

**Interfaces:**
- Produces: `fn sync_dir(path: &Path) -> std::io::Result<()>`（模块私有；Unix `File::open(dir).sync_all()`，Windows `OpenOptions` + `custom_flags(FILE_FLAG_BACKUP_SEGANTICS=0x02000000)` 后 `sync_all`）；`async fn flush_record_durable(&mut self) -> Result<()>`（写 tmp → fsync tmp → rename → sync_dir(intake)）
- Consumes: 既有 `PartWriter.file`/`flush_record`/`unique_target`
- 钩子：`#[cfg(test)] static SYNC_LOG: std::sync::Mutex<Vec<String>>`，`sync_all`/`sync_dir` 的包装函数追加 `"sync-file {path}"` / `"sync-dir {path}"`（失败追加 `"sync-dir-failed {path}"`）

- [ ] **Step 1: 写失败测试（同文件单元）**

```rust
#[cfg(test)]
mod tests {
    use super::*;

    fn log() -> Vec<String> { SYNC_LOG.lock().unwrap().clone() }

    async fn finalize_rel(rel: &str, size: u64) -> PathBuf {
        SYNC_LOG.lock().unwrap().clear();
        let out = tempfile::tempdir().unwrap();
        let entry = /* 按 rel 构造 FileEntry：1 块、随机 sha256，写满整块数据 */;
        let mut w = PartWriter::open(out.path(), &entry, false).await.unwrap();
        w.write_chunk(0, 0, &data).await.unwrap();
        w.finalize().await.unwrap()
    }

    #[tokio::test]
    async fn finalize_sequence_toplevel() {
        let p = finalize_rel("a.bin", 4096).await;
        // 有序落盘（spec §3①）：part fsync → 改名 → 直接父目录 + .mtdss → 记录 fsync → .mtdss
        assert_eq!(log(), vec![
            format!("sync-file {}", part_path_of("a.bin")),
            format!("sync-dir {}", p.parent().unwrap().display()),
            format!("sync-dir {}", intake_dir.display()),
            format!("sync-file {}", record_tmp_path_of("a.bin")),
            format!("sync-dir {}", intake_dir.display()),
        ]);
    }

    #[tokio::test]
    async fn finalize_sequence_nested_rel_syncs_immediate_parent() {
        let out = /* finalize_rel 的 out 目录句柄 */;
        let p = finalize_rel_at(&out, "d/e.bin", 4096).await;
        let dirs: Vec<String> = log().iter().filter(|e| e.starts_with("sync-dir ")).cloned().collect();
        // 直接父目录 = out/d,而非 out;intake 仍各出现一次(rename 后与记录后)
        assert!(dirs.contains(&format!("sync-dir {}", p.parent().unwrap().display())), "{dirs:?}");
        assert!(!dirs.contains(&format!("sync-dir {}", out.display())), "深层 rel 不得打 out_dir 本身: {dirs:?}");
        assert_eq!(dirs.iter().filter(|e| e.ends_with(&format!("{}", INTAKE_DIR))).count(), 2);
    }
}
```

（`finalize_rel`/`finalize_rel_at` 里 entry/data 与 `part_path`/`record_tmp_path` 的取值由实现者用既有私有函数补全——单元测试同 crate 可见；顶层用例断言整条 5 事件序列，深层用例断言 sync-dir 集合。）

- [ ] **Step 2: 跑测试确认失败**

Run: `cargo test -p mtdss-core --lib transport::intake`
Expected: 编译失败（`SYNC_LOG`/`flush_record_durable` 不存在）。

- [ ] **Step 3: 实现**

`finalize`（368-414）在整文件哈希校验后插入：`w.file.sync_all().await?`（标注 `sync-file {part}`）→ rename（不动）→ `sync_dir(final_path.parent())` + `sync_dir(&intake_dir)`（best-effort：Err 只 `tracing::warn`）→ 记 done/final_rel → `flush_record_durable().await?` → 清理。`flush_record_durable`：`OpenOptions` 写 tmp → `sync_all` → drop → rename → `sync_dir(intake)`。周期性 `flush_record`（16 块/2s 批落盘）**不加** fsync（spec 裁定 4）。模块文档补一行有序落盘说明。

- [ ] **Step 4: 跑测试确认通过**

Run: `cargo test -p mtdss-core --lib transport::intake && cargo test -p mtdss-core --test intake && cargo test -p mtdss-core --test transfer_v2`
Expected: 全 PASS（既有 intake/transfer_v2 集成不受影响）。

- [ ] **Step 5: 门禁 + 提交**

```bash
cargo test --workspace && cargo clippy --workspace --all-targets -- -D warnings
git add crates/core/src/transport/intake.rs; git commit -m "fix(core): durable finalize ordering in intake (fsync part -> rename -> dirs -> durable record)"
```

---

### Task 2: ② manifest 超限拒绝双侧不挂（stop_stream）

**Files:**
- Modify: `crates/core/src/transfer.rs`（cap 分支 675-678；`reject_exit` 1154-1166 旁新增 `reject_with_stop`；`send_files` manifest 写段 220-228）
- Test: `crates/core/tests/transfer_v2.rs`（追加）

**Interfaces:**
- Produces: `async fn reject_with_stop(ctl_tx: &mut SendStream, ctl_rx: &mut RecvStream, reason: &str) -> anyhow::Error`——发 `[0x00][len][reason]` 后 `ctl_rx.stop(VarInt::from_u32(1))`，**不等 EOF**，返回原原因
- 发送侧：manifest 写失败为 `quinn::WriteError::Stopped{..}` → 有界（5s）读一帧，`TAG_REJECT` → `SendError::Rejected{reason}`；读不出 → 维持 `net(...)`
- 其余预检失败路径（body 已读完）保留 `reject_exit` 的 EOF 舞步，不动

- [ ] **Step 1: 写失败测试**

`transfer_v2.rs` 追加（沿用该文件既有 loopback QUIC 直连 harness）：

```rust
/// cap 分支双侧不挂:收端拒绝后 stop,发送端拿到 Stopped → 读出 reject → Rejected。
#[tokio::test]
async fn manifest_cap_rejects_without_deadlock_both_sides() {
    // 服务端真 recv_files(out_dir);客户端 raw open_bi:
    // 写 u32 len = MAX_MANIFEST_BYTES+1,再持续写垃圾(若干 MiB 后停在 write 上);
    // 断言(各 5s 有界):read 到 [0x00] + reason 含 "too large";随后 write Err(Stopped)。
}

/// 发送端分类:真 send_files 对"读 4 字节即 stop"的对端 → Rejected 而非 Network。
#[tokio::test]
async fn manifest_cap_sender_gets_rejected_not_network() {
    // 服务端:accept_bi 读 4 字节 man_len,回 TAG_REJECT reason="nope",ctl_rx.stop(1);
    // 客户端:真 send_files(build_manifest 单文件);
    // 断言 matches!(err, SendError::Rejected { reason } if reason.contains("nope"))。
}
```

- [ ] **Step 2: 跑测试确认失败**

Run: `cargo test -p mtdss-core --test transfer_v2 manifest_cap`
Expected: `manifest_cap_rejects_without_deadlock_both_sides` 挂起至超时（收端 EOF 等待/发端 write 饿死）；`..._sender_gets_rejected_not_network` FAIL（现为 `Network`）。

- [ ] **Step 3: 实现**

cap 分支（675-678）改调 `reject_with_stop`。`send_files`：把两个 `write_all` 包进小函数，`Err` 时 `matches!(e, quinn::WriteError::Stopped { .. })` → `tokio::time::timeout(5s, 读 tag+reject)`（复用 `read_len_prefixed` 的私有逻辑或内联）→ `Rejected`；注意此时尚未 spawn `read_control`，直接在 `ctl_rx` 上读。错误码常量 `1` 两端一致即可（注释标注"cap 专用 stop 码"）。

- [ ] **Step 4: 跑测试确认通过**

Run: `cargo test -p mtdss-core --test transfer_v2`
Expected: 全 PASS。

- [ ] **Step 5: 门禁 + 提交**

```bash
cargo test --workspace && cargo clippy --workspace --all-targets -- -D warnings
git add crates/core/src/transport/transfer.rs crates/core/tests/transfer_v2.rs; git commit -m "fix(core): stop control stream on oversized-manifest reject; sender maps Stopped to Rejected"
```

---

### Task 3: ③ 计划帧位图总量上限

**Files:**
- Modify: `crates/core/src/transfer.rs`（常量区 83-91 + `read_control` 502-532 + `read_frame` 544-581）
- Test: `crates/core/tests/transfer_v2.rs`（追加）

**Interfaces:**
- Produces: `const MAX_PLAN_TOTAL_BITMAP_BYTES: usize = 64 * 1024 * 1024;`；`read_frame(rx, tag, plan_total: &mut usize)`——`read_control` 持连接级累计值传入（跨帧累计，含 re-request 轮的新计划帧）
- Consumes: 既有 `MAX_PLAN_BITMAP_BYTES`（单帧 1 MiB 不变）

- [ ] **Step 1: 写失败测试**

```rust
/// 恶意计划帧总量:65 × 1MiB 位图(65 MiB > 64 MiB cap)→ Protocol,且错误点名 total
#[tokio::test]
async fn plan_total_bitmap_cap_is_protocol_error() {
    // 服务端 mock:读走真 manifest(2 文件),回 TAG_PLAN count=65,全 RESUME、每个 bmp 1 MiB;
    // 客户端:真 send_files → 断言 SendError::Protocol,reason 含 "total";
    // (当前实现会解析完 65 个再报 "plan covers 65 files"——不含 "total",测试红)
}
```

- [ ] **Step 2: 跑测试确认失败**

Run: `cargo test -p mtdss-core --test transfer_v2 plan_total`
Expected: FAIL（reason 是 cover-mismatch 文案，无 "total"）。

- [ ] **Step 3: 实现**

`read_control` 增 `let mut plan_total = 0usize;`；`read_frame` 每读出一个 RESUME bmp 执行 `*plan_total += bmp.len()`，超 `MAX_PLAN_TOTAL_BITMAP_BYTES` → `bad(format!("plan bitmap total {total} exceeds {MAX_PLAN_TOTAL_BITMAP_BYTES}"))`。回环写 65 MiB 属测试可承受（秒级）。

- [ ] **Step 4: 跑测试确认通过**

Run: `cargo test -p mtdss-core --test transfer_v2`
Expected: 全 PASS。

- [ ] **Step 5: 门禁 + 提交**

```bash
cargo test --workspace && cargo clippy --workspace --all-targets -- -D warnings
git add crates/core/src/transport/transfer.rs crates/core/tests/transfer_v2.rs; git commit -m "fix(core): cumulative 64MiB cap on plan frame bitmaps (DoS bound)"
```

---

### Task 4: ⑤⑥⑧ 三笔小债

**Files:**
- Modify: `crates/core/src/transport/endpoint.rs`（`transport_config` 263-270 + 文件尾 `#[cfg(test)] mod tests`）
- Modify: `crates/cli/src/flow.rs`（`walk_dir` 396-423；`collect_items_paths_and_errors` 1110-1169 追加断言）
- Modify: `crates/core/src/transport/punch.rs`（140-143 预算边界注释）

**Interfaces:** 无新接口。

- [ ] **Step 1: 写失败测试**

```rust
// endpoint.rs mod tests:
#[test]
#[should_panic(expected = "sub-millisecond")]
fn sub_millisecond_tuning_panics_in_debug() {
    transport_config(TransportTuning {
        keep_alive_interval: Some(Duration::from_micros(500)),
        max_idle_timeout: Duration::from_micros(500),
    });
}
```

`flow.rs` 既有 `collect_items_paths_and_errors` 追加：`dir/.mtdss/x.bin`（嵌套 `.mtdss` 目录）与 `dir/.mtdss`（文件）→ `items` 不含任何 rel 带 `.mtdss` 段的条目，其余（`dir/a.txt` 等）保留；总体非空不 Err。

- [ ] **Step 2: 跑测试确认失败**

Run: `cargo test -p mtdss-core --lib transport::endpoint && cargo test -p mtdss-cli collect_items`
Expected: should_panic 测试 FAIL（未 panic）；collect_items FAIL（`.mtdss` 条目未被跳过）。

- [ ] **Step 3: 实现**

`transport_config`：计算 `let ka_ms = t.keep_alive_interval.map(|d| d.as_millis() as u64); let idle_ms = t.max_idle_timeout.as_millis() as u64;` 后 `debug_assert!`——`idle_ms > 0` 且 `ka_ms != Some(0)`，消息含 "sub-millisecond"。`walk_dir` 条目循环里：`if name == ".mtdss" { tracing::warn!("skip reserved .mtdss entry {}", path.display()); continue; }`（文件与目录都跳）。`punch.rs` 140-143 上方补 2-3 行中文注释：`checked_duration_since` 与后续 `sleep` 之间预算可能恰好耗尽（单节拍残竞态），多睡 ≤250ms 无害——预算收尾路径已兜底，不为它引入原子时钟。

- [ ] **Step 4: 跑测试确认通过**

Run: `cargo test -p mtdss-core --lib transport::endpoint && cargo test -p mtdss-cli collect_items`
Expected: 全 PASS。

- [ ] **Step 5: 门禁 + 提交**

```bash
cargo test --workspace && cargo clippy --workspace --all-targets -- -D warnings
git add crates/core/src/transport crates/cli/src/flow.rs; git commit -m "chore: sub-ms tuning debug_assert, skip .mtdss entries in collect_items, punch budget race comment"
```

---

### Task 5: ⑨ 测试注入面与可观测性

**Files:**
- Modify: `crates/core/src/net/stun.rs`（`GatheredCandidates` 109-114 + `gather_candidates` 118-139：新增 `pub srflx: Option<SocketAddr>`）
- Modify: `crates/cli/src/flow.rs`（`setup_punch_endpoint` 261-289、`punch_err` 292-298、新增 `apply_test_env`；既有 tests 追加）
- Modify: `crates/cli/src/main.rs`（send/recv 分支 67-90）
- Test: `crates/core/tests/stun_client.rs`（srflx 字段断言）、`flow.rs` 内 `#[cfg(test)]`

**Interfaces:**
- Produces:
  - `GatheredCandidates { addrs, nat_symmetric, stun_ok, srflx: Option<SocketAddr> }`（主端口映射成功且非本地地址 → `Some(映射)`）
  - `fn setup_punch_endpoint(me, peer_id, peer_pk, stun, tuning: TransportTuning) -> Result<(Endpoint, Vec<SocketAddr>, bool, bool)>`——内部改用 `ep::make_dual_endpoint_tuned(sock, me, &allowed, tuning)`
  - `pub fn apply_test_env(send: &mut SendOpts, recv: &mut RecvOpts, get: impl Fn(&str) -> Option<String>)`（解析下表；非法值 warn 后忽略保持默认）
  - 候选日志行：gather 后 `tracing::info!("candidates: local=[{}] srflx=[{}] symmetric={}", local_joined, srflx, nat_symmetric)`（local = `g.addrs` 去掉 srflx）
  - `punch_err` 文案恒含 NAT 类型：对称时既有提示，非对称追加 `"local NAT non-symmetric"`

| env | 字段 | 语义 |
|---|---|---|
| `MTDSS_TEST_IDLE_TIMEOUT_MS` | 两侧 `punch.tuning.max_idle_timeout` | u64 毫秒 |
| `MTDSS_TEST_KEEPALIVE_MS` | 两侧 `punch.tuning.keep_alive_interval` | `0` → `None` |
| `MTDSS_TEST_MAX_RECONNECTS` | `send.retry.max_reconnects` / `recv.retry.max_reconnects` | u32 |
| `MTDSS_TEST_BACKOFF_MS` | `send.retry.backoff_base` | u64 毫秒 |
| `MTDSS_TEST_RECV_WINDOW_MS` | `recv.retry.recv_window` | u64 毫秒 |

- Consumes: Task 4 的 `debug_assert`（本任务注入的亚秒值不得触发它——毫秒级注入安全）。

- [ ] **Step 1: 写失败测试**

```rust
// flow.rs tests:
#[test] fn apply_test_env_sets_all_five_knobs() { /* HashMap getter 注入 5 值 → 两 opts 字段逐一断言;含 KEEPALIVE_MS=0 → None */ }
#[test] fn apply_test_env_ignores_garbage() { /* "abc"/空 → 保持默认,不 panic */ }
#[test] fn punch_err_always_names_nat_type() { /* symmetric=false 的文案含 "non-symmetric",true 含 "symmetric" */ }
// stun_client.rs:既有假应答器用例追加断言 gather().srflx == Some(该应答器回的映射地址)
```

- [ ] **Step 2: 跑测试确认失败**

Run: `cargo test -p mtdss-cli apply_test_env punch_err && cargo test -p mtdss-core --test stun_client`
Expected: 编译失败（`apply_test_env`/`srflx` 字段不存在）。

- [ ] **Step 3: 实现**

按 Interfaces 逐项落。`main.rs` send/recv 分支改为构造 `let (mut so, mut ro) = (SendOpts::default(), RecvOpts::default()); apply_test_env(&mut so, &mut ro, |k| std::env::var(k).ok());` 后调 `run_send_with`/`run_recv_with`。`setup_punch_endpoint` 两个调用点（`send_round` 579、`recv_handshake` 920）传 `ctx.punch.tuning`；`single_stun_addr_uses_alt_primary` 测试传 `TransportTuning::default()`。

- [ ] **Step 4: 跑测试确认通过**

Run: `cargo test -p mtdss-cli && cargo test -p mtdss-core --test stun_client && cargo test -p mtdss-cli --test cli_e2e cli_send_recv_with_stun_punch`
Expected: 全 PASS（含既有 e2e——默认 tuning 不变）。

- [ ] **Step 5: 门禁 + 提交**

```bash
cargo test --workspace && cargo clippy --workspace --all-targets -- -D warnings
git add crates/core/src/net/stun.rs crates/cli/src; git commit -m "feat(cli): MTDSS_TEST_* env tuning surface, tuned accept-side endpoint, srflx-aware candidate log, punch_err NAT label"
```

---

### Task 6: ④ cli 级静默断链 e2e（真实子进程硬杀）

**Files:**
- Test: `crates/cli/tests/cli_e2e.rs`（追加两个用例；复用 `cli_send_directory_and_files_multi_path` 的真实子进程模式 826-843 与 `register_rest`）

**Interfaces:**
- Consumes: Task 5 的 env 面（`MTDSS_TEST_IDLE_TIMEOUT_MS/KEEPALIVE_MS/RECV_WINDOW_MS/MAX_RECONNECTS/BACKOFF_MS`）。

- [ ] **Step 1: 写两个 e2e（依赖 Task 5 已合入,先跑必红——env 不生效或检测失真时红）**

```rust
/// 杀发送方:接收端必须在注入 idle 内显形、窗口到点退出(而非 300s 默认挂)。
#[tokio::test]
async fn silent_sender_death_detected_receiver_window_expires() {
    // 90s 总兜底;两侧真实子进程:
    // recv child: IDLE=800ms, RECV_WINDOW=3000ms
    // send child: 3 文件(4KB/256KB/1MB), BACKOFF=200ms, MAX_RECONNECTS=2, IDLE=800ms
    // 轮询 out 出现首个文件 → t0 = now → sender.start_kill() + wait
    // 断言: recv child 在 ≤ 8s(窗口 3s + 轮距/宽限)内退出且非零,stderr 含 "接收窗口超时";
    //       t0.elapsed() ≥ idle 检测生效证明:窗口从断连起算(若 idle 未生效会拖到 30s+ 超断言)
}

/// 杀接收方:发送端检测→退避重连→对重启后的接收端经 .mtdss 对账续传完成。
#[tokio::test]
async fn silent_receiver_death_sender_reconnects_and_resumes() {
    // send child: BACKOFF=300ms, MAX_RECONNECTS=8, IDLE=800ms, 3 文件
    // recv child#1: IDLE=800ms, RECV_WINDOW=30000ms
    // 首文件落地后 start_kill() recv#1 → sleep 2s → 同 home/out 重启 recv#2(同 env)
    // 断言: send child 最终 exit 0;3 文件内容逐一吻合(跨进程续传);recv#2 正常退出
}
```

- [ ] **Step 2: 跑测试确认失败（或证明已修）**

Run: `cargo test -p mtdss-cli --test cli_e2e silent_`
Expected: Task 5 已线穿 tuning 时应直接绿；若红，红因即待修点（多为注入未达 accept 侧）。两用例在 ubuntu 与 windows 双 lane 都必须绿（CI 即证）。

- [ ] **Step 3:（仅当 Step 2 红）按红因修复后复绿**

- [ ] **Step 4: 门禁 + 提交**

```bash
cargo test --workspace && cargo clippy --workspace --all-targets -- -D warnings
git add crates/cli/tests/cli_e2e.rs; git commit -m "test(cli): silent link death e2e via real child kills (both directions, resume across process restart)"
```

---

### Task 7: ⑦ multi_path e2e 排查与加固

**Files:**
- Modify: `crates/cli/tests/cli_e2e.rs`（`cli_send_directory_and_files_multi_path` 826-943）

**Interfaces:** 无。

- [ ] **Step 1: 排查并加固 harness**

读该用例与被测路径，已知最大嫌疑：recv 子进程的 stdout/stderr 管道在 send 完成前**无人抽干**——传输期 tracing 日志（info 级）可能写满管道缓冲，子进程阻塞在日志写上、收尾轮不退出。加固：spawn 后立即起两个 `tokio::spawn` 持续 `read_to_end` 到共享缓冲（发送完成后的转储/断言逻辑改读该缓冲），保留 30s 超时 + 转储。同时排查其余挂点（stdin 未重定向、窗口等待、kill_on_drop 链），有则一并修。

- [ ] **Step 2: 本地重复跑验证**

Run: `for i in $(seq 1 10); do cargo test -p mtdss-cli --test cli_e2e cli_send_directory_and_files_multi_path || break; done`
Expected: 10/10 绿（Windows 本机；挂起类 flake 在本机不必然复现——以 CI 后续运行为准）。

- [ ] **Step 3: 门禁 + 提交（结案语进 commit body）**

```bash
cargo test --workspace && cargo clippy --workspace --all-targets -- -D warnings
git commit -m "test(cli): drain recv child pipes concurrently during multi-path e2e (flake hardening)

ff08a81 诊断在位;本轮加固管道抽干嫌疑点,机制未定则以 CI 再现时的转储定位。"
```

---

### Task 8: ⓪ + netns harness 基建 + full×full 冒烟 + CI job

**Files:**
- Modify: `.github/workflows/ci.yml`（第 31 行 clippy 补 `--all-targets`；新增 `nat-matrix` job）
- Create: `scripts/nat-matrix/lib.sh`、`setup.sh`、`teardown.sh`、`run_case.sh`、`matrix.sh`、`README.md`
- Create: `scripts/nat-matrix/payload.sh`（生成 4 文件 ×3MiB 合计 ~12MiB 的 dd/urandom 载荷并输出 sha256 清单）

**Interfaces:**
- Produces: `run_case.sh <typeA> <typeB> <expect:ok|fail>`（exit 0/非 0）；`matrix.sh`（逐例跑,当前只含 `full full ok` 一行）；`setup.sh`/`teardown.sh` 幂等
- Consumes: Task 5 的候选日志行（`srflx=[203.0.113.x:…]` grep）与二进制 `mtdss`/`signaling-server`/`stun-server`

- [ ] **Step 1: 写 harness（拓扑与 NAT 规则集为定稿,勿改语义）**

五个 netns 与编址（TEST-NET-3）：

```
ns pub:  203.0.113.1/30 (链 A 侧)  + 203.0.113.5/30 (链 B 侧)
ns gwA:  10.0.1.1/30 (lan) + 203.0.113.2/30 (ext)     ns a: 10.0.1.2/30, 默认路由 → gwA
ns gwB:  10.0.2.1/30 (lan) + 203.0.113.6/30 (ext)     ns b: 10.0.2.2/30, 默认路由 → gwB
gw 两侧 net.ipv4.ip_forward=1;veth 命名 <ns>-ext/<ns>-lan 可预测
pub 内起 signaling-server --bind 0.0.0.0:8787 与 stun-server --bind 0.0.0.0:3478 --bind-alt 0.0.0.0:3479
  signaling 通告 --stun-addr 203.0.113.1 --stun-port 3478,3479
设备注册:curl POST /auth/request-code(dev 模式回 code)→ /auth/verify;两侧 home 落在 ns 内临时目录
```

NAT 类型规则（gwA/gwB 同一配方,`<ext-if>`=朝 pub 的 veth）：

```bash
# 公共:固定外地址、保端口 SNAT + 转发放行出向
iptables -t nat -A POSTROUTING -o $ext_if -j SNAT --to-source $ext_ip
iptables -A FORWARD -i $lan_if -o $ext_if -j ACCEPT

full_cone:      # 静态 DNAT(全端口) + 入向全放
  iptables -t nat -A PREROUTING -i $ext_if -j DNAT --to-destination $peer_lan_ip
  iptables -A FORWARD -i $ext_if -o $lan_if -j ACCEPT

restricted_cone: # 静态 DNAT + 仅放行"曾被联系过的源 IP"(不限端口)
  iptables -t nat -A PREROUTING -i $ext_if -j DNAT --to-destination $peer_lan_ip
  iptables -t mangle -A FORWARD -i $lan_if -o $ext_if -m recent --set --rdest --name contacted
  iptables -A FORWARD -i $ext_if -o $lan_if -m recent --rcheck --name contacted -j ACCEPT
  iptables -A FORWARD -i $ext_if -o $lan_if -j DROP

port_restricted: # 静态 DNAT + conntrack 精确 (ip,port) 放行
  iptables -t nat -A PREROUTING -i $ext_if -j DNAT --to-destination $peer_lan_ip
  iptables -A FORWARD -i $ext_if -o $lan_if -m conntrack --ctstate ESTABLISHED,RELATED -j ACCEPT
  iptables -A FORWARD -i $ext_if -o $lan_if -j DROP

symmetric:       # 仅动态 SNAT(按目的各自映射),无静态 DNAT——入向只可能来自精确映射过的 (ip,port)
  # (公共两条之外不加规则)
```

`run_case.sh` 流程：`teardown.sh`（幂等：`ip netns del` 全部五个 + `iptables -t nat -F`/`-t mangle -F` 逐表刷）→ `setup.sh` → 注册 → b 侧 `ip netns exec b env MTDSS_HOME=… RUST_LOG=info $BIN recv --out …`（后台,日志落文件）→ 轮询 devices 在线 → a 侧同法 `send` → `expect=ok`:exit 0 且双侧 sha256 清单一致且两侧日志各 grep 到 `srflx=[203.0.113.2` / `srflx=[203.0.113.6`；`expect=fail`:exit 非 0 且 stderr 含 `symmetric` 且耗时 ≤ 30s；任何失败：转储两侧完整日志再 teardown。`matrix.sh` 末尾 `teardown.sh`。

- [ ] **Step 2: 本地语法验证**

Run: `bash -n scripts/nat-matrix/*.sh`
Expected: 无输出（语法过）。真实运行只能在 Linux——CI 验证。

- [ ] **Step 3: ci.yml 两个改动**

第 31 行 → `cargo clippy --workspace --all-targets -- -D warnings`（⓪）。新增 job：

```yaml
  nat-matrix:
    runs-on: ubuntu-latest
    steps:
      - uses: actions/checkout@v4
      - uses: dtolnay/rust-toolchain@stable
      - uses: arduino/setup-protoc@v3
        with: { repo-token: "${{ secrets.GITHUB_TOKEN }}" }
      - uses: Swatinem/rust-cache@v2
      - run: cargo build --release -p mtdss-cli -p signaling -p stun-server
      - run: sudo -E env "PATH=$PATH" bash scripts/nat-matrix/matrix.sh
```

- [ ] **Step 4: 推分支看 CI**

Run: `git push -u origin <branch>`（或既有 PR 自动跑）
Expected: 双平台 test job 绿（clippy 含 --all-targets 首次进 CI）+ nat-matrix job 绿（full×full 冒烟）。红则按日志修脚本（规则语义不得改）再推。

- [ ] **Step 5: 提交**

```bash
git add .github/workflows/ci.yml scripts/nat-matrix; git commit -m "ci: nat-matrix harness (netns topology, NAT type rules, full-cone smoke) + clippy --all-targets in CI"
```

---

### Task 9: 矩阵全量 14 例

**Files:**
- Modify: `scripts/nat-matrix/matrix.sh`（case 表）,`run_case.sh`（双键序支持）

**Interfaces:**
- Consumes: Task 8 全部。

- [ ] **Step 1: 展开 case 表**

```
# 6 纯锥 ×1(默认调参):
full full ok; full restricted ok; full port_restricted ok
restricted restricted ok; restricted port_restricted ok; port_restricted port_restricted ok
# 4 含对称 ×2(全新身份再跑一次;键序从 /devices 的 pubkey hex 字典序判定并打进日志,
# 预期与键序无关——双跑只为两侧角色都回归):
full symmetric ok ×2; restricted symmetric ok ×2
port_restricted symmetric fail ×2; symmetric symmetric fail ×2
```

**失败例注入小重试预算**（否则发送端 5 轮默认退避让每例拖 ~4 分钟）：`fail` 例两个子进程加 `MTDSS_TEST_MAX_RECONNECTS=1 MTDSS_TEST_BACKOFF_MS=500`——单例 ≈ 2×(punch 10s + 退避) ≈ 25s；**打洞预算保持默认 10s**（NAT 行为时序是测点，退避节奏不是）。

- [ ] **Step 2: 推分支看 CI,迭代到 14/14 绿**

Run: `git push`
Expected: nat-matrix job 全绿。失败例规则嫌疑排查顺序：full_cone 的 DNAT 放行了杂流量（应只影响该侧）、restricted 的 recent 表没记上（mangle 链顺序）、symmetric 误配静态 DNAT。**预期结果表不得改**。

- [ ] **Step 3: 门禁 + 提交**

```bash
cargo test --workspace && cargo clippy --workspace --all-targets -- -D warnings
git add scripts/nat-matrix; git commit -m "test(ci): full 14-case NAT matrix with order-independent expectations"
```

---

### Task 10: netem 混沌 4 用例

**Files:**
- Create: `scripts/nat-matrix/chaos.sh`；Modify: `matrix.sh`（末尾追加 `chaos.sh`）,`run_case.sh`（混沌模式参数）

**Interfaces:**
- Consumes: Task 8 harness、Task 5 env 面。

- [ ] **Step 1: 写 4 用例（拓扑固定 port_restricted×port_restricted,注入定稿）**

```bash
loss10:    tc qdisc add dev gwA-ext root netem loss 10% ; 同 gwB      # 硬超时 120s
delay200:  tc qdisc add dev gwA-ext root netem delay 200ms 50ms ; 同 gwB  # 180s
blackout3: 子进程 IDLE=1000ms,KEEPALIVE=500ms;send 启动 2s 后
           ip link set gwA-ext down;sleep 3;up                          # 120s;断言终态 sha256 一致
permfail:  同 blackout3 但不 up;子进程再注入 MAX_RECONNECTS=2 BACKOFF=300ms
           RECV_WINDOW=8000ms                                          # 120s;断言 send 非 0 + 报告清单、recv "接收窗口超时"
```

- [ ] **Step 2: 推分支看 CI**

Expected: 4 用例全绿（时间断言 = 各自硬超时;初值即上表,勿收紧）。

- [ ] **Step 3: 门禁 + 提交**

```bash
cargo test --workspace && cargo clippy --workspace --all-targets -- -D warnings
git add scripts/nat-matrix; git commit -m "test(ci): netem chaos suite (loss/delay/blackout/permanent) with hard-timeout bounds"
```

---

### Task 11: 发布检查单 + README + 校准 + 收官验收

**Files:**
- Create: `docs/release-checklist.md`
- Modify: `scripts/nat-matrix/README.md`（本地复跑说明 + `MTDSS_TEST_*` 清单与"测试专用"声明）
- Modify: `scripts/nat-matrix/chaos.sh`（校准后硬超时值,只放宽）

**Interfaces:** 无。

- [ ] **Step 1: 发布检查单（内容定稿）**

```markdown
# MTDSS 发布检查单
## 跨真实网络对传（V1 发布前执行一次,记录归档本目录）
1. 两处不同 NAT 网络(如 家宽 × 手机热点)各一台设备,各自 `mtdss register`
2. RUST_LOG=info 下多文件对传,记录两侧日志的 `candidates:` 行(srflx 值)与 `symmetric=` 预判
3. 记录:日期 / 两侧网络环境 / srflx 是否出现 / 传输结果(成功字节数或失败文案)
- [ ] 家宽 × 热点 对传记录:____
```

- [ ] **Step 2: 校准硬超时**

取最近 ≥5 次 CI nat-matrix job 各例耗时中位,任一硬超时 < 3× 中位的放宽到 3×（取整到 10s）。改动只放宽。

- [ ] **Step 3: 验收清单逐项核对（spec §9）并收官**

逐项：RIDE ⓪-⑨、矩阵 14 例预期全对、混沌 4 用例、srflx 自动断言 + 检查单、掉电有序落盘（Task 1 测试在）、⑦ 结案记录（Task 7 commit body）。门禁全绿（双平台 + nat-matrix）。

- [ ] **Step 4: 提交**

```bash
git add docs/release-checklist.md scripts/nat-matrix; git commit -m "docs: release checklist (real-network srflx validation) + calibrated chaos timeouts"
```
