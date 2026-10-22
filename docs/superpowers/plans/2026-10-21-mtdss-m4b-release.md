# MTDSS M4b(打包+更新+服务端)Implementation Plan

> **For agentic workers:** REQUIRED SUB-SKILL: Use superpowers:subagent-driven-development (recommended) or superpowers:executing-plans to implement this plan task-by-task. Steps use checkbox (`- [ ]`) syntax for tracking.

**Goal:** 把 M4a 收官后的工程变成可交付产品:清掉 M4a 终审移交的四笔债,版本升至 1.0.0,建 Windows NSIS 安装包(三件套同目录)、daemon 主导的 ed25519 签名更新通道(仅 Windows)、Caddy 统一入口的服务端资产与 tag 驱动的 release workflow,并交付 smoke-m4 手册与发布演练。

**Architecture:** 更新通道 = daemon 全权编排(`crates/daemon/src/update.rs` 状态机:检查→下载→验签→空闲门→安装契约),manifest 类型与 canonical ed25519 验签放 `mtdss-core::update`(签名助手 `mtdss-sign` 复用);IPC 面零 proto 改动(update.* 为 JSON-RPC match 臂 + `event.update` 字符串事件);安装契约 = MTDSS_HOME 标记文件(update-in-progress/restart-ui,原子写)+ ipc 连接计数(等 UI 断开 ≤5s)+ NSIS 钩子(装前轮询等退出 ≤15s 强杀兜底,装后拉起 mtdssd,卸载杀进程+清 HKCU Run 保数据);服务端 = signaling 加 `--plain-http` + scripts/deploy 资产(Caddy 443 统一入口);信令默认地址与更新 base URL 为两个独立编译期常量(`.invalid` 占位 + 构建/运行时 env 覆盖,测试永不触生产)。

**Tech Stack:** 既有栈 + `semver` 不引入(手写 X.Y.Z 三段比较);`mtdss-sign` = 新工作区成员(ed25519-dalek 既有);UI 不新增 npm 依赖;NSIS = tauri bundler(`bundle.windows.nsis` + `installerHooks` + resources map 形态)。

**Spec:** `docs/superpowers/specs/2026-10-20-mtdss-m4-release-design.md`(§7.2 打包、§7.3 更新体系、§7.4 服务端与发布、§7.5 验证、§8 M4b 切片与移交债;D1–D15 裁定)

## Global Constraints

- 门禁 = `cargo test --workspace` + `cargo clippy --workspace --all-targets -- -D warnings` 在 Windows 与 Linux(WSL/CI)双平台全绿 + nat-matrix CI job + ui job 全绿。**每个任务提交前门禁必须全绿。**
- UI 门禁:`cd ui && npx vitest run && npm run build`;ui 是独立 Tauri v2 crate(不入主 workspace,终门禁含 `cargo check --manifest-path ui/src-tauri/Cargo.toml`,check 前需先 build 出 ../dist)。
- 本机 bash 环境:`export PATH="$HOME/.cargo/bin:$PATH"; export PROTOC="/c/Users/Administrator/AppData/Local/Microsoft/WinGet/Packages/Google.Protobuf_Microsoft.Winget.Source_8wekyb3d8bbwe/bin/protoc.exe"`(新终端 PATH 陈旧时先执行前半句自救)。
- cargo 走 rsproxy.cn;npm 慢时临时用 npmmirror,**不得把 .npmrc 提交进仓库**;git 走 SSH;gh 需 `export PATH="/c/Program Files/GitHub CLI:$PATH"`。
- 执行在 worktree(`EnterWorktree` 分支 `worktree-mtdss-m4b`)。
- 版本 = 1.0.0(Task 6 起);四处版本位(根 `Cargo.toml:9` workspace.version、`ui/src-tauri/tauri.conf.json:4`、`ui/src-tauri/Cargo.toml:3`、`ui/package.json:4`)必须一致,release workflow 校验。
- 文案中文(沿现界面/CLI 全中文口径);错误码沿既有约定(RPC:-32601/-32602/1 业务/3 对端离线;CLI 退出码 0 成功/2 前置/1 其余)。
- 测试永不触生产端点:一切更新/信令测试用本地测试服或注入显式配置;默认常量占位 `.invalid` 域。
- secrets 时点(用户动作,非本计划任务):①签名密钥对 = M4b 冒烟前由用户生成并保管私钥、公钥入仓(Task 19 runbook);②部署 ssh key = 首次发布演练前;③域名 = 商业发布前。

## Review Focus

1. **验签失败必须闭门**:签名不匹配/被篡改 URL 与 hash/公钥未配置 → 客户端一律拒绝,不信任 manifest 任何域,系统零改动可重试 → Task 7(篡改/坏版本号单测)+ Task 11(坏签名/坏 sha256/公钥空 e2e)钉死。
2. **半安装态防护全路径**:复制中途拉起 ui/cli/mtdssd → spawn 守卫拒绝;新 daemon 版本不等 → warn 后清标记;NSIS 装前 15s 轮询 + taskkill 兜底 → Task 12(守卫/消费单测)+ Task 16(钩子步)钉死。
3. **空闲门全档位生效**:「活跃」= 未到终态 job(含 paused;stalled 即 running),auto_install 静默档同样守门;CLI 手动更新硬拒文案+退出码 1 → Task 10(active_count 含 paused 测试)+ Task 11(空闲门 e2e)+ Task 13(CLI 硬拒测试)钉死。
4. **测试/演练不触生产**:常量 `.invalid` 占位、构建期 option_env!/运行时 env 双覆盖、测试一律注入显式 base_url → Task 6(覆盖序单测)+ Task 11(测试服注入)钉死。
5. **tar 平台只发不更 + 无 UI 依赖**:非 Windows update.* 报 UNSUPPORTED;CLI-only(无 UI)更新全链可达;M4 允许降级(回滚走版本钉扎)→ Task 11(平台拒绝/降级接受测试)+ Task 13(CLI 链路测试)钉死。

---

### Task 1: 移交债①(daemon 面)— 同设备本机复制走作业面 + 策略

**Files:**
- Modify: `crates/daemon/src/jobs.rs`(start_local_copy :1049-1147、local_copy_driver :1876-1928、JobSnap :285-297、JobCore::emit_locked :389-405)
- Modify: `crates/daemon/src/api.rs`(transfer.start local_copy 臂 :499-514 收 policy,复用 policy_from_params :789-799)
- Modify: `crates/cli/src/main.rs`(finish_transfer :410-428 增跳过计数行)
- Test: `crates/daemon/tests/jobs_e2e.rs`(沿 :1020 local_copy_job 形态)、`crates/cli/tests/ipc_cli.rs`

**Interfaces:**
- Consumes: `TransferPolicy { skip, overwrite }`(core intake.rs:191);`copy_paths_with/move_entries_with → CopyOutcome/MoveOutcome { copied/moved, failed, skipped }`(core fs.rs:475-512);`check_policy_limits`(jobs.rs:185)。
- Produces: `Jobs::start_local_copy(&self, state, paths: Vec<PathBuf>, dir: &str, cut: bool, policy: TransferPolicy) -> Result<String, String>`(六参);driver 内 cut=false → `copy_paths_with`、true → `move_entries_with`;outcome.failed 非空 → job error 串(沿现失败表形态)且 state=failed、空 → done;`JobSnap` 增 `pub skipped: u64`(各 kind 缺省 0),event.transfer 与 `job_json`(api.rs:544)params 增 `"skipped"`。RPC `transfer.start {kind:"local_copy", paths, dir, cut, skip?, overwrite?}`。

- [ ] **Step 1: 写失败测试** — ①jobs_e2e:`local_copy_job_policy_skip`(同目录同名三项 + skip 命中一项 → job done、目标缺该项、snapshot.skipped==1、`transfer.wait` 终态含 skipped);`local_copy_cut_policy_overwrite`(cut+overwrite 命中 → 目标内容翻新、源不在);`local_copy_failures_error`(一项不可读 → failed、error 含路径)。②ipc_cli:local_copy 作业完成后输出含「跳过 1 项」。
- [ ] **Step 2: 跑测试确认失败** — Run: `cargo test -p mtdss-daemon --test jobs_e2e local_copy` 与 `cargo test -p mtdss-cli --test ipc_cli -- local_copy` Expected: FAIL(参数不匹配/字段不存在)
- [ ] **Step 3: 实现** — 六参签名贯通;driver 换 `*_with`;JobSnap/emit/job_json 增 skipped;api.rs local_copy 臂经 policy_from_params(自带 limits 校验);CLI finish_transfer 在 done 分支按 skipped>0 打印「(跳过 N 项)」。
- [ ] **Step 4: 跑测试确认通过 + 回归** — Run: `cargo test -p mtdss-daemon -p mtdss-cli` Expected: PASS(含既有 local_copy_job / transfer 面不破坏)
- [ ] **Step 5: 门禁 + Commit** — `feat(daemon+cli): same-device local copy as a job with conflict policy (card/progress/skip count; kills 35s RPC face)`

### Task 2: 移交债①(UI 面)— 本机粘贴回迁 local_copy 作业卡

**Files:**
- Modify: `ui/src/lib/clipboard.ts`(pastePlan local→local 臂 :58-69)
- Modify: `ui/src/App.tsx`(executePlan :789-806 device_copy 本机臂、模块注释 :27-29)
- Modify: `ui/src/lib/daemon.ts`(transferStart 参数类型增 policy)
- Test: `ui/src/lib/paste.test.ts`(沿 M4a planSameDeviceOp 测试形态)

**Interfaces:**
- Consumes: Task 1 的 RPC 参数;既有 `planSameDeviceOp` 冲突流(App.tsx pendingConflict :229 附近)、ConflictDialog 四键、transfer 卡管线(`transfer.kind.local_copy` 标签已存在 zh.json:106)。
- Produces: pastePlan local→local 返回 `{ kind: "local_copy", paths, dir, cut }`(policy 由执行期冲突流决定);App `case "local_copy"` → 冲突扫描(沿 planSameDeviceOp)→ 有冲突走 ConflictDialog → `daemonTransferStart({kind:"local_copy", paths, dir, cut, skip?, overwrite?})` → 卡片由 event.transfer 管线自然呈现。

- [ ] **Step 1: 写失败测试** — vitest:local→local pastePlan 产出 kind=local_copy(不再 device_copy);exec 纯函数带 policy 时参数含 skip/overwrite 数组。
- [ ] **Step 2: 跑测试确认失败** — Run: `cd ui && npx vitest run` Expected: FAIL
- [ ] **Step 3: 实现** — clipboard.ts 改 kind 与注释;App.tsx 增 local_copy case(冲突流复用)+ 更新模块注释「local_copy 作业面自 M4b 起带策略参数,回迁」;daemon.ts transferStart 类型扩展。
- [ ] **Step 4: 跑测试确认通过 + build** — Run: `cd ui && npx vitest run && npm run build` Expected: PASS
- [ ] **Step 5: 门禁 + Commit** — `feat(ui): local paste routes to local_copy job (card + progress + conflict policy)`

### Task 3: 移交债② — mv/cp 失败并存时跳过计数可见

**Files:**
- Modify: `crates/cli/src/main.rs`(同机复制 :357-358、mv :520-521、report_skipped_done :550-559、report_failed :528-548)
- Test: `crates/cli/tests/ipc_cli.rs`

**Interfaces:**
- Produces: 调用序改为「先 `report_skipped_done`(始终打印完成+跳过行)再 `report_failed(...)?`」;两函数本体不动。远端 cp/mv(copy_remote_cmd/mv_cmd 路径)同型调用点一并检查修齐。

- [ ] **Step 1: 写失败测试** — ipc_cli:构造 failed+skipped 并存的 copy 响应(沿现有管道测试服形态)→ stdout 同时含「跳过 N 项」与「部分失败」两行。
- [ ] **Step 2: 跑测试确认失败** — Run: `cargo test -p mtdss-cli --test ipc_cli -- skip` Expected: FAIL(跳过行缺失)
- [ ] **Step 3: 实现** — 交换两处调用序 + 排查其余 report_failed? 后置调用点。
- [ ] **Step 4: 跑测试确认通过 + 回归** — Run: `cargo test -p mtdss-cli` Expected: PASS
- [ ] **Step 5: 门禁 + Commit** — `fix(cli): skip count visible even when copy/move has failures`

### Task 4: 移交债③ — ctrl responder 补 check_policy_limits 对称

**Files:**
- Modify: `crates/daemon/src/ctrl.rs`(handle_ctrl_request Copy 臂 :1030-1040、Move 臂 :1016-1029)
- Test: `crates/daemon/src/ctrl.rs`(mod tests :1114+)

**Interfaces:**
- Consumes: `check_policy_limits(&TransferPolicy) -> Result<(), String>`(jobs.rs:185,pub(crate) 同 crate 可直调);`policy_engine`(ctrl.rs:740)。
- Produces: 两臂在 `policy_engine` 转换后立即 `check_policy_limits`,Err → `CtrlResponse { ok:false, body: error }`,`CtrlError.code = "POLICY_LIMIT"`(code 为 string,零 proto 改动)。

- [ ] **Step 1: 写失败测试** — ctrl.rs tests:`responder_rejects_oversized_policy`(MoveRequest 带 2049 条 skip → ok=false、code=POLICY_LIMIT、目标目录零改动);上限边界 2048 条通过。
- [ ] **Step 2: 跑测试确认失败** — Run: `cargo test -p mtdss-daemon --lib responder_rejects` Expected: FAIL(现放行)
- [ ] **Step 3: 实现** — 两臂各加 3 行检查 + 错误映射。
- [ ] **Step 4: 跑测试确认通过 + 回归** — Run: `cargo test -p mtdss-daemon`(含 ctrl_e2e 全面)Expected: PASS
- [ ] **Step 5: 门禁 + Commit** — `fix(daemon): ctrl responder enforces policy limits (POLICY_LIMIT error, symmetry with initiator)`

### Task 5: 移交债④ — jobs 测试 panic-hook 串行化

**Files:**
- Modify: `crates/daemon/src/jobs.rs`(tests :2638 lock_ok_recovers_poison、:2660 panicking_driver_marks_failed)

**Interfaces:**
- Produces: tests 模块内 `static PANIC_HOOK_LOCK: std::sync::Mutex<()>`;两个测试在 take_hook 前取守卫(锁保持到 set_hook 恢复后),零新依赖。

- [ ] **Step 1: 实现 + 验证** — 加锁守卫包住三处 hook 换装(:2641-2649/:2677-2680/:2695-2698);Run: `cargo test -p mtdss-daemon --lib -- --test-threads=8` 重复 3 遍 Expected: PASS 且无交错 panic 噪音。
- [ ] **Step 2: 门禁 + Commit** — `test(daemon): serialize panic-hook swaps in jobs tests (global hook race)`

### Task 6: 版本 1.0.0 + 两个独立编译期常量 + `--server` 可选

**Files:**
- Modify: `Cargo.toml`(:9 version="1.0.0")、`ui/src-tauri/tauri.conf.json:4`、`ui/src-tauri/Cargo.toml:3`、`ui/package.json:4`
- Create: `crates/ipc/src/consts.rs`(模块注册进 lib.rs)
- Modify: `crates/cli/src/main.rs`(Login.server :45-48 改 Option + 缺省文案)、`crates/cli/src/client.rs`(login_request_code :178-193 收 Option<&str> 落常量默认)
- Test: `crates/ipc/tests/ipc.rs` 或 consts 模块内单测

**Interfaces:**
- Produces(consts.rs):
  ```rust
  pub const DEFAULT_SIGNALING_URL: &str = option_env!("MTDSS_SIGNALING_URL").unwrap_or("https://mtdss-signaling.invalid");
  pub const DEFAULT_UPDATE_URL: &str = option_env!("MTDSS_UPDATE_URL").unwrap_or("https://mtdss-update.invalid");
  pub fn signaling_url() -> String { std::env::var("MTDSS_SIGNALING_URL").unwrap_or_else(|_| DEFAULT_SIGNALING_URL.into()) }
  pub fn update_url() -> String { std::env::var("MTDSS_UPDATE_URL").unwrap_or_else(|_| DEFAULT_UPDATE_URL.into()) }
  ```
  覆盖序:运行时 env → 构建期 env → `.invalid` 占位(保证未注入时绝不触真实端点)。`--ca` 本就是 Option 不动;`login --server` 缺省走 `signaling_url()`。

- [ ] **Step 1: 写失败测试** — 单测:set_var 覆盖生效、clear 后回落常量;四处版本位一致性断言(读文件比较)。
- [ ] **Step 2: 跑测试确认失败** — Run: `cargo test -p mtdss-ipc consts` Expected: FAIL(模块不存在)
- [ ] **Step 3: 实现** — consts.rs + 四处版本位 + cli server 可选(usage 文案注明缺省值来源)。
- [ ] **Step 4: 跑测试确认通过 + 回归** — Run: `cargo test --workspace` 与 `cd ui && npm run build` Expected: PASS
- [ ] **Step 5: 门禁 + Commit** — `feat(core consts): v1.0.0 + independent signaling/update default URLs (env-overridable, .invalid placeholders)`

### Task 7: `mtdss-core::update` — manifest 类型 + canonical 验签 + 版本比较

**Files:**
- Create: `crates/core/src/update.rs`(lib.rs 注册 mod)
- Modify: `crates/core/Cargo.toml`(若无 ed25519-dalek/sha2 则补 workspace 依赖;core 已有 sha2 与签名体系,预计零改动)
- Test: `crates/core/src/update.rs` 模块内单测

**Interfaces:**
- Produces:
  ```rust
  pub struct PlatformAsset { pub url: String, pub sha256: String /*hex*/, pub size: u64 }
  pub struct UpdateManifest {
      pub version: String, pub notes: String, pub pubdate: String,
      pub platforms: std::collections::BTreeMap<String, PlatformAsset>, // key "windows-x86_64"
      pub signature: String, // hex(ed25519) over canonical(除 signature 外全域)
  }
  impl UpdateManifest {
      pub fn stripped_value(&self) -> serde_json::Value;               // {version,notes,pubdate,platforms}
      pub fn canonical_bytes(&self) -> Vec<u8>;                        // stripped_value 的紧凑 JSON(键排序,serde_json 默认 BTreeMap,禁 preserve_order feature)
      pub fn verify(&self, vk: &ed25519_dalek::VerifyingKey) -> bool;  // canonical_bytes + hex signature
  }
  pub fn sign_manifest(m: &mut UpdateManifest, sk: &ed25519_dalek::SigningKey);
  pub fn version_cmp(a: &str, b: &str) -> Option<std::cmp::Ordering>;  // 严格 "X.Y.Z" 三段 u64;非法 → None(调用方按无效 manifest 拒绝)
  pub fn asset_sha256_hex(bytes: &[u8]) -> String;                     // 下载复核用
  ```
  serde 依赖:core 已有;ed25519-dalek = workspace 2.x。

- [ ] **Step 1: 写失败测试** — ①`sign_verify_roundtrip` + 篡改任意域(version/url/sha256/pubdate)→ verify false;②`version_cmp`("1.0.0"<"1.0.1"、"1.2.0">"1.10.0" 字典序陷阱、"1.0"→None、"1.0.0-rc1"→None);③canonical 字节确定性(两次序列化相等、键序与白板 JSON 一致)。
- [ ] **Step 2: 跑测试确认失败** — Run: `cargo test -p mtdss-core update` Expected: FAIL(模块不存在)
- [ ] **Step 3: 实现** — 如上签名;verify 对 hex 解码失败/坏长度一律 false。
- [ ] **Step 4: 跑测试确认通过** — Run: `cargo test -p mtdss-core` Expected: PASS
- [ ] **Step 5: 门禁 + Commit** — `feat(core): signed update manifest types (canonical ed25519, strict version compare)`

### Task 8: `mtdss-sign` — 签名助手(工作区新成员)

**Files:**
- Create: `crates/mtdss-sign/Cargo.toml`(bin `mtdss-sign`,deps: mtdss-core path、clap、serde_json、hex、ed25519-dalek、sha2、anyhow)
- Create: `crates/mtdss-sign/src/main.rs`
- Modify: 根 `Cargo.toml` members + workspace.dependencies(clap/sha2/hex 已有)
- Test: `crates/mtdss-sign/tests/sign_cli.rs`(std::process::Command 跑自身二进制)

**Interfaces:**
- Consumes: Task 7 全部。
- Produces: CLI 子命令 —— `mtdss-sign keygen [--out <dir>]`(打印/落盘 update-pub.hex + update-priv.hex,已存在则拒);`mtdss-sign manifest --version <v> --notes-file <f> --url-base <https://host/dl> --file <setup.exe> [--platform windows-x86_64] --out <unsigned.json>`(算 sha256+size,asset url = `{url-base}/win/<ver>/<file 名>`,pubdate=now UTC RFC3339);`mtdss-sign sign --manifest <unsigned.json> --key <priv.hex> --out <manifest.json>`;`mtdss-sign verify --manifest <manifest.json> --pubkey <pub.hex>`(退出码 0/1)。

- [ ] **Step 1: 写失败测试** — sign_cli.rs:keygen→manifest→sign→verify 全链退出码 0;篡改 manifest.json 一字符 → verify 退出码 1;--version 非三段 → manifest 子命令报错。
- [ ] **Step 2: 跑测试确认失败** — Run: `cargo test -p mtdss-sign` Expected: FAIL(crate 不存在)
- [ ] **Step 3: 实现** — clap derive 四子命令;密钥 hex 编解码;manifest 子命令内嵌 version 校验(version_cmp Some 才收)。
- [ ] **Step 4: 跑测试确认通过** — Run: `cargo test -p mtdss-sign` Expected: PASS
- [ ] **Step 5: 门禁 + Commit** — `feat(mtdss-sign): ed25519 manifest signing tool (keygen/manifest/sign/verify)`

### Task 9: signaling `--plain-http <addr>`

**Files:**
- Modify: `server/signaling/src/main.rs`(argv 循环 :16-42、TLS 必填分支 :59-64、硬编码地址 :76)
- Test: `server/signaling/tests/tls.rs` 或新 `plain.rs`

**Interfaces:**
- Produces: `--plain-http <ip:port>` 与 `--tls-cert/--tls-key` 三选一(二者都给 → bail;都缺 → 原 bail 文案不变);plain 路径 `axum::serve(TcpListener::bind(addr), app)`(沿 spawn_test_server_with 先例 lib.rs:160-167);TLS 分支保持硬编码 0.0.0.0:8787 不动。usage 串更新。客户端零改动(ws_url 已支持 http:// 前缀)。

- [ ] **Step 1: 写失败测试** — 测试:spawn `signaling-server --plain-http 127.0.0.1:0`?端口不可探测则用固定高端口 + `--dev-gen-tls` 单测形态:库层暴露 `pub async fn serve_plain(app, addr)`;test 起 plain 服务 → reqwest `http://...` GET /devices 返回 401(可达即证)。双旗标冲突 → 进程退出码非 0 + usage。
- [ ] **Step 2: 跑测试确认失败** — Run: `cargo test -p signaling` Expected: FAIL(serve_plain 不存在)
- [ ] **Step 3: 实现** — tls.rs 或 lib.rs 增 serve_plain + main.rs 三路分支。
- [ ] **Step 4: 跑测试确认通过 + 回归** — Run: `cargo test -p signaling` Expected: PASS(TLS 面不破坏)
- [ ] **Step 5: 门禁 + Commit** — `feat(signaling): --plain-http listen mode (loopback behind Caddy; TLS mode unchanged)`

### Task 10: daemon 更新设置 + prefs RPC + Jobs::active_count

**Files:**
- Modify: `crates/daemon/src/settings.rs`(Settings :15-22)
- Modify: `crates/daemon/src/state.rs`(增访问器,沿 set_autostart :192 形态)
- Modify: `crates/daemon/src/api.rs`(dispatch 增两臂;沿 autostart_get :867/set :874 惯例)
- Modify: `crates/daemon/src/jobs.rs`(增 `pub fn active_count(&self) -> usize`,读 CountWatch 原子值;count_rx 的 cfg 门不动)
- Test: `crates/daemon/tests/daemon_e2e.rs`(沿 autostart 测试形态)

**Interfaces:**
- Produces: `Settings { autostart, recv_dir, #[serde(default = "default_true")] update_auto_check: bool, #[serde(default)] update_auto_install: bool }`(default_true fn 返回 true;D8 默认开/关);`DaemonState::update_auto_check()/update_auto_install()/set_update_check(bool)/set_update_install(bool) -> Result<()>`(set_update_check(false) 连带强制 update_auto_install=false,D4 父子耦合收口在 daemon 一处);RPC `update.prefs.get {}` → `{"auto_check":b,"auto_install":b}`;`update.prefs.set {auto_check?, auto_install?}` → `{}`(Option<bool>,缺省不动);`Jobs::active_count` = 非终态 job 数(paused 计入)。

- [ ] **Step 1: 写失败测试** — ①settings:老 settings.json(无新字段)加载 → auto_check=true/auto_install=false;set_update_check(false) 后 auto_install 持久化 false。②RPC:prefs.set auto_install=true(auto_check=true)→ get 回读一致;set auto_check=false → auto_install 一并 false。③active_count:构造 paused + terminal 混合 → 计数只含未终态。
- [ ] **Step 2: 跑测试确认失败** — Run: `cargo test -p mtdss-daemon -- prefs active_count` Expected: FAIL
- [ ] **Step 3: 实现** — 如上;prefs.set 走 need_bool 可选参数(opt bool helper 沿 opt_str 形态新增)。
- [ ] **Step 4: 跑测试确认通过 + 回归** — Run: `cargo test -p mtdss-daemon` Expected: PASS
- [ ] **Step 5: 门禁 + Commit** — `feat(daemon): update prefs settings + RPC + jobs active_count (paused counts as active)`

### Task 11: daemon 更新核心 — 状态机/检查/下载/验签/空闲门(不 spawn)

**Files:**
- Create: `crates/daemon/src/update.rs`(lib.rs 注册;`#![cfg(windows)]` 顶部 + api.rs 非 Windows 臂报 UNSUPPORTED)
- Modify: `crates/daemon/src/state.rs`(DaemonState 增 `update: Arc<UpdateService>`)
- Modify: `crates/daemon/src/lib.rs`(Daemon::start 拉起 update 任务:启动检查 + 24h 定时,auto_check 门)
- Modify: `crates/daemon/src/api.rs`(dispatch 增 `update.check`/`update.state`/`update.apply` 三臂)
- Modify: `crates/daemon/Cargo.toml`(补 sha2 workspace 依赖;reqwest/ed25519-dalek 已有)
- Test: `crates/daemon/tests/update_e2e.rs`(新;本地 axum 静态服,dev-deps 已有 axum/hyper/tower)

**Interfaces:**
- Consumes: Task 7(core::update)、Task 6(ipc::consts::update_url)、Task 10(active_count、prefs)。
- Produces:
  ```rust
  pub(crate) struct UpdateConfig { pub base_url: String, pub ca: Option<PathBuf>, pub pubkey: Option<ed25519_dalek::VerifyingKey>, pub current: String }
  pub(crate) struct UpdateService { /* cfg + Mutex<UpdateSt> + 事件发送器 */ }
  // 状态 JSON(事件与 update.state 同形):
  // {"state":"idle|available|downloading|ready|pending_install|applying|installing|failed",
  //  "current":"1.0.0","latest":opt,"notes":opt,"progress":opt(0.0-1.0,>=400ms 节流沿 PROGRESS_EVENT_MIN_GAP 口径),"error":opt}
  impl UpdateService {
      pub fn new(cfg: UpdateConfig, events: broadcast::Sender<EventFrame>) -> Self;   // 生产:base=ipc::consts::update_url(), ca=env MTDSS_UPDATE_CA, pubkey=option_env!("MTDSS_UPDATE_PUBKEY") hex(空→None→更新未配置), current=env!("CARGO_PKG_VERSION")
      pub async fn check(&self, force: bool) -> Result<Value, RpcError>;              // 拉 {base}/manifest.json(force)或 {base}/win/{ver}/manifest.json(钉扎);验签失败/版本非法→Err;较新→state=available+event;旧/等→idle
      pub fn state(&self) -> Value;
      pub async fn apply(&self, version: Option<String>, state: &Arc<DaemonState>) -> Result<Value, RpcError>; // idle 门:state.jobs.active_count()>0 → Err(code1, data.kind="JOBS_ACTIVE", message="N 个传输中(含已暂停),可先取消作业后重试") 且转 pending_install(job 终态事件或下轮 check 重试);通过→downloading→sha256/size 复核→ready;本任务止于 ready(applying/installing 属 Task 12)
  }
  ```
  RPC:`update.check {}`(即强制检查)→ check 结果;`update.state {}`;`update.apply {version?}`。事件 `const EV_UPDATE: &str = "event.update"`。下载落 `{home}/downloads/mtdss-setup-<ver>.exe`(临时名 + 复核通过后 rename;失败删除)。失败路径:任何 Err → state=failed + event,**系统零改动**(下载文件不算系统改动,失败即删)。

- [ ] **Step 1: 写失败测试** — 测试服(注入 UpdateConfig{base_url=本地服, pubkey=测试钥}):①happy:签好 manifest(1.0.1)+假 setup 字节 → check available → apply(空闲)→ ready、下载文件 sha 复核过;②坏签名(换钥签)→ check Err、state=failed、无下载;③sha 不符 → apply Err on 复核、临时文件已删;④公钥 None → check Err「更新未配置」;⑤空闲门:预置一个 paused job → apply Err kind=JOBS_ACTIVE 文案精确;job 转 done 后(模拟终态事件)重试过门;⑥降级:apply(Some("0.9.0")) 走钉扎 URL 且被接受(M4 无防回滚);⑦进度事件节流 ≥400ms(沿 jobs 时序测试教训:机制不等式不断言绝对时长)。测试全程零 env 依赖(显式注入)。
- [ ] **Step 2: 跑测试确认失败** — Run: `cargo test -p mtdss-daemon --test update_e2e` Expected: FAIL(模块不存在)
- [ ] **Step 3: 实现** — 如上;auto_install 静默档 = 定时 check 发现新版且 prefs.auto_install → 直接 apply(仍守空闲门);仅 auto_check → available 时 notify_one + event(UI toast)。24h 定时用 tokio interval,daemon 启动即首查。
- [ ] **Step 4: 跑测试确认通过 + 回归** — Run: `cargo test -p mtdss-daemon` Expected: PASS
- [ ] **Step 5: 门禁 + Commit** — `feat(daemon): update service — check/download/verify/idle-gate state machine (ed25519, throttled events, no-touch failure paths)`

### Task 12: 安装契约 — 标记/连接计数/编排/守卫(D11)

**Files:**
- Modify: `crates/ipc/src/lib.rs`(serve :250 连接计数 + 标记助手)
- Modify: `crates/daemon/src/update.rs`(apply 的 Installing 序列)
- Modify: `crates/daemon/src/lib.rs`(Daemon::start 开机消费 update-in-progress;shutdown 复用)
- Modify: `crates/daemon/src/uilaunch.rs`(open_main_window :78 前置守卫 + 增 `open_update_window()`)
- Modify: `crates/cli/src/client.rs`(spawn_daemon :153 守卫)、`ui/src-tauri/src/ipc_bridge.rs`(spawn_daemon :102 守卫)
- Test: `crates/ipc/tests/ipc.rs`(标记助手/计数)、`crates/daemon/tests/update_e2e.rs`(消费/编排不真正 spawn 的注入点)

**Interfaces:**
- Produces(ipc):
  ```rust
  pub fn update_marker_path(home: &Path) -> PathBuf;   // {home}/update-in-progress
  pub fn restart_ui_marker_path(home: &Path) -> PathBuf;
  pub fn update_in_progress(home: &Path) -> Option<String>;  // 读标记 → Some(目标版本)/None(不存在/读失败按 None)
  static CONN_COUNT: AtomicUsize; pub fn connected_clients() -> usize;  // serve 每连接 ±1(accept 后 +1,写循环退出 -1)
  ```
- Produces(daemon):`pub(crate) fn write_atomic(path: &Path, contents: &str) -> Result<()>`(session.rs:同目录 `.tmp` + rename;unix 保 0600);apply 的 Installing 序列 = jobs 快照冲刷(广播一次各 job 终态/现状帧)→ 写 restart-ui(仅当 connected_clients()>0)→ 写 update-in-progress(内容=目标版本,原子)→ 广播 event.update{state:"installing"} → 等 `connected_clients()==0` ≤5s(250ms 轮询,超时不候)→ detached spawn `<已验签 setup 路径> /S`(creation_flags DETACHED_PROCESS|CREATE_NO_WINDOW)→ `Daemon::shutdown` 退出;spawn 与退出间零等待。Daemon::start 开机:marker 版本 == 当前版本 → 清标记 + notify「已更新到 X」;不等 → warn「上次更新未完成(目标 X,当前 Y),已清除标记」+ 清除;restart-ui 标记存在 → 清除 + open_main_window。
- Produces(守卫):三处 spawn 路径(cli/ui 桥 spawn_daemon、uilaunch::open_main_window/open_update_window)入口 `update_in_progress(home).is_some()` → 拒绝:CLI `CliError{code:2, message:"更新进行中,请稍候"}`;UI 桥 warn 不 spawn;daemon 进程本体不设此守卫(开机消费在 start)。

- [ ] **Step 1: 写失败测试** — ①ipc:标记助手写读/缺失;connected_clients 在 serve 挂一个客户端连接期间 ≥1、断开后归零。②update_e2e:ready 态 + 注入 spawn 桩(UpdateConfig 增 `spawner: Option<Arc<dyn Fn(PathBuf) + Send+Sync>>` 测试注入,生产 None→真 Command)→ apply → 标记已写、installing 事件已发、spawner 收到路径、状态 ready→installing;版本相等消费/不等消费两分支;③守卫单测:写 marker 后三路径拒绝(cli 管道测试形态 / uilaunch 纯函数化检查)。
- [ ] **Step 2: 跑测试确认失败** — Run: `cargo test -p mtdss-ipc -p mtdss-daemon -- update marker guard` Expected: FAIL
- [ ] **Step 3: 实现** — 如上;write_atomic 失败 → apply 转 failed(标记未写 = 未进入 Installing,可重试)。
- [ ] **Step 4: 跑测试确认通过 + 回归** — Run: `cargo test -p mtdss-ipc -p mtdss-daemon` Expected: PASS
- [ ] **Step 5: 门禁 + Commit** — `feat(daemon+ipc): install contract — atomic markers, connection count wait, detached spawn, spawn guards, boot consumption`

### Task 13: CLI `updatecheck` / `update [version]`

**Files:**
- Modify: `crates/cli/src/main.rs`(Cmd 增两变体 :43-145;登录豁免集 :171 增两员;run 分派)
- Modify: `crates/cli/src/client.rs`(update_check_cmd/update_cmd 包装)
- Test: `crates/cli/tests/ipc_cli.rs`

**Interfaces:**
- Consumes: Task 11 RPC 面(update.check/state/apply)、Task 12 守卫文案。
- Produces: `Updatecheck`(rpc update.check → 打印「当前 X / 最新 Y」+「有更新可用,运行 mtdss update 安装」或「已是最新」);`Update { version: Option<String> }`(打印目标版本与 notes 首行 → stdin 确认「确认更新? [y/N]」(N/EOF → CliError{code:0,"已取消"},沿 ask 先例)→ rpc update.apply → 500ms 轮询 update.state:failed → 打印错误退出 1;installing/applying → 打印「已开始安装,守护进程退出」退出 0;transport 断(daemon 已退)同按 installing 处理)。空闲门 Err(kind=JOBS_ACTIVE)→ 原样打印 message 退出 1。

- [ ] **Step 1: 写失败测试** — ipc_cli 管道测试:①updatecheck 有新版/已最新两态输出;②update 确认 N 取消 code 0;③apply 回 JOBS_ACTIVE → 输出文案精确 + code 1;④state 流 failed/installing/断连三分支退出码。
- [ ] **Step 2: 跑测试确认失败** — Run: `cargo test -p mtdss-cli --test ipc_cli -- update` Expected: FAIL
- [ ] **Step 3: 实现** — 如上;rpc 超时沿 CALL_TIMEOUT,轮询不设长超时(daemon 退出即终态)。
- [ ] **Step 4: 跑测试确认通过 + 回归** — Run: `cargo test -p mtdss-cli` Expected: PASS
- [ ] **Step 5: 门禁 + Commit** — `feat(cli): mtdss updatecheck / update [version] (confirm flow, idle-gate hard reject exit 1)`

### Task 14: 托盘 — 设置子菜单三档 + 「检查更新…」

**Files:**
- Modify: `crates/daemon/src/tray.rs`(菜单 :159-186、事件循环 :203-242)
- Test: `crates/daemon/tests/tray_logic.rs`

**Interfaces:**
- Consumes: Task 10 state 访问器、Task 11 UpdateService::check/force、Task 12 open_update_window。
- Produces: settings_sub 增 `CheckMenuItem 自动更新`(seed=state.update_auto_check())与 `CheckMenuItem 自动安装`(seed=update_auto_install,`set_enabled(auto_check)` 联动);点击沿 autostart 回滚模式(:222-235):set 成功才 set_checked,失败回读;「自动更新」取消勾选 → 成功后 `set_update_check(false)`(内部连带 auto_install=false)+ 自动安装项 set_checked(false)+set_enabled(false)。主菜单增 `MenuItem 检查更新…` → `state.update.check(force)` → available:notify_one「发现新版本 X」+ `uilaunch::open_update_window()`;已是最新:notify_one「已是最新版本」;Err:notify_one「检查更新失败」。菜单文案中文。

- [ ] **Step 1: 写失败测试** — tray_logic 纯函数化:三档联动决策表(auto_check 开关 × auto_install 使能/勾选 × set 成败 → 期望 UI 态);检查更新三结果分支的通知文案选择器。
- [ ] **Step 2: 跑测试确认失败** — Run: `cargo test -p mtdss-daemon --test tray_logic` Expected: FAIL
- [ ] **Step 3: 实现** — tray.rs 增项与处理器(守护线程内 async 用 state 句柄的 handle 派生,沿现 login/logout 处理形态);open_update_window = spawn ui.exe 带 `--update-page` 参数 + 更新进行中守卫。
- [ ] **Step 4: 跑测试确认通过 + 回归** — Run: `cargo test -p mtdss-daemon` Expected: PASS
- [ ] **Step 5: 门禁 + Commit** — `feat(daemon): tray update settings (auto-check/auto-install coupled) + check-for-updates item`

### Task 15: UI — 更新页/toast/事件桥/单实例路由

**Files:**
- Modify: `ui/src-tauri/src/ipc_bridge.rs`(event_action :206-219 增 event.update→Forward("daemon://update");命令 update_check/update_state/update_apply)
- Modify: `ui/src-tauri/src/lib.rs`(invoke_handler :43-64 增三命令;single-instance 回调 :20-33 增 argv 含 `--update-page` → emit("daemon://open-update");启动时自身 args 同查)
- Create: `ui/src/lib/update.ts` + `ui/src/lib/update.test.ts`
- Create: `ui/src/components/UpdatePage.tsx`
- Modify: `ui/src/lib/daemon.ts`(updateCheck/updateState/updateApply + listenUpdate/listenOpenUpdate)
- Modify: `ui/src/App.tsx`(updatePageOpen 状态 + 挂载 UpdatePage 覆盖层 + available toast)
- Modify: `ui/src/i18n/zh.json`(update.* 命名空间)
- Test: `ui/src/lib/update.test.ts`

**Interfaces:**
- Consumes: Task 11 状态 JSON 形、Task 12/13 守卫、Task 14 托盘拉起。
- Produces: `lib/update.ts` 纯函数面:`updateStateFromEvents(prev, ev)` reducer(幂等、failed 保留 error、installing 后 UI 显示「安装中,窗口将自动关闭」)、`updateTierLabel(state)`、`canInstall(st)`(ready/pending_install 才可点)、进度格式化;UpdatePage 覆盖层(独立于登录态;不进 nav 栈):当前/最新版本、notes、进度条(复用 ProgressBlock 视觉规格)、安装按钮 → updateApply(可选版本输入仅回滚场景展示)、失败重试按钮、关闭;App 监听 event.update → reducer + available 时 pushToast「发现新版本 X(查看)」(toast 点击 open page);listenOpenUpdate → open page;页面挂载时拉一次 update.state。

- [ ] **Step 1: 写失败测试** — update.test.ts:reducer 全迁移表(idle→available→downloading 进度节流展示→ready→installing;failed 带 error;乱序旧帧丢弃)、canInstall 三态、tier 文案映射。
- [ ] **Step 2: 跑测试确认失败** — Run: `cd ui && npx vitest run` Expected: FAIL
- [ ] **Step 3: 实现** — 如上;event.quit 关窗逻辑不动。
- [ ] **Step 4: 跑测试确认通过 + build** — Run: `cd ui && npx vitest run && npm run build && cargo check --manifest-path ui/src-tauri/Cargo.toml` Expected: PASS
- [ ] **Step 5: 门禁 + Commit** — `feat(ui): update page overlay + toasts + event bridge + --update-page routing`

### Task 16: Windows NSIS 打包(三件套 + 生命周期钩子)

**Files:**
- Modify: `ui/src-tauri/tauri.conf.json`(bundle 增 windows 节)
- Create: `ui/src-tauri/windows/hooks.nsh`
- Test: 手动验证步(本任务内跑一次本地打包并核对产物存在;深度行为验证入 smoke-m4 §A)

**Interfaces:**
- Produces(tauri.conf.json bundle 增):
  ```json
  "windows": {
    "webviewInstallMode": { "type": "downloadBootstrapper" },
    "nsis": {
      "installMode": "currentUser",
      "installerHooks": "./windows/hooks.nsh"
    }
  },
  "resources": {
    "../../target/release/mtdss.exe": "mtdss.exe",
    "../../target/release/mtdssd.exe": "mtdssd.exe"
  }
  ```
  (resources map 形态:Windows 下 $RESOURCE=$INSTDIR → 三件套同目录,契合 find_daemon_bin/find_ui_bin 解析序;快捷方式沿 tauri 默认=桌面+开始菜单。)
- Produces(hooks.nsh,NSIS 宏):
  - `NSIS_HOOK_PREINSTALL`:轮询 ≤15s(500ms×30)等 mtdssd.exe 与 ui.exe 退出(nsProcess::FindProcess,tauri NSIS 模板自带该插件);超时 → nsExec taskkill /F /IM 兜底后放行;
  - `NSIS_HOOK_POSTINSTALL`:`Exec '"$INSTDIR\mtdssd.exe"'`(新装/升级同,托盘回归,async 不等);
  - `NSIS_HOOK_PREUNINSTALL`:taskkill mtdssd.exe/ui.exe + `DeleteRegValue HKCU "Software\Microsoft\Windows\CurrentVersion\Run" "MTDSS"`;
  - `NSIS_HOOK_POSTUNINSTALL`:空(保留 MTDSS_HOME 用户数据,业界惯例)。

- [ ] **Step 1: 写 hooks.nsh + tauri.conf** — 如上;注意 `${__FILEDIR__}` 不需要(无相邻文件)。
- [ ] **Step 2: 本地打包验证** — Run: `cargo build --release -p mtdss-cli -p mtdss-daemon && cd ui && npm ci && npx tauri build` Expected: 产出 `ui/src-tauri/target/release/bundle/nsis/*-setup.exe`;安装到本机(手动)核对:三件套同目录、桌面/开始菜单快捷方式、装完托盘出现、卸载后 MTDSS_HOME 仍在且 Run 键已清(结果记录进 Task 19 checklist 底稿)。
- [ ] **Step 3: 门禁 + Commit** — `build(ui): NSIS per-user installer — trio same-dir resources + lifecycle hooks (D11/D14)`

### Task 17: tar 打包 + 部署资产 + dev CA SAN 扩展

**Files:**
- Create: `scripts/package/package-tar.sh`(入参:产物目录与平台标识;tar 内容 = mtdss、mtdssd、README.txt(解压即用/无更新能力说明);输出 `mtdss-<ver>-<x86_64-linux|aarch64-macos>.tar.gz`)
- Create: `scripts/deploy/Caddyfile`、`scripts/deploy/mtdss-signaling.service`、`scripts/deploy/mtdss-stun.service`、`scripts/deploy/README.md`(目录布局、首次部署、备案前演练拓扑、域名就绪切换步骤与回退、更新发布目录规约 /var/www/mtdss/dl/{manifest.json, win/<ver>/})
- Modify: `server/signaling/src/tls.rs`(generate_dev_tls :35-70 增 `sans: &[String]` 参数并入 leaf SAN)、`server/signaling/src/main.rs`(`--tls-san <name>` 可多次,仅与 --dev-gen-tls 组合)
- Test: `server/signaling/tests/tls.rs`

**Interfaces:**
- Produces(Caddyfile 语义):`:443 { tls <cert> <key>; handle /dl/* { uri strip_prefix /dl; root * /var/www/mtdss/dl; file_server }; handle { reverse_proxy 127.0.0.1:8787 } }`;signaling.service = `signaling-server --plain-http 127.0.0.1:8787 --stun-addr <公网IP> --stun-port 3478,3479 /var/lib/mtdss/signaling.db`(After=network.target;stun.service 沿现 VPS 形态);generate_dev_tls(dir, sans):sans 追加为 IpAddress/DnsName SAN(原 localhost/127.0.0.1/::1 保留)。

- [ ] **Step 1: 写失败测试** — tls.rs:generate_dev_tls 带IP SAN(`203.0.113.1`)→ spawn_test_tls_server 变体用该证书 + reqwest 信任 ca.crt 访问 `https://203.0.113.1`?本地无该 IP —— 改测:x509 解析 SAN 列表包含注入项(dev-dep 加 x509-parser?不引 —— 用 rustls 服务端握手 + `ServerName::IpAddress` 客户端连 127.0.0.1 验证 DnsName/Ip 混合不破坏原路径 + 参数单测断言不 panic/文件生成)。简化口径:证书 PEM 内肉眼可断言字段(SAN 扩展文本出现在 crt 内)即收,x509-parser 已是 workspace 依赖可入 dev-deps 时用它精确断言。
- [ ] **Step 2: 跑测试确认失败** — Run: `cargo test -p signaling -- tls_san` Expected: FAIL
- [ ] **Step 3: 实现** — tls.rs 参数化 + main.rs 旗标(重复旗标收集 `--tls-san`);package-tar.sh + 部署资产(README 含演练/切换/回退三节,步骤引用本计划 Task 18/19)。
- [ ] **Step 4: 跑测试确认通过 + 回归** — Run: `cargo test -p signaling` Expected: PASS
- [ ] **Step 5: 门禁 + Commit** — `feat(deploy+signaling): tar packaging, Caddy/systemd assets, dev CA SAN injection`

### Task 18: release workflow(tag v*)

**Files:**
- Create: `.github/workflows/release.yml`
- Test: 推 tag 前用 workflow_dispatch 干跑(见 Step 3);无单测

**Interfaces:**
- Consumes: Task 8 mtdss-sign、Task 16 NSIS、Task 17 tar。
- Produces: `on: push: tags: ['v*']` + `workflow_dispatch`(入参 dl_base,演练用);jobs:
  1. **check**:tag 名 == 根 workspace version == tauri.conf.json == ui/src-tauri/Cargo.toml == ui/package.json(pwsh/bash 读文件比对,不一致 fail);
  2. **build-windows**(needs check):checkout → rust-toolchain + arduino/setup-protoc@v3(沿 ci.yml :24-26)→ `cargo build --release -p mtdss-cli -p mtdss-daemon -p mtdss-sign`(构建期 env `MTDSS_UPDATE_PUBKEY=${{ vars.MTDSS_UPDATE_PUBKEY }}`)→ node 22 + `cd ui && npm ci`(沿 ci.yml npm registry 重写 :79-83)→ `npx tauri build` → `mtdss-sign manifest --version <tag> --url-base <dl_base> --file <setup.exe> --out unsigned.json` → `mtdss-sign sign --key ${{ secrets.MTDSS_SIGNING_KEY }}` → `mtdss-sign verify --pubkey <vars 公钥>` → 产物:setup.exe、manifest.json;
  3. **build-linux / build-macos**(needs check,并行):`cargo build --release -p mtdss-cli -p mtdss-daemon`(macos 加 `--target aarch64-apple-darwin`)→ `scripts/package/package-tar.sh`;
  4. **publish**(needs 全部):VPS 上传(env 判 secrets.MTDSS_DEPLOY_KEY 非空才执行:rsync manifest.json → `/var/www/mtdss/dl/manifest.json` 与 `/dl/win/<ver>/`,与 setup.exe 同目录)→ `softprops/action-gh-release` 附全部产物(setup.exe、两 tar、manifest.json)。
  secrets/vars:MTDSS_SIGNING_KEY(私钥 hex)、MTDSS_DEPLOY_KEY/MTDSS_DEPLOY_HOST/MTDSS_DEPLOY_USER、vars.MTDSS_UPDATE_PUBKEY;演练期缺 ssh key → publish 仅 GH Release 不失败。

- [ ] **Step 1: 写 release.yml** — 如上;step 名中文注释沿 ci.yml 风格。
- [ ] **Step 2: 语法/干跑校验** — Run: `act` 不可用则 gh CLI:`gh workflow list` 确认注册;推 `v1.0.0-rc1` tag 到 fork/本仓由用户裁定(此步在发布演练执行,本任务仅文件落地 + `gh api repos/:owner/:repo/actions/workflows` 确认解析通过)。Expected: workflow 出现且无 schema 错误。
- [ ] **Step 3: 门禁 + Commit** — `ci(release): tag-driven 3-platform release — version check, mtdss-sign manifest, optional VPS upload, GH release`

### Task 19: smoke-m4 手册 + checklist 段 + 发布演练底稿

**Files:**
- Create: `docs/smoke-m4.md`(沿 smoke-m3b.md 结构:0 能力边界 / 1 环境准备 / 2 测试项 / F 记录格式)
- Modify: `docs/release-checklist.md`(增「## M4 手动冒烟」段,沿 M4a 段 :99 形态)
- Test: 纯文档;验证项由用户执行

**Interfaces:**
- Produces(smoke-m4 §2 测试项,全部回填 checklist):A 安装/卸载/快捷方式/升级安装(NSIS,含 per-user 免 UAC、静默 `/S`);B 更新全链(手动检查→确认→下载→安装→重启→版本翻新);C 自动安装静默档(含空闲门生效:活跃传输时不装、终态后装);D 回滚 `mtdss update <旧版>`(版本钉扎);E 空闲门手动档(paused 也拒);F 失败注入(拔网线/坏签名 manifest/半装态模拟:手写 update-in-progress 标记后拉起各入口);G 三档开关行为(关→纯手动;开+装关→提醒;开+装开→静默);H tar 平台解包可用(Linux/macOS,确认无更新入口提示);I 公网真机更新至少一例(VPS+两台真机)。发布演练底稿:①用户跑 `mtdss-sign keygen`,公钥 hex 提交入仓 `keys/update-pub.hex` + 配 GH vars/secrets,私钥离线保管;②VPS 部署 scripts/deploy(自签 CA 走 --tls-san <VPS_IP> 生成,daemon 侧 MTDSS_UPDATE_URL=https://<IP>/dl + MTDSS_UPDATE_CA);③发 v1.0.0 安装;④人工把 1.0.1 产物与签名 manifest 放 VPS → 真机 1.0.0→1.0.1 全链;⑤回滚。域名就绪后切换步骤引用 scripts/deploy/README.md。

- [ ] **Step 1: 写 smoke-m4.md** — 每项含「判定标准」一句 + 环境矩阵(VPS/Windows/Linux/macOS);失败注入节给出具体手法(改 manifest 一字节/断网时机)。
- [ ] **Step 2: checklist 增段** — 「M4 手动冒烟」空表(A–I 行)链接 smoke-m4;M4a 后置项 :110-114 中债①条目标注已清。
- [ ] **Step 3: 终门禁 + Commit** — 全套门禁 + `gh pr checks` 全绿;`docs(smoke): m4 smoke manual + checklist section + release rehearsal runbook`

---

## 执行注意

- 任务顺序即上列序号(债①→⑤ 先行,版本/常量打底,core 签名件 → 服务端 → daemon 更新核心 → 安装契约 → 三个入口面 → 打包/资产/workflow → 手册);Task 11 依赖 6/7/10,Task 12 依赖 11,Task 13-15 依赖 11/12,Task 16 依赖 12(钩子与标记对齐),Task 18 依赖 8/16/17。
- 行号为写计划时快照,执行时以符号搜索为准(符号名优先)。
- 发布演练(真 tag、真 VPS、secrets)由用户择时执行,不在本分支门禁内;workflow_dispatch 干跑可先行。
