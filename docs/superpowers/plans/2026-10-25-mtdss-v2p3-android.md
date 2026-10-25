# MTDSS v2-P3 Android 客户端 实施计划

> **For agentic workers:** REQUIRED SUB-SKILL: Use superpowers:subagent-driven-development (recommended) or superpowers:executing-plans to implement this plan task-by-task. Steps use checkbox (`- [ ]`) syntax for tracking.

**Goal:** 交付安卓原生客户端(Kotlin + Compose):登录、设备列表、浏览对端、收发、任务管理与断点续传;引擎 mtdssd 子进程零重写;存储走商店合规的 fd 桥。

**Architecture:** mtdssd 以 `libmtdssd.so` 打进 APK 由 App 拉起,Kotlin 作为 crates/ipc 本地套接字上的又一 JSON-RPC 客户端;上传/接收的文件 I/O 经 fd 桥(`{home}/fds.sock`,SCM_RIGHTS)由平台层供给;无常驻、无前台服务、不自动续传。

**Tech Stack:** Rust workspace(既有,cfg 门内适配 android 目标)+ Kotlin/Compose + Gradle + cargo-ndk + android.net.LocalSocket(fd 传递)。

**Spec:** [docs/superpowers/specs/2026-10-25-mtdss-v2p3-android-design.md](../specs/2026-10-25-mtdss-v2p3-android-design.md)(含评审六裁定,已回写)。

## Global Constraints

- minSdk 29 / targetSdk 35(spec §3.7 评审裁定⑤);不申请任何存储运行时权限(29+ 无传统分支),manifest 权限只有 `INTERNET`。
- 无前台服务、无通知、无常驻;不自动续传;cut 在安卓 UI 不暴露、契约恒 `false`(spec §5.2 裁定④)。
- 桌面回归零:既有全量测试面(cargo test --workspace 双平台、nat-matrix、ui job)不得因本计划任何任务变红;既有 op 语义、NDJSON 线格式、8 MiB 帧上限、RpcError 形状不动。
- 引擎改动边界:manifest/transfer 的源参数化、intake 的 DestIo 参数化、android cfg 门(PDEATHSIG 等)——除此之外信令/打洞/中继/QUIC/任务存储零改动。
- 安卓 download 的 `dir` 固定哨兵 `"inbox"`;桌面侧收到 `"inbox"` 必须照常 `INVALID_PATH`(防串台)。
- 本机 bash 门禁先 `unset MTDSS_UPDATE_URL`(CI 口径);cargo 走 rsproxy;gh CLI bash 需 `export PATH="/c/Program Files/GitHub CLI:$PATH"`。
- 分支:`worktree-mtdss-v2p3-android`(EnterWorktree);每任务一提交起,测试与实现同commit。

## Review Focus

1. **fd 生命周期泄漏**(大目录上传逐文件开/关,泄漏→句柄耗尽):T4 e2e 多文件逐个 request_source 后断言引擎 fd 计数回落;T9/T11 沿用。
2. **Kotlin 客户端慢消费者断连重连**(8 MiB/64 帧语义在移动网络下频发):T8 单测「写队列满断连→自动重连→补拉 transfer.list」。
3. **授权中途被撤**(用户在系统设置 revoke URI 权限→request_source 打开失败):T4 e2e 伪 provider 第二次拒开→任务 failed「源不可用」;T10 实机呈现。
4. **MediaStore pending 项被系统清理**(resume 重开撞 NOT_FOUND→须整文件重收不崩):T6 e2e 分支「resume 无 pending 项→FULL」。
5. **哨兵串台**(`"inbox"` 出现在桌面客户端请求):T6 校验测试「桌面 host dispatch download dir="inbox" → INVALID_PATH」。

---

### Task 1: 工具链落地 + mtdssd 交叉编译打通 + android cfg 门

**Files:**
- Modify: `crates/daemon/src/main.rs`(android cfg:PDEATHSIG、tracing stderr 直出)
- Modify: `crates/daemon/src/lib.rs` 及 tray.rs/uilaunch.rs/update.rs/notify.rs 引用点(android cfg 门;现状多为 cfg(windows),补 `target_os = "android"` 分支)
- Modify: `crates/daemon/Cargo.toml`(`[target.'cfg(target_os = "android")'.dependencies]`:libc)

**Interfaces:**
- Produces: `cargo ndk -t arm64-v8a -p mtdss-daemon -- build` 产出可运行于安卓的 mtdssd;后续所有任务的编译前提。
- Consumes: 既有 `mtdss_ipc::mtdss_home()`(env `MTDSS_HOME` 已支持,零改)。

- [ ] **Step 1: 装环境(用户参与)**:Android Studio(SDK Platform 35 + build-tools + NDK)、`rustup target add aarch64-linux-android`、`cargo install cargo-ndk`。验证:`cargo ndk --version`、`sdkmanager --list_installed` 含 ndk。
- [ ] **Step 2: 先跑基线**:host `cargo test --workspace` + `cargo clippy --all-targets --all-features -D warnings` 全绿(记录为回归基线)。
- [ ] **Step 3: cfg 门**:grep daemon 内 tray/uilaunch/update/notify/autostart 的引用点,android 目标下全部编译剔除(多数已是 cfg(windows),补 android 分支;Linux 桌面行为不变)。`main.rs` 启动序 android 下加 `prctl(PR_SET_PDEATHSIG, SIGKILL)`(libc,unsafe 一行,注释引 spec §4.1 裁定⑥);`init_tracing` android 下恒 stderr(logcat 直读)。
- [ ] **Step 4: 交叉编译验证**:`cargo ndk -t arm64-v8a -p mtdss-daemon -- build` 成功;host 门禁重跑全绿。
- [ ] **Step 5: Commit** `feat(android): mtdssd 交叉编译打通 + cfg 门 + PDEATHSIG`

### Task 2: fdpass 通道(SCM_RIGHTS 收发)

**Files:**
- Create: `crates/ipc/src/fdpass.rs`(server + client,双端都在本 crate,供测试与协议同源)
- Modify: `crates/ipc/src/lib.rs`(mod fdpass)、`crates/ipc/Cargo.toml`(`[target.'cfg(unix)'.dependencies]` libc;模块整体 cfg(unix))
- Test: `crates/ipc/tests/fdpass_test.rs`

**Interfaces:**
- Produces:`pub async fn serve_fdpass(home: &Path, handler: Arc<dyn FdMsgHandler>) -> Result<()>`,监听 `{home}/fds.sock`(0600,bind 前清残留,与 ipc::serve 同款清理);
  ```rust
  pub struct FdMsg { pub kind: u8, pub payload: serde_json::Value, pub fd: Option<OwnedFd> }
  pub trait FdMsgHandler: Send + Sync {
      fn on_msg(&self, msg: FdMsg, reply: FdReply) -> Pin<Box<dyn Future<Output=()> + Send>>;
  }
  pub struct FdPassClient { … } // 测试/mock 用:connect(home)、send_msg(kind, payload, fd) -> FdMsg(应答)
  ```
- 线格式(与 Kotlin 共享,注释钉死):`kind u8 | len u32 BE | JSON`;fd 经 SCM_RIGHTS 随本消息附带(0/1 个);应答同格式回流。kind 常量:`0x01 source_fd`(payload `{request_id}` + 1 fd)、`0x02 dest_fd`(payload `{request_id}` + 1 fd)、`0x81 ack {request_id, ok, error?}`、`0x03 dest_commit {request_id, done}`(daemon→Kotlin,见 T7)。
- 实现:裸 `tokio::net::UnixListener/UnixStream` + `libc::sendmsg/recvmsg`(cmsg SCM_RIGHTS;libc 依赖仅 unix,Windows 不编译)。单帧 payload 上限 64 KiB(元数据足够)。

- [ ] **Step 1: 失败测试**:双 LocalSocket… 用 FdPassClient 连 serve_fdpass,handler 收到 fd 后向 fd 写问候字节,客户端读回断言内容;再测无 fd 消息、超限 payload 断连。
- [ ] **Step 2: 跑红**(编译失败即红)。
- [ ] **Step 3: 实现 fdpass.rs**(sendmsg/recvmsg 封装、cmsg 解析、逐连接 actor;注释钉线格式)。
- [ ] **Step 4: 跑绿** + host clippy/test 全量重跑。
- [ ] **Step 5: Commit** `feat(ipc): fdpass 通道(SCM_RIGHTS)`

### Task 3: 契约 golden 防漂移(Rust 侧)

**Files:**
- Create: `crates/daemon/tests/contract_golden.rs`
- Create: `crates/ipc/contract/*.json`(生成后提交;目录即单一真源,Kotlin T8 读同一路径)

**Interfaces:**
- Produces: golden 集——每个可移植 op 一组 `{op}.req.json` / `{op}.resp.json`,事件 `{event}.json`;平台分歧面 `{op}.resp.android.json` 变体。fixture:固定 DaemonState(未登录/已登录两态各一套必要 op)+ 固定时间源。
- Produces: regenerate 途径 = 测试 `REGEN=1 cargo test contract_golden` 重写文件(注释写明)。

- [ ] **Step 1: 写测试骨架**:逐 op 构造 fixture state → `ApiDispatcher::dispatch` → 序列化请求/响应帧 → 与 golden 比对(缺文件先写)。覆盖 hello/status/devices/login.request_code/login.verify/login.logout/list/stat/transfer.start(四形态各一,含 bridge 形态占位——T4 落地后补)/transfer.list/pause/resume/cancel/wait/reinitiate/clear + 事件 event.login/event.devices/event.online/event.transfer/event.notify 样例帧。
- [ ] **Step 2: 跑红→生成→跑绿**;`REGEN=1` 幂等(重生成无 diff)。
- [ ] **Step 3: 全量回归 + Commit** `test(contract): IPC 契约 golden 锚`

### Task 4: 源侧泛化 SourceFile + bridge upload 形态 + fs.request_source

**Files:**
- Modify: `crates/core/src/protocol/manifest.rs`(`build_manifest` 条目参数 `&[(PathBuf, String)]` → `&[(SourceFile, String)]`;`pub enum SourceFile { Path(PathBuf), Fd(std::os::fd::OwnedFd) }`,Fd 分支经 `File::from(OwnedFd)` 同读)
- Modify: `crates/core/src/transport/transfer.rs`(`send_file` 的 `path: &Path` → `source: &SourceFile`,打开点单改)
- Modify: `crates/daemon/src/api.rs`(transfer.start upload 臂增判别字段 `source:"bridge"` + `items:[{ref,rel,size,mtime}]`,无 files/dir 语义变化:dir 仍必填、policy/cut 照旧)
- Modify: `crates/daemon/src/jobs.rs`(`UploadJob` 增 `source: UploadSource { Paths(Vec<PathBuf>), Bridge(Vec<BridgeItem>) }`;manifest 构建与 send 逐文件经 `SourceResolver`:`request_source` 事件 `{"request_id":u64,"ref":str}` 广播 + 10s oneshot 等待;taskstore `src_paths` 序列化 bridge 形态为 `bridge://{ref}` 串)
- Modify: `crates/daemon/src/flow.rs`(`run_send_with` 文件表改 `&[SourceFile]`;`collect_items` 路径形态不动)
- Test: `crates/daemon/tests/upload_bridge_e2e.rs`

**Interfaces:**
- Consumes: T2 `FdMsgHandler` + `0x01 source_fd`;T3 golden(regenerate 补 bridge 形态)。
- Produces: `BridgeItem { pub r#ref: String, pub rel: String, pub size: u64, pub mtime: i64 }`;`fs.request_source` 事件与 `SourceResolver`(jobs 内部,供 T6 复用模式)。

- [ ] **Step 1: 失败 e2e**:host 双 daemon + 伪 App 客户端(IpcClient 订阅 `fs.request_source` → 打开临时源文件 fd → 经 FdPassClient 回 `0x01`)→ `transfer.start {kind:upload, source:"bridge", items, to, dir}` → 接收端断言内容逐字节;断言多文件逐个 request、10s 超时路径(不真等 10s,超时常量注入测试值)→ 任务 failed「源不可用」。
- [ ] **Step 2: 跑红**。
- [ ] **Step 3: 实现**(顺序:SourceFile enum + core 两点签名 → api 判别 → jobs SourceResolver → taskstore 序列化;桌面路径形态调用方一行适配)。
- [ ] **Step 4: 跑绿 + 桌面全量 upload e2e 回归 + REGEN golden 补 bridge 形态**。
- [ ] **Step 5: Commit** `feat(bridge): 上行 SourceFile 泛化 + request_source 协议`

### Task 5: 接收侧 DestIo 抽象(纯重构,桌面回归零)

**Files:**
- Modify: `crates/core/src/transport/intake.rs`(`pub trait DestIo: Send + Sync { records_dir(); final_state(rel)->Option<u64>; open_writer(entry,resume)->(tokio::fs::File,); commit(entry) }`——签名以实现落定为准,但语义钉死;`FsDestIo { recv_dir }` 承接今行为:`.mtdss` 记账目录 = recv_dir 下、final 查在位文件、commit = 现 finalize 的 rename;`reconcile`/`PartWriter`/`cleanup_rels` 改吃 `&dyn DestIo`)
- Modify: `crates/core/src/transport/transfer.rs` + `crates/daemon/src/flow.rs`(responder 侧 out_dir 传参点改传 DestIo;桌面接线 FsDestIo)
- Test: 既有 intake/reconcile/e2e 全量(不新增面,本任务零行为变化)

- [ ] **Step 1: 重构 + 全量回归跑绿**(先跑既有测试确认重构前后一致;无新测试,证明物 = 全量绿 + diff 审查)。
- [ ] **Step 2: clippy 双平台 + nat-matrix + ui job(本地口径)+ Commit** `refactor(intake): DestIo 抽象,桌面行为逐字节不变`

### Task 6: BridgeDestIo + fs.request_dest + 收件箱 e2e + 哨兵

**Files:**
- Create: `crates/daemon/src/bridge_dest.rs`(`BridgeDestIo:records_dir = {home}/inbox-records;final_state/commit 经 provider 请求;cfg 无关——host 测试可跑`)
- Modify: `crates/daemon/src/jobs.rs`/`flow.rs`(download 建链处:dir == `"inbox"`(常量 `INBOX_SENTINEL`,放 intake.rs)→ BridgeDestIo;fs.request_dest 事件 `{"request_id","rel","size","resume"}` + fdpass 0x02 应答 + 10s 超时;完成发 0x03 dest_commit)
- Modify: `crates/daemon/src/api.rs`(status.recv_dir android 报 `"inbox"`、set_recv_dir android 业务错误;cfg 门)
- Test: `crates/daemon/tests/inbox_bridge_e2e.rs`

**Interfaces:**
- Consumes: T2 fdpass;T5 DestIo;T3 golden(补 android 变体)。
- Produces: `INBOX_SENTINEL = "inbox"`;`fs.request_dest` 事件协议;`dest_commit`;BridgeDestIo 的 records 布局 `{home}/inbox-records/<sha256(rel)>.json`(与 FsDestIO 的 `.mtdss` 键空间隔离)。

- [ ] **Step 1: 失败 e2e 四支**:①全新收(伪 provider 应 fd=临时文件)→ 内容+记录+dest_commit 断言;②中断续传(半程断 fdpass 连接,重发,provider resume=true 应答重开 fd,bitmap 续传);③resume 撞 NOT_FOUND(provider 答 ok:false)→ FULL 重收;④request_dest 10s 超时(常量注入)→ 任务 failed。桌面哨兵测试:桌面 host `transfer.start {kind:download, dir:"inbox"}` → INVALID_PATH。
- [ ] **Step 2: 跑红**。
- [ ] **Step 3: 实现 BridgeDestIo + 接线 + cfg 门**(注意 records 原子写沿 settings.json 模式;P2 reinitiate 对 inbox 语义照常走 —— rel 集来自 job_query 回填,不变)。
- [ ] **Step 4: 跑绿 + REGEN(android 变体)+ 全量回归 + Commit** `feat(bridge): 收件箱 DestIo + request_dest 协议`

### Task 7: Android 工程空壳 + 引擎拉起 + hello 走通(垂直骨架)

**Files:**
- Create: `android/`(独立 Gradle 工程:`settings.gradle.kts`、`app/`、Gradle wrapper;不入 cargo workspace)——`MtdssApplication.kt`、`engine/MtdssEngine.kt`、`MainActivity.kt`(临时调试屏)、`ipc/`(占位)
- Create: `android/app/build.gradle.kts` 任务 `cargoDaemonDebug`(Exec:cargo ndk 编 mtdssd → 拷 `libmtdssd.so` 入 jniLibs)

**Interfaces:**
- Produces: `MtdssEngine`:`start(context)`(ProcessBuilder 启 nativeLibraryDir/libmtdssd.so,env `MTDSS_HOME={filesDir}/mtdss`,stderr→logcat 转发线程,死亡→指数退避重启)、`stop()`;socket 路径 = `{filesDir}/mtdss/daemon.sock`。
- Produces: Gradle 属性注入面(后续任务用):`MTDSS_DL_BASE` 等。

- [ ] **Step 1: 工程骨架编译过**(Compose BOM 当前稳定版、Kotlin 2.x;minSdk 29/target 35;`INTERNET` 唯一权限)。
- [ ] **Step 2: cargoDaemonDebug 任务产出 APK 内含 libmtdssd.so**(unzip 验证)。
- [ ] **Step 3: 实机 MIX 3 装通**:adb install → 调试屏显示 `hello` 响应(version/logged_in);logcat 见 mtdssd 启动日志;App 退出后 logcat 见引擎随亡(PDEATHSIG 生效)+ 对端(PC)设备列表见 MIX 3 离线。
- [ ] **Step 4: Commit** `feat(android): 工程空壳 + 引擎拉起 + hello 走通`

### Task 8: Kotlin IPC 客户端 + golden 契约测试

**Files:**
- Create: `android/app/src/main/java/…/ipc/IpcClient.kt`(LocalSocket FILESYSTEM 命名空间连 daemon.sock;NDJSON 行帧;id 路由挂起 call;事件 SharedFlow;断连自愈重连 + 重连后补 `transfer.list`)
- Create: `android/app/src/main/java/…/ipc/Contract.kt`(op 常量 + 数据类;字段与 crates/ipc/contract 对齐)
- Test: `android/app/src/test/…/IpcClientTest.kt`、`ContractGoldenTest.kt`

**Interfaces:**
- Consumes: T3 golden 目录(Gradle sourceSets 挂 `../../crates/ipc/contract` 为测试资源——单一真源)。
- Produces: `suspend fun <T> call(op: Op, params: JsonObject): Result<T>`;`val events: SharedFlow<EventFrame>`;`RpcException(code, message, kind)`。

- [ ] **Step 1: 失败单测(JVM)**:伪 server(java.net.UnixSocket?JVM 无本地套接字——接口抽象 `SocketFactory`,测试注入内存管道)逐帧喂 golden `.req.json` 断言客户端字节级请求;喂 `.resp.json`/`.json` 事件断言解析;慢消费者断连→重连补拉。
- [ ] **Step 2: 跑红 → 实现 → 跑绿**(gradle testDebugUnitTest)。
- [ ] **Step 3: ContractGoldenTest:遍历 golden 目录全量比对 Kotlin 数据类序列化**。
- [ ] **Step 4: 实机 MIX 3 hello 复验(换真 LocalSocket)+ Commit** `feat(android): IPC 客户端 + 契约 golden 双端锁定`

### Task 9: 登录 + 设备列表 + 浏览对端 + 下载收件箱

**Files:**
- Create: `android/app/src/main/java/…/ui/{LoginScreen,DevicesScreen,BrowseScreen}.kt`、`InboxProvider.kt`(MediaStore:`fs.request_dest` 应答:insert IS_PENDING=1 到 Downloads/MTDSS + relative_path、openFileDescriptor"w"回 fd;resume 重开;`0x03 dest_commit` → IS_PENDING=0 / 失败清项)、`fdpass/FdPassChannel.kt`(LocalSocket setFileDescriptorsForSend/getAncillaryFileDescriptors,线格式同 T2 注释)
- Create: `android/app/src/main/java/…/AppNav.kt`(导航图)

**Interfaces:**
- Consumes: T8 IpcClient;T6 fs.request_dest 协议;T2 线格式。
- Produces: `InboxProvider`(T10/T11 复用);导航路由表。

- [ ] **Step 1: 登录链实机**:request_code → 验证码 → logged_in,设备名入列(MIX3 与 PC 互见)。
- [ ] **Step 2: 浏览对端实机**:list/stat 经 daemon ctrl 打通 PC 目录树(分页/截断标志照 golden)。
- [ ] **Step 3: 下载收件箱实机**:PC 上选文件发起 download(dir="inbox")→ MIX3 `Downloads/MTDSS/` 落盘、系统文件管理器可见、IS_PENDING 揭开。
- [ ] **Step 4: Commit** `feat(android): 登录/设备/浏览/收件箱下载主链`

### Task 10: 发送链:SAF 选择器 + 授权根 + 分享入口 + request_source 应答

**Files:**
- Create: `android/app/src/main/java/…/send/{PickerBridge.kt,RootsStore.kt,SourceProvider.kt,ShareActivity.kt}`(PickerBridge:OPEN_DOCUMENT 多选 + takePersistableUriPermission + ContentResolver 元数据;RootsStore:OPEN_DOCUMENT_TREE 持久授权根列表 DataStore;SourceProvider:`fs.request_source` 应答——按 ref 解析持久 URI 或树根+rel 重建 DocumentsContract URI,打开回 fd,拒开回 ok:false)
- Modify: BrowseScreen/DevicesScreen 增发送入口(选目标设备 → 传 items + dir)

**Interfaces:**
- Consumes: T4 fs.request_source;T8 事件流;T2 0x01。
- Produces: 发送统一入口 `startUpload(items: List<BridgeItemMeta>, to: String, dir: String, policy)`。

- [ ] **Step 1: 文件选择器发送实机**:MIX3 选本机文件 → PC 收到(内容逐字节);本机树 UI(收件箱 + 授权根列表/添加/移除)随 RootsStore 落地;大目录(授权根)发送,断言逐文件 request_source(PC 侧 daemon 日志)。
- [ ] **Step 2: 分享入口实机**:系统分享 → MTDSS → 选设备 → PC 收到;文档内标注「分享件中断后重发=源已不存在」的实测行为与 UI 文案。
- [ ] **Step 3: 授权中途撤销路径**:系统设置 revoke 后重发 → 任务 failed「源不可用」呈现在任务列表。
- [ ] **Step 4: Commit** `feat(android): 发送链 SAF/授权根/分享 + request_source 应答`

### Task 11: 任务列表 UI + 熄屏中断续传实机验证

**Files:**
- Create: `android/app/src/main/java/…/ui/TasksScreen.kt`(transfer.list/events 订阅;三类型徽章 upload/download/remote_copy 中安卓只见前两类+会话级行;状态/进度;操作:暂停/恢复/取消/清除/重新发起——仅 failed/interrupted 行亮)
- Modify: MainActivity(活任务存在时 `FLAG_KEEP_SCREEN_ON`,随 tasks 快照驱动)

- [ ] **Step 1: 列表实机**:PC 发 MIX3、MIX3 发 PC 各一任务,列表行/进度/操作全对 golden 字段。
- [ ] **Step 2: 熄屏中断→手动续传(headline 验证)**:大文件(≥1GB)传输中熄屏 → 任务中断态(屏幕亮后见)→ 重新发起 → 从 bitmap 续传(PC 侧 daemon 日志验证跳过已收块)→ done。
- [ ] **Step 3: 清除语义实机**:发送任务清除 → PC 侧残留 `.mtdss` 清净(在线回执);分享件任务重发 → 「源已不存在」路径文案。
- [ ] **Step 4: Commit** `feat(android): 任务列表 + 断点续传实机验收`

### Task 12: 设置页 + 检查更新 + CI android job + 打包签名 + 冒烟文档

**Files:**
- Create: `android/app/src/main/java/…/ui/SettingsScreen.kt`、`update/UpdateCheck.kt`(Gradle 注入 MTDSS_DL_BASE;拉 manifest + ed25519 验签 + sha256;HttpURLConnection 下载带进度;FileProvider + ACTION_INSTALL_PACKAGE)
- Create: `android/app/src/main/res/xml/file_paths.xml`、debug/release 签名配置(release keystore 用户离线保管,CI 走 secret)
- Modify: `.github/workflows/ci.yml`(android job:setup-java 17 + android-actions/setup-android + rust target + cargo ndk build + gradle testDebugUnitTest + assembleDebug;门禁串联既有四 job)
- Create: `scripts/android/README.md`(本地构建/签名/发布步骤)、`docs/smoke-p3.md`(spec §8 checklist 展开)
- Modify: `docs/superpowers/specs/2026-10-25-mtdss-v2p3-android-design.md` 状态行(收官回填)

- [ ] **Step 1: 检查更新全链实机**:测试 manifest(升版本)→ 提示 → 下载进度 → 拉起安装器 → 装新版本起。
- [ ] **Step 2: CI android job 绿**(PR 全 job 串联绿)。
- [ ] **Step 3: release APK 签名 + 16KB 对齐检查(zipalign -c -P 16)+ 直发物放 smoke/dl 路径演练**。
- [ ] **Step 4: docs/smoke-p3.md 按 spec §8 逐项勾 + Commit + 整支终审(requesting-code-review → 修复波 → 复审)→ PR**。

---

## 执行注

- 顺序刚性:T1→T6 串行(Rust 面互为前提);T7 起可与 T3-T6 并行的只有 T7 本身(垂直骨架先立);T8→T12 串行。
- 实机步骤(MIX 3)标注「实机」处需用户在场协作(装包/操作),子代理执行到该步时暂停征召用户。
- 每任务收尾跑门禁:本机 Windows `cargo test --workspace && cargo clippy --all-targets -D warnings` + `gradle testDebugUnitTest`(T7 起);WSL Linux 面在 T5/T6(引擎重构)两处加跑;nat-matrix/ui job 在 T5 前后各一次。
