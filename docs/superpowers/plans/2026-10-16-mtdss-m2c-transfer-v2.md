# MTDSS M2c(传输 v2)Implementation Plan

> **For agentic workers:** REQUIRED SUB-SKILL: Use superpowers:subagent-driven-development (recommended) or superpowers:executing-plans to implement this plan task-by-task. Steps use checkbox (`- [ ]`) syntax for tracking.

**Goal:** 交付传输 v2——多文件/目录、隐藏收件区对账、接收计划协议、并发调度、会话内断线重连、按原始路径的失败报告,并清偿 M2b 六笔遗留债务。

**Architecture:** 对账驱动一切:接收端 `.mtdss/` 隐藏区里的分片临时文件+记录是唯一事实源,manifest 之后接收端逐文件回"跳过/全量/续传+bitmap"计划,发送端只推计划内块——初次发送、断线重连、手动重发三种来件走同一机制。core 每连接自洽(`send_files`/`recv_files`),重试循环在 cli/flow 编排层,manifest 由编排层一次构建、跨 attempt 复用。死链检测靠注入双角色端点的 QUIC keep-alive + 空闲超时。

**Tech Stack:** Rust 2021;quinn 0.11(TransportConfig:keep_alive_interval/max_idle_timeout);tokio;prost;serde_json(记录文件)。

**Spec:** `docs/superpowers/specs/2026-10-16-mtdss-m2c-transfer-v2-design.md`

## Global Constraints

- `manifest.proto` 与 `signal.proto` **零改动**(多文件用既有 repeated files)
- 常量逐字取自 spec:checkpoint 重请求上限 **3 轮**;调度 **4 文件并发 × 每文件 4 在途流**;重连 **MAX_RECONNECTS=5、退避 1/2/4/8/16s**;接收窗口 **RECV_RECONNECT_WINDOW=300s**;**keep_alive_interval=10s、max_idle_timeout=30s**;记录批量落盘 **每 16 块或 2s**;manifest 编码上限 **64 MiB**、文件数 **≤ 65535**;隐藏区目录名 **`.mtdss`**。全部可注入(测试秒级/亚秒级)
- 测试门禁:Task 1 沿用 `cargo test --workspace && cargo clippy --workspace -- -D warnings`;**Task 2 起改为** `cargo test --workspace && cargo clippy --workspace --all-targets -- -D warnings`(门禁升级本身是 Task 2 的债务项)
- 本机(Windows):bash 里 `export PATH="$HOME/.cargo/bin:$PATH"; export PROTOC="/c/Users/Administrator/AppData/Local/Microsoft/WinGet/Packages/Google.Protobuf_Microsoft.Winget.Source_8wekyb3d8bbwe/bin/protoc.exe"`
- 提交信息 conventional 前缀;代码注释跟随仓库中文风格
- 流帧格式不做跨版本兼容(spec §4.2):旧 `send_file` 与旧块帧直接删除,调用点随任务迁移,不留兼容层

## Review Focus

1. **静默断链挂死**:对端进程消失/NAT 重绑时无任何 close 帧——若无 keep-alive/空闲超时,读写永久挂起、重连永不触发。→ Task 5:`idle_timeout_kills_silent_connection` + `keep_alive_keeps_quiet_connection_alive` 两测钉死机制。
2. **冲突改名后 SKIP 误判**:文件曾落盘为 `a (1).txt`,按原始 rel 查存在性必失败 → 每次手动重发重复传整个文件。→ Task 3:`skip_uses_final_rel_after_conflict_rename`。
3. **记录损坏/半写导致误续或崩**:截断 JSON、半写记录在崩溃后必然存在。→ Task 3:`corrupt_record_degrades_to_full` + 对账四态测试(不崩、不误续、按 FULL 重传)。
4. **终局错误空耗重试**:磁盘不足/源文件被改重试不可能成功,还把"对端拒绝"误报成"网络中断"。→ Task 4:手打 reject 帧 → `SendError::Rejected`;Task 6:断言只有 Network 进重试循环。
5. **恶意对端无限重请求/伪造块**:checkpoint 循环无上限、(file_idx,offset) 不查签名块表 = DoS 与写放大。→ Task 4:`checkpoint_rounds_exhausted_is_protocol_error` + `forged_chunks_rejected`。

---

### Task 1: manifest 多文件(`protocol::manifest`)

**Files:**
- Modify: `crates/core/src/protocol/manifest.rs`(`build_single` → `build_manifest`;`verify` 放宽+新增校验;`validate_rel_path` 拒绝首段 `.mtdss`)
- Modify: `crates/core/src/transport/transfer.rs:63`(send_file 内改用 build_manifest,一行)
- Test: `crates/core/tests/manifest.rs`(迁移 + 追加)

**Interfaces:**
- Produces: `pub fn build_manifest(items: &[(std::path::PathBuf, String)], id: &DeviceIdentity) -> Result<Manifest>`——items[i].1 即 files[i].rel_path,**保序**;`build_single` 删除。
- Consumes: 既有 `signable_bytes`/`validate_rel_path`/`MANIFEST_VERSION`(零改动)。
- Task 3/4/6 依赖 `build_manifest` 签名与保序契约。

- [ ] **Step 1: 迁移既有测试并写失败测试**

`crates/core/tests/manifest.rs`:全部 `build_single(&p, rel, &id)` 改 `build_manifest(&[(p.clone(), rel.into())], &id)`(7 处:34/51/68/82/105/116/218 行附近)。追加:

```rust
#[test]
fn multi_file_manifest_builds_and_verifies_in_order() {
    let dir = write_tmp_dir(); // 三个文件:a.txt(10B)、d/b.bin(2 MiB 跨块)、e/空(0B)
    let id = DeviceIdentity::generate();
    let items = vec![(dir.join("a.txt"), "a.txt".into()),
                     (dir.join("d/b.bin"), "d/b.bin".into()),
                     (dir.join("e"), "e".into())];
    let man = build_manifest(&items, &id).unwrap();
    assert_eq!(man.files.iter().map(|f| f.rel_path.as_str()).collect::<Vec<_>>(),
               ["a.txt", "d/b.bin", "e"]);           // 保序
    let files = verify(&man, &id.public_key()).unwrap();
    assert_eq!(files.len(), 3);                       // 多文件通过(旧断言 ==1 消亡)
}

#[test]
fn duplicate_rel_paths_rejected() {
    // 手工构造:两个条目同 rel,签名有效 → verify 必须拒绝
    ...构造 man(两条目 rel 均 "a"),signable_bytes 签名...
    assert!(verify(&man, &pk).unwrap_err().to_string().contains("duplicate"));
}

#[test]
fn mtdss_first_segment_rejected() {
    // 发送侧:build_manifest 拒绝;接收侧:verify 拒绝(手签 manifest)
    let items = vec![(p, ".mtdss/x".into())];
    assert!(build_manifest(&items, &id).is_err());
    ...手造签名 manifest(rel=".mtdss/x") → verify Err("intake")...
}

#[test]
fn file_count_cap_enforced() {
    // 65536 个最小条目的手造签名 manifest → verify 拒绝
}
```

- [ ] **Step 2: 跑测试确认失败**

Run: `cargo test -p mtdss-core --test manifest`
Expected: 编译失败(`build_manifest` 不存在)。

- [ ] **Step 3: 实现**

`build_manifest` = 现 `build_single` 循环化(逐 item 读文件分块;transfer_id 仍一次性随机 hex32;签名覆盖全部条目——`signable_bytes` 已多文件)。`validate_rel_path` 开头加:`rel.split('/').next() == Some(".mtdss") → false`。`verify`:删 `==1` 断言;加 `files.len() <= 65535`;加 rel 去重(`HashSet` 检测重复 → "duplicate rel path")。`transfer.rs:63` 改 `manifest::build_manifest(&[(path.to_path_buf(), rel.to_string())], id)?`。

- [ ] **Step 4: 跑测试确认通过**

Run: `cargo test -p mtdss-core --test manifest && cargo test -p mtdss-core --test transfer && cargo test -p mtdss-core --test e2e`
Expected: 全 PASS(e2e 仍单文件路径,经 build_manifest 等价)。

- [ ] **Step 5: 旧门禁 + 提交**

```bash
cargo test --workspace && cargo clippy --workspace -- -D warnings
git add crates/core/ crates/cli/ 2>/dev/null; git commit -m "feat(core): multi-file manifest (build_manifest, verify N>=1, .mtdss reserved segment)"
```

---

### Task 2: M2b 小债清偿(alt=primary、--stun-port、RFC1918、run_listener、--all-targets)

**Files:**
- Modify: `crates/cli/src/flow.rs`(`setup_punch_endpoint` 单通告分支)
- Modify: `server/signaling/src/main.rs` + `server/signaling/src/lib.rs`(解析函数入库)
- Modify: `server/stun/src/lib.rs`(`run_listener` 错误续行)
- Modify: `crates/core/tests/stun_client.rs`(RFC1918 断言恢复)
- Test: `crates/cli/src/flow.rs` 内 `#[cfg(test)]`、`server/signaling/tests/ws.rs`(追加)

**Interfaces:**
- Produces: `pub fn parse_stun_ports(ports_raw: &str) -> anyhow::Result<Vec<u16>>`(signaling lib;拒绝 0 端口与重复端口,1-2 个)。
- Consumes: 既有 `setup_punch_endpoint`/`run_listener`。

- [ ] **Step 1: 写失败测试**

`server/signaling/tests/ws.rs` 追加:

```rust
#[test]
fn stun_port_validation() {
    assert!(signaling::parse_stun_ports("3478,3479").is_ok());
    assert!(signaling::parse_stun_ports("3478").is_ok());
    assert!(signaling::parse_stun_ports("0").is_err());            // 0 端口
    assert!(signaling::parse_stun_ports("3478,3478").is_err());    // 重复
    assert!(signaling::parse_stun_ports("").is_err());
    assert!(signaling::parse_stun_ports("1,2,3").is_err());        // 超过 2 个
}
```

`crates/cli/src/flow.rs` tests 追加(起真实回环 STUN 单端口,验证不再降级):

```rust
#[tokio::test]
async fn single_stun_addr_uses_alt_primary() {
    // 回环真 STUN 单端口;单条通告下 gather 必须成功(stun 路径不降级为 local-only)
    let s = std::net::UdpSocket::bind("127.0.0.1:0").unwrap();
    let pa = s.local_addr().unwrap();
    tokio::spawn(async move { let _ = stun::run_listener(s, 100, 100).await; });
    let me = mtdss_core::identity::DeviceIdentity::generate();
    let (ep, addrs, _) = setup_punch_endpoint(&me, "peer", &me.public_key(), &[pa.to_string()]).unwrap();
    assert!(!addrs.is_empty());
    drop(ep);
}
```

`crates/core/tests/stun_client.rs` 的 `local_addrs_ordering_and_cap` 恢复排序断言:

```rust
// RFC1918 私网必须排在非私网非环回之前;环境无私网地址时跳过该断言
let privs: Vec<i64> = c.iter().map(|a| match a.ip() {
    std::net::IpAddr::V4(v4) if v4.is_private() => 1, _ => 0 }).collect();
if privs.iter().any(|&p| p == 1) {
    let first_pub = privs.iter().position(|&p| p == 0);
    let last_priv = privs.iter().rposition(|&p| p == 1);
    assert!(first_pub.map_or(true, |f| f > last_priv.unwrap()),
        "private addrs must precede public: {c:?}");
}
```

- [ ] **Step 2: 跑测试确认失败**

Run: `cargo test -p signaling --test ws stun_port && cargo test -p mtdss-cli --lib single_stun`
Expected: `parse_stun_ports` 编译失败;flow 测试失败(单通告现走 local-only 分支,gather 不发生——断言 addrs 含 srflx 或以行为差异体现,以实现后同一测试 PASS 为准)。

- [ ] **Step 3: 实现**

1. `signaling/src/lib.rs` 加 `parse_stun_ports`;`main.rs` 内联解析段改调用它。
2. `flow.rs` `setup_punch_endpoint` 的 match 改三分支:`(Some(p), Some(a)) => (*p, *a)`、`(Some(p), None) => (*p, *p)`、`_ => 本地降级`(现状)。
3. `stun/src/lib.rs` `run_listener`:`recv_from`/`send_to` 的 `?`/忽略改为 `Err(e) => { tracing::warn!("stun io: {e}"); tokio::time::sleep(Duration::from_millis(50)).await; continue }`(接收错误);send_to 错误 warn 后继续。

- [ ] **Step 4: 跑测试确认通过 + 门禁升级**

Run: `cargo test --workspace && cargo clippy --workspace --all-targets -- -D warnings`
Expected: 全 PASS;--all-targets 暴露的存量警告在本步清零(此后为常态门禁)。

- [ ] **Step 5: 提交**

```bash
git add server/ crates/ && git commit -m "fix(debt): m2b leftovers (alt=primary, stun-port validation, rfc1918 assert, listener resilience, all-targets clippy)"
```

---

### Task 3: 隐藏收件区与对账(`core::transport::intake`)

**Files:**
- Create: `crates/core/src/transport/intake.rs`
- Modify: `crates/core/src/transport/mod.rs`(加 `pub mod intake;` + re-export)、`crates/core/Cargo.toml`(deps 加 serde/serde_json,版本随 workspace)
- Modify: `crates/core/src/transport/transfer.rs`(`unique_target` 移入 intake.rs,原处删除)
- Test: `crates/core/tests/intake.rs`(新建)

**Interfaces:**
- Produces:
  - `pub const INTAKE_DIR: &str = ".mtdss";`
  - `pub enum FilePlan { Full, Skip, Resume(Vec<u8>) }`——Resume 位 = 1 表示**缺块待收**(与记录"已收"位互补)
  - `pub fn record_key(rel: &str) -> String`(sha256 hex 全长)、`pub fn part_path(out_dir: &Path, rel: &str) -> PathBuf`、`pub fn record_path(out_dir: &Path, rel: &str) -> PathBuf`
  - `pub fn reconcile(out_dir: &Path, man: &Manifest) -> Vec<FilePlan>`(与 man.files 等长,保序)
  - `pub struct PartWriter`:《`pub async fn open(out_dir: &Path, entry: &FileEntry, resume: bool) -> Result<Self>`;`pub async fn write_chunk(&mut self, idx: usize, offset: u64, data: &[u8]) -> Result<()>`;`pub async fn flush_record(&mut self) -> Result<()>`;`pub fn missing_count(&self) -> usize`;`pub async fn finalize(self) -> Result<PathBuf>`》——finalize 含整文件哈希读回 + unique_target 占位 + 原子改名 + 记 done/final_rel + 删 .part
  - `pub(crate) fn unique_target(out_dir: &Path, rel: &str) -> Result<PathBuf>`(自 transfer.rs 移入,逻辑不变)
- Task 4 的 recv_files 消费以上全部。

- [ ] **Step 1: 写失败测试**

`crates/core/tests/intake.rs`(手造 `FileEntry`+签名 manifest 用 `mtdss_core::protocol::manifest::build_manifest` 于临时文件):

```rust
//! 隐藏收件区:对账四态 + 记录持久化 + 完成落盘。

核心用例(逐个 #[test]/#[tokio::test]):
- reconcile_four_states:无记录→Full;部分 bitmap+匹配 .part→Resume(断言缺块位图=反相);
  done+final_rel 存在且 size 吻合→Skip;哈希不符→Full 且旧记录/.part 被删。
- skip_uses_final_rel_after_conflict_rename:记录 done、final_rel=Some("a (1).txt"),
  out_dir 有 "a (1).txt" 而无 "a.txt" → Skip(而非 Full)。
- done_but_final_missing_is_full:done=true、final 不存在 → Full。
- corrupt_record_degrades_to_full:record_path 写入截断 JSON 垃圾 → reconcile=Full,不 panic。
- zero_chunk_file_finalizes:0 字节 entry → open→finalize 直接落盘。
- batched_flush_visible:open(FULL) 写 5 块 → flush 前读记录文件可无新位(允许),
  再写至第 16 块 → 记录文件自动含全部 16 位(无需显式 flush/未 finalize)。
- finalize_renames_and_cleans:.part 完成后改名到终名、.part 删除、记录 done+final_rel 落盘;
  终名冲突时得 "name (1).ext" 且 final_rel 记改名后名字。
- windows_intake_dir_hidden(#[cfg(windows)]):建区后 metadata 文件属性含 HIDDEN(0x2)。
```

- [ ] **Step 2: 跑测试确认失败**

Run: `cargo test -p mtdss-core --test intake`
Expected: 编译失败(模块不存在)。

- [ ] **Step 3: 实现**

`intake.rs` 要点:
- 记录结构 `#[derive(Serialize, Deserialize)] struct FileRecord { rel, size, sha256(hex), chunk_count, bitmap(hex), done, final_rel: Option<String> }`;落盘 = 写 `<name>.json.tmp` + `rename`(原子);读失败/字段不符 → None。
- `reconcile` 按 §3 对账表实现;Skip 校验 `final_rel` 的 size 用 `std::fs::metadata`。
- `ensure_intake_dir(out_dir)`:`create_dir_all` + `#[cfg(windows)]` `std::os::windows::fs::set_attributes(path, FILE_ATTRIBUTE_HIDDEN)`(经 `MetadataExt`/`fs::set_attributes`;不引新依赖,用 `std::os::windows::fs::set_attributes` 若不可用则 `winapi` 风格 `SetFileAttributesW` 经 `windows-sys`——以最小依赖实现,取舍留给实现者,测试钉行为)。
- `PartWriter::open`:`resume=true` 载入既有记录与 .part(重 `set_len` 对齐);`resume=false` truncate + set_len(entry.size) + 全零 bitmap + 立即落一次记录。
- `write_chunk`:seek(offset) 写 → 置位 → `dirty += 1`;`dirty >= 16 || last_flush.elapsed() >= 2s` → `flush_record`。
- `finalize`:seek(0) 流式读回算整文件哈希 → 比对 entry.sha256 → `unique_target` 占位 → `rename(.part → 终名)` → 记录 done=true + final_rel → 删 .part → 返回终名。rename 失败:记录保持 partial(不置 done)、Err 上抛。

- [ ] **Step 4: 跑测试确认通过**

Run: `cargo test -p mtdss-core --test intake && cargo test -p mtdss-core --test transfer`(unique_target 移动后既有冲突改名测试仍绿)
Expected: 全 PASS。

- [ ] **Step 5: 全量门禁 + 提交**

```bash
cargo test --workspace && cargo clippy --workspace --all-targets -- -D warnings
git add crates/core/ && git commit -m "feat(core): hidden intake area with reconciliation records and part writer"
```

---

### Task 4: 传输 v2——协议、调度、SendError(`transfer.rs` 重写)

**Files:**
- Modify: `crates/core/src/transport/transfer.rs`(重写:删 `send_file`,新 `send_files`/`recv_files` v2)
- Test: `crates/core/tests/transfer.rs`(迁移既有 6 测到 v2 API)、`crates/core/tests/transfer_v2.rs`(新建)、`crates/core/tests/e2e.rs:204`(m1_full_vertical 随迁)、`crates/core/tests/punch.rs:95`(send_file → send_files)

**Interfaces:**
- Consumes: Task 1 `build_manifest`;Task 3 intake 全部。
- Produces(spec §4.5 逐字):
  - `pub struct SendOptions { pub max_concurrent_files: usize, pub max_inflight_per_file: usize, #[doc(hidden)] pub fault_drop_chunks: std::collections::HashSet<(u32, u64)>, #[doc(hidden)] pub fault_drop_until_round: u32 }`(`Default` = 4/4/空/0;fault 语义:第 r 轮(0 起)且 r ≤ fault_drop_until_round 时跳发表中 (file_idx, offset) 块)
  - `pub enum SendProgress<'a> { FileDone { rel: &'a str }, Bytes { rel: &'a str, done: u64, total: u64 } }`
  - `pub enum SendError { Network(String), Rejected { reason: String }, Protocol { reason: String } }`(impl Display + std::error::Error)
  - `pub async fn send_files(conn: &Connection, man: &Manifest, sources: &[(PathBuf, String)], progress: &(dyn Fn(&SendProgress) + Sync), opts: &SendOptions) -> Result<SendReport, SendError>`
  - `pub async fn recv_files(conn: &Connection, out_dir: &Path, me: &DeviceIdentity) -> Result<Vec<PathBuf>>`(签名不变,内部 v2;委托 `recv_files_fault(.., None)`)
  - `#[doc(hidden)] pub async fn recv_files_fault(conn, out_dir, me, close_after_files: Option<usize>) -> Result<Vec<PathBuf>>`——测试钩子:每文件 finalize 后计数,命中即 `conn.close(0u32, b"fault")` 并返回已完成路径(供 Task 6 重连 e2e 注入中途断连)
- Wire 帧文法(模块文档更新,receiver→sender 帧均带 tag):
  - S→R:`[u32 BE man_len][manifest]`;uni 块 `[u32 BE file_idx][u64 BE offset][u64 BE len][data]`;checkpoint `[0xFF]`
  - R→S:拒绝 `[0x00][u32 BE len][reason]`;计划 `[0x01][u32 BE file_count]{[u8 kind][u32 BE bmp_len][bmp]}`(kind 0=FULL/1=SKIP/2=RESUME,非 RESUME 无 bmp);FileDone `[0x02][u32 BE file_idx]`;终局 `[0x03]`
- Task 6 消费 `SendError`/`SendProgress`/`SendOptions`。

- [ ] **Step 1: 迁移既有测试**

`tests/transfer.rs` 6 个用例:`send_file(&sc, &f, name, &a, &|_,_|{})` → `send_files(&sc, &man, &[(f.clone(), name.into())], &|_|{}, &SendOptions::default())`,manifest 由 `build_manifest` 预建;`corrupt_chunk_never_written`/`reverse_order_chunks_assemble_correctly` 手打协议改为新文法(块帧加 `0u32` file_idx 前缀;verdict 读取改为 `[0x01]` 计划帧并按计划发块;完成标记 0xFF 后收 `[0x03]`)。`tests/punch.rs`、`tests/e2e.rs` 同式随迁。

- [ ] **Step 2: 写失败测试(transfer_v2.rs 新用例)**

```rust
//! v2:多文件/计划协议/调度/断点/有界性。

- multi_file_and_directory_transfers:3 文件(含子目录 rel 与 0 字节)→ 结构落盘、
  内容一致、无 .part 残留、.mtdss/ 仅剩记录;progress 收齐 3 个 FileDone。
- second_send_of_same_manifest_skips_everything:同一 manifest 同一连接第二次 send_files
  → SendReport.bytes_sent == 0,recv_files 再收一组路径(全部 SKIP,无数据流)。
- dropped_connection_then_new_connection_resumes_missing_only:
  A/B 直连;先手打发送前半块后 abort 控制流(drop 连接);
  新连接对上 run 一对 endpoint → send_files(同 man) → 完成;
  断言本次 bytes_sent == 仅缺块字节之和(按块流计数语义),文件内容完整。
- corrupt_chunk_re_requested_then_completes:
  SendOptions.fault_drop_chunks={(0, chunk[2].offset)}, fault_drop_until_round=0
  → 首轮缺块 → checkpoint 计划 → 次轮补发 → final;文件完整。
- checkpoint_rounds_exhausted_is_protocol_error:fault_drop_until_round=u32::MAX
  → send_files Err(SendError::Protocol{..}),文案含 "rounds"。
- rejected_maps_to_send_error_rejected:手打 recv 侧(manifest 后回 0x00+reason)
  → 真 send_files 得 Err(Rejected{reason})。
- forged_chunks_rejected:手打 sender,块帧 file_idx=99 / offset 不在表 / len>1MiB
  → recv_files Err(文案分别含 "not in manifest"/"oversized")。
- scheduling_small_files_first:SendOptions{2,1}:1 大(4 块)2 小(1 块);
  progress 的 FileDone 顺序:两小文件先于大文件(顺序断言,非计时)。
```

- [ ] **Step 3: 跑测试确认失败**

Run: `cargo test -p mtdss-core --test transfer_v2 && cargo test -p mtdss-core --test transfer`
Expected: 编译失败(`send_files` 不存在)。

- [ ] **Step 4: 实现**

发送端结构(要点):
1. 控制流 open_bi,写 manifest,spawn 控制读取任务 → `mpsc<ControlMsg>`(`Plan(Vec<FilePlan>)`/`FileDone(u32)`/`Reject(String)`/`Final`/`Closed`);流关闭且未 Final → `Network`。
2. 首帧须 Plan/Reject;round=0。计划内文件索引按 size 升序,活动波 ≤ `max_concurrent_files`:每文件一个任务,块序列(FULL=全部,RESUME=位 1 的块)过 `futures::stream::iter(...).map(send_chunk).buffer_unordered(max_inflight_per_file)`;每块:读源文件区间(每文件一个 File 句柄,`read_exact_at` 风格 seek+read)→ open_uni → `[file_idx][offset][len][data]` → finish;Bytes 进度累加。fault_drop 命中(r ≤ fault_drop_until_round)跳过。
3. 波与轮内全部推完 → 写 0xFF → 等 ControlMsg:Plan → round+=1,`round > 3` → `Protocol`,否则按新计划续推;FileDone → progress 回调 + 记录;Final → `ctl_tx.finish()` → EOF 舞步同 v1(收 0x03 后半关)。
4. 错误映射:io/open_uni/write/连接层 → `Network`;坏 tag → `Protocol`。

接收端结构(要点):
1. `accept_bi` 读 manifest(`man_bytes.len() ≤ 64 MiB` 先检)、验签(Task 1 verify)、hex32 transfer_id 校验保留。
2. `intake::reconcile` → 发计划帧;SKIP 文件立即回 FileDone;其余每文件 `PartWriter::open`(FULL/RESUME)+ `Arc<tokio::Mutex<PartWriter>>`。
3. 磁盘预检:`files.iter().map(size).sum()` 超可用 → 0x00 拒绝(同 v1)。
4. 主循环 `select! { uni = conn.accept_uni() => spawn 处理任务, ctl = ctl_rx.read() => }`:块任务读帧 → `(file_idx, offset)` 查签名块表(命中才有 desc;否则 Err)→ len ≤ CHUNK_SIZE → 哈希验 → `write_chunk` → `missing_count()==0` 时 `finalize()` + 回 FileDone(每文件恰好一次,`AtomicBool`/闭包防重)。ctl 收 0xFF:有缺 → 复核发新计划帧;全齐 → 等所有 finalize 完成 → 发 `[0x03]` → EOF 舞步同 v1。重复块(位已置)直接丢弃。
5. 返回全部落盘路径(SKIP 的不在本次路径里,仅新完成者)。`recv_files_fault`:finalize 成功后若 `close_after_files` 计数命中 → `conn.close()` 并立即返回已完成路径(Err 上抛亦可,以 Task 6 e2e 断言为准:断连后 run_recv 侧重连)。

- [ ] **Step 5: 跑测试确认通过**

Run: `cargo test -p mtdss-core`(全部测试文件)
Expected: 新旧全 PASS(含迁移后的 transfer/punch/e2e)。

- [ ] **Step 6: 全量门禁 + 提交**

```bash
cargo test --workspace && cargo clippy --workspace --all-targets -- -D warnings
git add crates/ && git commit -m "feat(core): transfer v2 (plan protocol, 4x4 scheduling, resume, bounded re-request)"
```

---

### Task 5: 死链检测——TransportTuning(`endpoint.rs`/`punch.rs`)

**Files:**
- Modify: `crates/core/src/transport/endpoint.rs`(TransportTuning + tuned 变体)
- Modify: `crates/core/src/transport/punch.rs`(PunchConfig 加 tuning)
- Test: `crates/core/tests/transport_tuning.rs`(新建);`crates/core/tests/punch.rs`(字面量补 `..Default::default()`)

**Interfaces:**
- Produces:
  - `#[derive(Debug, Clone, Copy)] pub struct TransportTuning { pub keep_alive_interval: Option<Duration>, pub max_idle_timeout: Duration }`(`Default` = `Some(10s)`/`30s`;None = 关 keep-alive)
  - `pub fn make_dual_endpoint_tuned(socket, me, allowed, tuning: TransportTuning) -> Result<quinn::Endpoint>`(server config 挂 TransportConfig);`make_dual_endpoint` 保持签名委托 Default
  - `pub async fn connect_pinned_tuned(ep, addr, peer, me, tuning) -> Result<Connection>`;`connect_pinned` 委托 Default
  - `PunchConfig { budget, handshake_timeout, tuning: TransportTuning }`(Default 携带 TransportTuning::default();dial 走 connect_pinned_tuned)
- Task 6 编排层经 PunchConfig.tuning 注入。

- [ ] **Step 1: 写失败测试**

`crates/core/tests/transport_tuning.rs`:

```rust
//! 空闲超时显形静默死亡;keep-alive 维持静默连接。

fn tuned_pair(t: TransportTuning) -> (Connection, Connection) {
    // A、B 各自 make_dual_endpoint_tuned(回环),A connect_pinned_tuned 拨 B,
    // B accept_pinned——复用 punch.rs 测试的 dual() 模式,两侧同 tuning
}

#[tokio::test]
async fn idle_timeout_kills_silent_connection() {
    let t = TransportTuning { keep_alive_interval: None, max_idle_timeout: Duration::from_millis(400) };
    let (ca, _cb) = tuned_pair(t).await;
    // 双向静默:无 keep-alive → 两端 ~400ms 空闲超时
    tokio::time::timeout(Duration::from_secs(2), ca.closed()).await
        .expect("idle timeout must terminate silent connection");
}

#[tokio::test]
async fn keep_alive_keeps_quiet_connection_alive() {
    let t = TransportTuning { keep_alive_interval: Some(Duration::from_millis(100)),
                              max_idle_timeout: Duration::from_millis(400) };
    let (ca, _cb) = tuned_pair(t).await;
    // 静默 1.5s(> 3×idle):keep-alive 必须维持连接
    let survived = tokio::time::timeout(Duration::from_millis(1500), ca.closed()).await.is_err();
    assert!(survived, "keep-alive must prevent idle death");
    // 且连接仍可用:开一条 uni 流写读正常(对端 accept_uni 收到)
}
```

- [ ] **Step 2: 跑测试确认失败**

Run: `cargo test -p mtdss-core --test transport_tuning`
Expected: 编译失败(类型不存在)。

- [ ] **Step 3: 实现**

`fn transport_config(t) -> Arc<quinn::TransportConfig>`:`max_idle_timeout(Some(VarInt::from_u32(t.max_idle_timeout.as_millis() as u32)))`、`keep_alive_interval(t.keep_alive_interval)`。server:`ServerConfig::with_crypto(..)` 后 `.transport_config(Arc::clone(&tc))`;client:`ClientConfig::new(..)` 后同样挂。`connect_pinned_tuned` 与 `connect_pinned` 共用私有 `connect_pinned_impl(ep, addr, peer, me, tuning)`(含接受令牌逻辑,零改动)。`punch.rs` dial 处传 `cfg.tuning`。既有 `PunchConfig { budget: .., handshake_timeout: .. }` 字面量调用点补 `..Default::default()`。

- [ ] **Step 4: 跑测试确认通过**

Run: `cargo test -p mtdss-core`
Expected: 全 PASS(含 punch/dual_endpoint 既有测试与两个新测试)。

- [ ] **Step 5: 全量门禁 + 提交**

```bash
cargo test --workspace && cargo clippy --workspace --all-targets -- -D warnings
git add crates/core/ && git commit -m "feat(core): transport keep-alive and idle-timeout tuning for dead-link detection"
```

---

### Task 6: 编排——重连循环、失败报告、ClientEvent 统一、接收窗口(`cli/flow.rs` + `signaling/client.rs`)

**Files:**
- Modify: `crates/core/src/signaling/client.rs`(事件序列统一)
- Modify: `crates/cli/src/flow.rs`(run_send/run_recv 重写 + collect_items;删 sweep_part_files 与 run_send 单文件旧签名)
- Modify: `crates/cli/src/main.rs`(Recv 多路径打印最小适配)、`crates/cli/tests/cli_e2e.rs`(3 处 run_send 调用点 `std::slice::from_ref(&fpath)`;run_recv 返回 Vec 适配 `landed[0]`)
- Test: `crates/core/tests/signaling_client.rs`(追加)、`crates/cli/tests/cli_e2e.rs`(追加)、`crates/cli/src/flow.rs` tests(collect_items)

**Interfaces:**
- Consumes: Task 4 `SendError/SendProgress/SendOptions/build_manifest`;Task 5 `PunchConfig.tuning`;既有 punch/信令编排。
- Produces:
  - `pub struct RetryTuning { pub max_reconnects: u32, pub backoff_base: Duration, pub recv_window: Duration }`(`Default` = 5/1s/300s;退避 = backoff_base << attempt)
  - `pub struct SendOpts { pub retry: RetryTuning, pub punch: punch::PunchConfig }` + Default
  - `pub struct RecvOpts { pub retry: RetryTuning, pub punch: punch::PunchConfig, #[doc(hidden)] pub fault_close_after_files: Option<usize> }` + Default
  - `pub async fn run_send(session, home, paths: &[PathBuf], to_device, out_report) -> Result<()>` = `run_send_with(.., &SendOpts::default())`
  - `pub async fn run_send_with(session, home, paths, to_device, out_report, opts: &SendOpts) -> Result<()>`
  - `pub async fn run_recv(session, home, out_dir) -> Result<Vec<PathBuf>>` = `run_recv_with(.., &RecvOpts::default())`
  - `pub async fn run_recv_with(session, home, out_dir, opts: &RecvOpts) -> Result<Vec<PathBuf>>`
  - `pub fn collect_items(paths: &[PathBuf]) -> Result<Vec<(PathBuf, String)>>`
- Task 7 消费 run_send 多路径与 main 接线。

- [ ] **Step 1: 写失败测试**

`crates/core/tests/signaling_client.rs` 追加(ClientEvent 统一):

```rust
#[tokio::test]
async fn server_drop_after_hello_yields_error_then_closed() {
    // 假服务器:完成 WS 握手并回 HelloOk 后直接断 TCP(无 Close 帧)
    // → 客户端事件流必须依次出现 Error(code=0, 含原因)与 Closed,而非流尽(None)
}
```

`crates/cli/src/flow.rs` tests 追加:

```rust
#[test]
fn only_network_errors_are_retryable() {
    // 终局错误不空耗重试轮次(Review Focus #4)
    assert!(retryable(&SendError::Network("x".into())));
    assert!(!retryable(&SendError::Rejected { reason: "disk".into() }));
    assert!(!retryable(&SendError::Protocol { reason: "rounds".into() }));
}

#[test]
fn collect_items_paths_and_errors() {
    // 目录展开:dir/{a.txt,sub/b.bin} + 单文件 c.txt →
    //   rel 集 = {"dir/a.txt","dir/sub/b.bin","c.txt"}(保输入序)
    // 不存在路径 → Err;空目录(仅此一项)→ Err;符号链接跳过;
    // 两输入产生相同 rel → Err;输入名 ".mtdss"(目录或文件)→ Err
}
```

`crates/cli/tests/cli_e2e.rs` 追加(复用 register_rest/既有信令+STUN 起法):

```rust
#[tokio::test]
async fn reconnect_after_mid_transfer_close_completes() {
    // B:run_recv_with(RecvOpts{ fault_close_after_files: Some(1), ..Default::default() })
    // A:run_send_with(SendOpts{ retry: RetryTuning{ backoff_base: 50ms, .. }, .. })
    // 3 文件(首文件完成后 B 主动断连)→ 断言:最终全部落盘、run_send Ok、
    // B 侧第二次连接内 .mtdss 记录被续传复用(断言 A 的总发送字节数 < 全量×2,可选)
}

#[tokio::test]
async fn failure_report_lists_original_paths_after_retries() {
    // B 上线接受首个请求后整个 recv 任务 abort(drop);
    // A:SendOpts{ retry: { max_reconnects: 2, backoff_base: 50ms, .. }, punch: { budget: 300ms, .. } }
    // → run_send Err;文案断言:含用户原始输入路径、含已完成/未完成清单、含文件名
}
```

- [ ] **Step 2: 跑测试确认失败**

Run: `cargo test -p mtdss-core --test signaling_client server_drop && cargo test -p mtdss-cli`
Expected: 编译失败(run_send_with 等不存在);signaling 测试失败(现仅 Closed/None)。

- [ ] **Step 3: 实现 client.rs 统一**

WS 任务所有退出路径收敛:每处 `break` 前必发事件——sink 写失败(两处)先 `ev_tx.send(Error{code:0, message:"signaling transport write failed"})` 再 `Closed`;keepalive 判死先 `Error{code:0,"signaling silent limit"}` 再 `Closed`;inbound 关闭(既有)前补 `Error{code:0,"signaling connection closed"}` 再 `Closed`。调用方语义不变:一律以 Closed/None 终止。

- [ ] **Step 4: 实现 flow.rs 编排**

1. `collect_items`:目录 → 递归 walk(`std::fs`,`read_dir` 按名排序保序),rel = `输入目录名/内部相对路径`;文件 → file_name;预检:不存在 Err、symlink 跳过+`tracing::warn`、空目录跳过(全空 Err)、跨输入 rel 重复 Err、输入 basename == `.mtdss` Err。
2. `run_send_with`:manifest 一次构建;attempt 循环(`0..=max_reconnects`,`attempt>0` 先 `sleep(retry.backoff_base * (1u32 << (attempt - 1)))`):每轮新信令连接(先 drop 旧 client/ev)→ HelloOk → ConnectRequest/Accept/候选交换 → punch(opts.punch) → `send_files`。`Ok` → out_report 汇总成功退出;`retryable(e)` 为真(仅 Network)→ 续轮;`Rejected/Protocol` → 立即终局。progress 闭包收 `FileDone`/最后 `Bytes` 快照(逐文件),终局时按 rel→原始输入路径映射(发起时由 paths 构建)生成成败清单文案,`Err(anyhow!(报告))`。
3. `run_recv_with`:外层 loop:信令连接 → HelloOk → wait_connect_request → setup/候选/punch(opts.punch)→ `transfer::recv_files_fault`(Task 4 交付,fault = opts.fault_close_after_files);连接/传输 Err → 若无 deadline 则置 `deadline = now + retry.recv_window`,未超窗继续 loop(信令死了也重建);超窗 → Err。首次等待无期限(仅断后起窗)。
4. 删 `sweep_part_files`(对账已接管陈旧 .part)。
5. 重试分类提取 `fn retryable(e: &SendError) -> bool`(仅 Network 为 true)供循环使用。

- [ ] **Step 5: 跑测试确认通过**

Run: `cargo test -p mtdss-cli && cargo test -p mtdss-core --test signaling_client`
Expected: 新旧全 PASS(含 3 个迁移后的 e2e)。

- [ ] **Step 6: 全量门禁 + 提交**

```bash
cargo test --workspace && cargo clippy --workspace --all-targets -- -D warnings
git add crates/ && git commit -m "feat(cli): reconnect orchestration, per-file failure report, client-event unification"
```

---

### Task 7: CLI 接线、e2e 与文档

**Files:**
- Modify: `crates/cli/src/main.rs`(Send 多路径)
- Modify: `docs/demo-m1.md`(send 用法)
- Test: `crates/cli/tests/cli_e2e.rs`(追加)

**Interfaces:**
- Consumes: Task 6 `run_send(paths)`/`run_recv -> Vec<PathBuf>`。

- [ ] **Step 1: 写失败测试**

```rust
#[tokio::test]
async fn cli_send_directory_and_files_multi_path() {
    // A 发送 [dir(含子目录+2 文件), 单文件];B recv --out
    // 断言:结构还原 dir/a.txt、dir/sub/b.bin、单文件名;内容一致;
    // out 下 .mtdss/ 存在且仅含记录(无 .part)
}
```

- [ ] **Step 2: 跑测试确认失败**

Run: `cargo test -p mtdss-cli --test cli_e2e multi_path`
Expected: 现 CLI 单路径,`mtdss send a b` 会因 clap 单值报错/流程不符 → FAIL。

- [ ] **Step 3: 实现**

`main.rs`:`Send { files: Vec<PathBuf>, #[arg(long)] to: String }`;调用 `run_send(&s, &home, &files, &to, &|m| println!("{m}"))`。`demo-m1.md` send 示例改 `mtdss send <文件或目录...> --to <device>`,补一句断点续传说明(中断后重发同一目录自动跳过已完成)。

- [ ] **Step 4: 跑测试确认通过 + 全量门禁**

```bash
cargo test --workspace && cargo clippy --workspace --all-targets -- -D warnings
```

- [ ] **Step 5: 提交**

```bash
git add crates/cli/ docs/demo-m1.md && git commit -m "feat(cli): multi-path and directory send"
```

---

## 完成后

1. 核对 spec §12 验收清单逐项打勾(静默断链、final_rel SKIP、终局不空耗、批量落盘等 Review Focus 项均已各归其测试)
2. M2c 分支经 PR 合入 main;随后启动 M2d(netns NAT 矩阵 + netem 混沌)brainstorm
