# MTDSS v2-P1a 实施计划:账号预置与认证收口

> **For agentic workers:** REQUIRED SUB-SKILL: Use superpowers:subagent-driven-development (recommended) or superpowers:executing-plans to implement this plan task-by-task. Steps use checkbox (`- [ ]`) syntax for tracking.

**Goal:** 把信令服务端的账号模型从"任意邮箱自动建号"收口为"运营者 CLI 预置制",并落地中继配置表与管理 CLI(中继数据面在 P1b)。

**Architecture:** signaling crate 内新增 `admin` 库模块(纯函数操作 `&Connection`,可单测)+ 薄 CLI bin `mtdss-admin`(手工参数解析,沿 `main.rs` 风格);认证收口改四个登录路径(request-code / verify / refresh / WS hello);测试全部改为先预置再登录。

**Tech Stack:** Rust + rusqlite(WAL)+ axum;无新增依赖。

**Spec:** docs/superpowers/specs/2026-10-22-mtdss-v2p1-relay-design.md(§6 决策 5/6/7、§6.1 裁定 A/B/F、§7.1 认证收口、§7.2 CLI、§8 P1a)

## Global Constraints

- 门禁:`cargo test --workspace` + `cargo clippy --workspace --all-targets -- -D warnings` 双平台(本机 Windows + WSL)+ nat-matrix + ui job,每任务提交前全绿
- 现网兼容:1.0.x 客户端零改动——生产库既有账号(历史自动建号)必须继续可登录;relay_config 无行的账号一律按 disabled + 默认值呈现
- 值钉死:默认 `rate_kbps = 1024`;默认 `quota_bytes = 10_737_418_240`(10 GiB);`0` = 不限;配额周期 period = UTC `"YYYY-MM"`(与北京时间有 ≤8h 月界偏差,接受)
- CLI 为服务器专用、输出面向人读;不引入 clap(沿 main.rs 手工解析风格)
- SQLite WAL:CLI 与 server 并存同库是常态,不得加 PRAGMA foreign_keys(级联用手写事务)

## Review Focus

1. **未预置邮箱 request-code → 404**(不是 400/500)——Task 3 测试 `unknown_email_gets_404` 钉住
2. **删号后 refresh 轮换的 30 天僵尸窗口**→ 401——Task 3 测试 `refresh_after_removal_rejected` 钉住(审查发现的现网真缺陷)
3. **删号后新 WS hello 被拒、已建立连接不被踢**——Task 3 测试 `ws_hello_after_removal_rejected` 断言 Error code 1;已连连接不动由"只改 hello 路径"的实现结构保证
4. **遗留账号无 relay_config 行**:user_list / relay enable 不炸、按 disabled+默认呈现——Task 2 测试 `legacy_account_shows_defaults` 钉住
5. **删除级联不彻底 → devices 残行复活 device_id 重绑旧账号历史 bug**——Task 1 测试 `remove_cascades_devices` 断言三表全空

---

### Task 1: DB schema + admin 账号生命周期

**Files:**
- Modify: `server/signaling/src/db.rs`(schema 批末尾追加两表)
- Create: `server/signaling/src/admin.rs`
- Modify: `server/signaling/src/lib.rs`(挂 `pub mod admin;`)
- Test: `server/signaling/tests/admin.rs`(新建)

**Interfaces:**
- Consumes: `db::open` / `db::now`(既有)
- Produces(Task 2/3/4 依赖):

```rust
// server/signaling/src/admin.rs
pub fn user_add(conn: &rusqlite::Connection, email: &str) -> anyhow::Result<bool>;
    // true=新建;false=已存在(幂等,不覆盖任何既有配置)
    // 新建时同步预建 relay_config 行:enabled=0 + 默认值(裁定 F)
pub fn user_remove(conn: &rusqlite::Connection, email: &str) -> anyhow::Result<bool>;
    // 单事务级联删 accounts+devices+codes+relay_config+relay_usage(裁定 A);true=删了,false=本就不存在
pub fn default_rate_kbps() -> u32;   // 1024
pub fn default_quota_bytes() -> i64; // 10_737_418_240
```

- [ ] **Step 1: 写失败测试 `tests/admin.rs`**

测试要点(全部用 `db::open(":memory:")` 直连):

```rust
#[test] fn add_creates_account_and_default_relay_config() {
    // user_add == true;accounts 行存在;relay_config 行存在且 enabled=0、rate=1024、quota=10_737_418_240
}
#[test] fn add_is_idempotent() { // 第二次返回 false;relay_config 不被重置
    // 先 relay_set 由 Task 2 引入,此处仅断第二次 add 后 enabled 仍为 0 行数仍 1
}
#[test] fn remove_cascades_devices_and_codes() {
    // add → 手工 INSERT devices 行(同 account_id)与 codes 行(同 email)→ remove == true
    // 断言四表(accounts/devices/codes/relay_config)对该账号全空;再 remove 返回 false
}
#[test] fn readd_mints_new_account_id() { // add → 记 id → remove → add → id 不同(白纸)
}
```

- [ ] **Step 2: 跑测试确认失败**

Run: `cargo test -p signaling --test admin`
Expected: FAIL(编译错误 `unresolved module admin`)

- [ ] **Step 3: 实现 schema 与 admin.rs**

`db.rs` 的 `execute_batch` 追加(Schema 钉死,含列默认):

```sql
CREATE TABLE IF NOT EXISTS relay_config(
  account_id TEXT PRIMARY KEY,
  enabled    INTEGER NOT NULL DEFAULT 0,
  rate_kbps  INTEGER NOT NULL DEFAULT 1024,
  quota_bytes INTEGER NOT NULL DEFAULT 10737418240,
  updated_at INTEGER NOT NULL);
CREATE TABLE IF NOT EXISTS relay_usage(
  account_id TEXT NOT NULL,
  period     TEXT NOT NULL,
  bytes_forwarded INTEGER NOT NULL DEFAULT 0,
  PRIMARY KEY(account_id, period));
```

`admin.rs`:id 生成沿 `auth.rs` 现状 `hex::encode(rand::random::<[u8; 16]>())`;user_add 用两条 `INSERT OR IGNORE`;user_remove 单个 `Connection::transaction` 内五条 DELETE(按 email 查 account_id,查不到直接返回 false)。

- [ ] **Step 4: 跑测试确认通过**

Run: `cargo test -p signaling --test admin`
Expected: PASS(4 个)

- [ ] **Step 5: Commit**

```bash
git add server/signaling/src/db.rs server/signaling/src/admin.rs server/signaling/src/lib.rs server/signaling/tests/admin.rs
git commit -m "feat(signaling): admin account lifecycle + relay config/usage schema"
```

### Task 2: relay 配置面 + user_list

**Files:**
- Modify: `server/signaling/src/admin.rs`
- Test: `server/signaling/tests/admin.rs`(追加)

**Interfaces:**
- Consumes: Task 1 的 `user_add`/`AccountRow`
- Produces(Task 4 CLI 依赖):

```rust
pub struct UserRow {
    pub email: String, pub account_id: String,
    pub relay_enabled: bool, pub rate_kbps: u32, pub quota_bytes: i64,
    pub used_bytes: i64, // 当期 relay_usage(LEFT JOIN COALESCE 0)
}
pub fn relay_set_enabled(conn: &Connection, email: &str, enabled: bool) -> anyhow::Result<()>;
    // 邮箱不存在 → Err("no such account");无 relay_config 行先 INSERT 默认再 UPDATE(遗留账号路径)
pub fn relay_set_limit(conn: &Connection, email: &str, kbps: u32) -> anyhow::Result<()>;  // 0=不限,原样存 0
pub fn relay_set_quota(conn: &Connection, email: &str, gib: u64) -> anyhow::Result<()>;   // 字节 = gib * 1024^3;0=不限
pub fn user_list(conn: &Connection) -> anyhow::Result<Vec<UserRow>>; // 按 email 排序;无行账号按 disabled+默认呈现
pub fn current_period() -> String; // UTC "YYYY-MM"
```

- [ ] **Step 1: 写失败测试**

```rust
#[test] fn enable_flips_and_is_idempotent() { // add→enable→enabled=1;再 enable 仍 1;邮箱不存在→Err }
#[test] fn limit_and_quota_persist() { // set_limit 512→行值 512;set_quota 5→5*1024^3;set 0 存 0 }
#[test] fn legacy_account_shows_defaults() {
    // add 后手工 DELETE relay_config 行(模拟生产遗留)→ user_list 仍列出、disabled+默认;enable 后行被补建
}
#[test] fn list_shows_current_period_usage() {
    // 无 usage 行 → used_bytes 0;手工 INSERT 当期 bytes_forwarded=123 → user_list 反映 123
    // 非当期(period="2000-01")行不计入
}
```

- [ ] **Step 2: 跑测试确认失败**

Run: `cargo test -p signaling --test admin`
Expected: FAIL(新函数未定义)

- [ ] **Step 3: 实现**

全部单条 SQL 即可,无需事务;`current_period` 用 `time` crate(Cargo.toml 已有)UTC 格式化;`used_bytes` 用子查询 `SELECT bytes_forwarded FROM relay_usage WHERE account_id=?1 AND period=?2`。

- [ ] **Step 4: 跑测试确认通过**

Run: `cargo test -p signaling --test admin`
Expected: PASS(8 个)

- [ ] **Step 5: Commit**

```bash
git add server/signaling/src/admin.rs server/signaling/tests/admin.rs
git commit -m "feat(signaling): relay config knobs + user list via admin module"
```

### Task 3: 认证收口(四条登录路径 + 测试面预置)

**Files:**
- Modify: `server/signaling/src/api.rs`(request-code 404)
- Modify: `server/signaling/src/auth.rs`(verify 去自动建号;refresh 补账号检查)
- Modify: `server/signaling/src/ws.rs`(hello 补账号检查)
- Test: `server/signaling/tests/auth.rs`(新增 4 个 + 既有修正)、`tests/devices.rs`、`tests/ws.rs`、`tests/plain.rs`、`tests/tls.rs`(登录前置预置)

**Interfaces:**
- Consumes: Task 1 `admin::user_add`
- Produces: 无新导出;行为契约——未知邮箱 request-code=**404**、verify(含已发码但账号被删)=**401**、refresh(账号被删)=**401**、WS hello(账号被删)=Error 帧 **code 1** `account removed`。不引入错误枚举:api 层自行前置查询区分 404,其余路径 anyhow 错误映射 401(现状已如此)。

- [ ] **Step 1: 写失败测试(tests/auth.rs 追加)**

```rust
#[tokio::test] async fn unknown_email_gets_404() { /* 未预置 email → POST request-code → 404 */ }
#[tokio::test] async fn verify_never_autocreates() {
    /* 预置→request-code 拿码→user_remove→verify(带码)→401;再直接查库断 accounts 无该 email 行 */
}
#[tokio::test] async fn refresh_after_removal_rejected() {
    /* 预置→完整登录拿 refresh→user_remove→POST /auth/refresh→401 */
}
#[tokio::test] async fn removed_account_cannot_get_new_code() { /* remove 后 request-code → 404 */ }
```

`tests/ws.rs` 追加:

```rust
#[tokio::test] async fn ws_hello_after_removal_rejected() {
    /* 走 login helper 完整登录并建 WS→user_remove(经 state.db)→同一 jwt+device_id 发起新 WS hello→收到 Error code 1 */
}
```

- [ ] **Step 2: 跑测试确认失败**

Run: `cargo test -p signaling`
Expected: 新 5 个 FAIL;**既有测试大面积 FAIL(未预置邮箱)**——这是本步骤的预期中间态

- [ ] **Step 3: 实现四路径收口 + 修测试面**

1. `api.rs::request_code`:调 `auth::request_code` 前查 `accounts` 存在,缺失返回 `(StatusCode::NOT_FOUND, "unknown account".into())`
2. `auth.rs::verify_code`:删自动建号分支——`query_row` Err 时返回 `Err(anyhow!("unknown account"))`(api 映射 401)
3. `auth.rs::rotate_refresh`:取得 account_id 后补 `SELECT 1 FROM accounts WHERE id=?`,缺失返回 Err(401)
4. `ws.rs` hello:owner 校验后补 accounts 存在检查,缺失 `err(1, "account removed")`
5. 测试面:五个测试文件中每个 request-code 前加 `admin::user_add(&state.db.lock().unwrap(), email).unwrap();`——`tests/devices.rs` 折进既有 `login` helper(一处);`tests/auth.rs`/`tests/ws.rs`/`tests/plain.rs`/`tests/tls.rs` 在各调用点前置;`auth.rs::jwt_roundtrip_and_refresh` 直接铸 JWT,无需预置

- [ ] **Step 4: 全量跑包测试确认通过**

Run: `cargo test -p signaling`
Expected: PASS(全绿,含新增 5 个)

- [ ] **Step 5: Commit**

```bash
git add server/signaling/src/api.rs server/signaling/src/auth.rs server/signaling/src/ws.rs server/signaling/tests
git commit -m "feat(signaling): auth closed to provisioned accounts (404/401/ws-hello gates)"
```

### Task 4: mtdss-admin CLI + 部署文档

**Files:**
- Create: `server/signaling/src/admin_main.rs`
- Modify: `server/signaling/Cargo.toml`(追加 `[[bin]]`)
- Modify: `scripts/deploy/README.md`(新增 mtdss-admin 小节)
- Test: `server/signaling/tests/admin_cli.rs`(新建)

**Interfaces:**
- Consumes: Task 1/2 全部 admin 函数
- Produces: 可执行文件 `mtdss-admin`,语法钉死:

```
mtdss-admin [--db <path>] user add <email> | user remove <email> | user list [--with-usage]
mtdss-admin [--db <path>] relay enable <email> | relay disable <email> | relay limit <email> <kbps> | relay quota <email> <GB>
```

`--db` 默认 `"signaling.db"`(与 server 默认一致);未知命令/参数 → 非 0 退出 + USAGE 到 stderr;输出面向人读(每命令一行确认;list 为对齐表格;`--with-usage` 追加当期用量列,默认不显)。参数解析沿 `main.rs` 手工风格(USAGE 常量 + bail),不引 clap。

- [ ] **Step 1: 写失败集成测试 `tests/admin_cli.rs`**

用 `env!("CARGO_BIN_EXE_mtdss-admin")` + `tempfile::tempdir` 建临时库,`std::process::Command` 串联:

```rust
#[test] fn cli_happy_chain() {
    // user add a@b.c → exit 0,stdout 含 "a@b.c"
    // user list → 含 a@b.c 且含 "disabled"
    // relay enable a@b.c → 0;relay limit a@b.c 512 → 0;relay quota a@b.c 5 → 0
    // user list --with-usage → 此时限速列应为 "512"、配额列 "5"(不再是默认 1024/10)
    // user remove a@b.c → 0;user list → 不含 a@b.c
}
#[test] fn cli_rejects_unknown_command() { // exit != 0 且 stderr 含 usage
}
```

- [ ] **Step 2: 跑测试确认失败**

Run: `cargo test -p signaling --test admin_cli`
Expected: FAIL(`CARGO_BIN_EXE_mtdss-admin` 未定义——bin 尚不存在)

- [ ] **Step 3: 实现 bin + Cargo.toml + README**

`[[bin]] name = "mtdss-admin" path = "src/admin_main.rs"`;main 为纯参数解析薄壳,全部逻辑调用 admin 模块;`scripts/deploy/README.md` 增小节:命令表、"先建号再能登录"、WAL 并存(server 运行中可直接执行)、生产遗留账号按 disabled+默认呈现的说明。

- [ ] **Step 4: 跑测试确认通过**

Run: `cargo test -p signaling --test admin_cli`
Expected: PASS(2 个)

- [ ] **Step 5: 终门禁 + Commit**

Run: `cargo test --workspace && cargo clippy --workspace --all-targets -- -D warnings`(本机;WSL 同跑)
Expected: 全绿(nat-matrix/ui job 无涉改动,按门禁照跑)

```bash
git add server/signaling/src/admin_main.rs server/signaling/Cargo.toml scripts/deploy/README.md server/signaling/tests/admin_cli.rs
git commit -m "feat(signaling): mtdss-admin provision CLI + deploy docs"
```
