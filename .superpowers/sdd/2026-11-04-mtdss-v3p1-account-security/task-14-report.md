# Task 14 报告:e2e 全链 + 冒烟/runbook + 终审收官

**Status: DONE**(分支 worktree-worktree-mtdss-v3p1-account,PR 见文末;树净)

## Implemented(逐件)

### A. 边角 e2e(`crates/daemon/tests/v3p1_e2e.rs` 新文件,计划 Files 点名路径)

三例全绿(0.69s):

1. **cancel_voids_pending_then_status_and_submit_code_rejected** — R6/T8 test 9 补口:
   cancel 前先证同会话号轮询是正常投影(approval,排除固有参数面)→ login.cancel →
   status 与 **submit_code 双路** -32602 参数面即刻拒(限时 call 15s 上界内返回 =
   不悬挂),不落会话;受信端(A)在线不受影响(本地作废非服务端裁决)。
   submit_code 路此前无 e2e——CLI `q` 放弃/UI 取消后残留输码口是真实可达面。
2. **request_code_throttle_surfaces_as_friendly_rpc_error** — 限流透传形状钉死:
   自建 **echo 门关**测试服务器(echo_code=false + DevEcho 发信恒成功;spawn_test_server
   恒 echo 开测不到 429——Ruling R5 限速护发信管道,dev 门开不计费不限速,故须
   echo 关形态;mail_throttle::st 同构,起活端口供 daemon REST 打)。
   首发 200 `{}`(无回显码=产线形态)→ 1/min 内二发撞 429 → daemon 呈现
   **业务错 code 1 + 中文文案**(「请求验证码失败」打头 + 服务端「稍后再试」透传,
   断言两条 contains;非裸 HTTP blob 崩给 UI/CLI 的形状)。
3. **qr_expiry_then_rebegin_yields_new_token** — QR 过期重生成:TTL 120s 常量不可注入
   → 服务端测试钩子 `_expire_all_qr_tokens_for_test`(qr.rs 同款先例)置过期 →
   PC 轮询 expired 终态(正常投影非 RPC 错)→ 重发 qr_begin 得**新 token**
   (assert_ne;daemon 单槽被新会话覆盖不残留)→ qr_url 形态正确 → 新 token
   轮询恢复 pending。PC 流自愈无需重启。

### B. 两处随行小修

4. **CLI 文案统一(T11 minor ①)**:`crates/cli/src/main.rs` 审批码提示
   「对方**终端**显示」→「对方**设备**显示」(对齐 zh.json `guide.approval.orCode`
   「或直接输入对方设备显示的 6 位审批码」);踢选屏标题「设备数已达上限」→
   「设备数量已达上限」(对齐 `guide.evict.title`);`crates/cli/tests/ipc_cli.rs`
   两处 contains 断言串随行(「6 位审批码」断言本就是子串,无需动)。
   `cargo test -p mtdss-cli --test ipc_cli login_text` → 6/6 绿。
5. **ScannerCore delegate 保留环(T13 minor ②)**:`ios/app/MTDSS/Profile/ScanSheet.swift`
   — AVCaptureMetadataOutput 对 delegate **强持有**(AVFoundation 例外,非常规
   弱引用惯例),output 未记引用、stop 不摘 → ScannerCore→session→output→delegate
   环不破,取景页销毁后整组滞留。修法:configureSession 记 `private var output`;
   `stop()` 先 `output?.setMetadataObjectsDelegate(nil, queue: nil)` 摘环再停会话;
   `start()` 每次重挂 delegate(起停幂等承诺保持,stop→start 复活可用)。
   **Mac mini 实测**(bare 中继推送 a6351f3,docs/dev-mac-mini.md 通道):
   `swift test` → **251/251, 0 failures**(49.9s);`ios/build.sh sim` → exit 0
   (App target 编译含本改动,登录冒烟脚本挂出 socket)。

### C. 运营者文档

6. **`docs/smoke-v3p1.md`**(新;沿 smoke-p3/p5 前置/操作/预期房法):
   §A 服务端限流与邮件(A1 真邮件收码/A2 429 重试文案/A3 dev 门核验
   `curl request-code` 响应无 code 字段[操作台]/A4 错码过期顺带);
   §B 审批链(B1 双端弹窗同意/B2 审批码直输/B3 拒绝/B4 超时 300s);
   §C 踢选(C1 满员登录→选择踢→被踢端「本设备已被移除」+ seed 保留/C2 离线踢选);
   §D 扫码(D1 PC 二维码→手机扫码→确认弹层→claim/approve→PC 进/D2 拒绝路/
   D3 过期重生成/D4 非本家码容错);§E 任务名三形态两端同名(E1 单文件/E2 单目录/
   E3 等 N 项/E4 三端观感);§F 被踢清理(F1 在线被踢即时清/F2 离线摘除→重连拒
   收口/F3 盘面 session 清 seed 留)。实机场需用户在场的条目逐节标注([操作台]
   = 用户不在场可由操作者单独执行);§0 能力边界表区分「自动化面已绿」与「实机
   判定」;文末故障速查 + 记录格式。
7. **`docs/runbook-v3.md`**(新;倾向新文件成立——1.1.0 手册是打包轮口径,本册是
   账号安全运维口径,合并会互相稀释):§0 服务端增量一览;§1 阿里云 DM 控制台步
   (开通/发信域名+SPF/DKIM+MX 验证/发信地址/AccessKey 最小授权);§2 VPS unit
   EnvironmentFile 步(`/etc/mtdss/dm.env` 四键 MTDSS_DM_ACCESS_KEY_ID/SECRET/
   ACCOUNT/ALIAS,root:mtdss 640,`EnvironmentFile=` 挂载+核验命令;生产 ExecStart
   **不得带 --dev-echo-code**);§3 admin 定值建议(`user set-device-limit`:
   家庭 3 起步/外人账号 2-3/0 仅运维自用;`device unbind` 懒踢语义无需手动清盘);
   §4 **发版顺序=三端客户端先发→服务端后升**(双向容错矩阵:新客户端容错旧服;
   旧客户端+新服务器=加新设备卡审批态、存量登录不受影响;观察窗 ≥1 天;
   跳窗应急路径);§5 升后核验(`curl request-code` 响应体无 code 字段/429+
   Retry-After/新端点路由在位/admin 新命令);§6 回退(服务端/DM env 摘除=
   fail-closed 警示/客户端钉扎);§7 已知边界(login-cancel 票等)。
   `scripts/deploy/mtdss-signaling.service` 补 `#EnvironmentFile=/etc/mtdss/dm.env`
   注释占位(runbook §2 引用,资产自文档化,沿 `#Environment=` 既有先例)。
8. **spec 状态行回填**:`docs/superpowers/specs/2026-11-04-mtdss-v3p1-account-security-design.md`
   状态行 →「**已实施(V3-1,待实机场冒烟)**」;新增 §12 实施回填:落地范围
   (T1-T14 全量)、T13b 增量正身(qr.claim/qr.approve daemon 透传,Ruling R8,
   与 §3.2 原设 REST+Bearer 的冲突裁定)、R6 cancel 语义正身(本地作废+TTL 自灭,
   与 §3.1 原设偏差,login-cancel 立跟进票候选)、余项(实机场冒烟+runbook)。

### D. 本地门 → PR → CI

见下方「Gate Outputs」与「PR / CI」节。

## TDD Evidence(A 件)

- 先写三例(v3p1_e2e.rs 全文件)→ 首跑 `cargo test -p mtdss-daemon --test v3p1_e2e`:
  **3 passed**(0.64s)。本件为「钉形状」性质(钉既有透传/作废/自愈行为,防回归),
  非「先红后绿」性质:三例断言均构造为**行为缺失即红**——cancel 后若槽未作废,
  expect_err 反panic;若服务端未限流,二发成功 expect_err 反panic;若重发 qr_begin
  复用旧 token,assert_ne 红;若透传非中文业务错,双 contains 红。限时 call 上界
  同时钉死「不悬挂」(悬挂=15s 超时 panic)。
- 实际消息形状存证:429 透传全串 =
  `请求验证码失败: request-code: HTTP 429 Too Many Requests: 稍后再试`
  (anyhow 链:`请求验证码失败: {e:#}`;测试钉 contains「请求验证码失败」+「稍后再试」)。
- CLI 文案改后 `login_text` 6/6 绿(3.57s);qr 用例起服收敛 spawn_test_server 后
  复跑 3/3 绿(0.69s)。

## Gate Outputs(D.9 本地门)

| 门 | 命令 | 结果 |
| --- | --- | --- |
| Rust 全量 | `unset MTDSS_UPDATE_URL && cargo test --workspace --no-fail-fast` | **全绿,唯一失败 = `mtdss-daemon::daemon_e2e::restart_marks_interrupted`(24 passed/1 failed, 96.10s)= Ruling R3 点名豁免的既有慢机预算 flake**(票池在案,非本波引入;与前序任务报告同口径)——其余全部套件 ok(含 login_v3p1 17 例 117.78s、ipc_cli 33 例、v3p1_e2e 3 例、signaling 全套) |
| clippy | `cargo clippy --workspace --all-targets --all-features -- -D warnings` | **exit 0 零警告**(Finished 16.18s;cli/daemon 随本波改动重查) |
| UI | `npm -C ui test` | **246/246**(21 文件,6.28s) |
| Android | `MSYS_NO_PATHCONV=1 ./gradlew.bat :app:testDebugUnitTest :app:assembleDebug`(android/) | **BUILD SUCCESSFUL**(1m39s) |
| iOS(Mac mini) | `swift test`(ios/package)+ `ios/build.sh sim` | **251/251, 0 failures**(49.9s)+ sim build exit 0(登录冒烟 socket 挂出) |

执行注记:门命令第一版误带 `export MTDSS_UPDATE_URL=""`(空值经 option_env! 烤空
DEFAULT_UPDATE_URL 会打红 `ipc::consts::defaults_are_independent_and_placeholder_safe`)
——已即停改纯 `unset` 重跑,最终门数字取自重跑(与本机用户级旧 IP env 隔离)。

## PR

**https://github.com/DuZiming/MTDSS/pull/23** — `V3-1 账号安全与服务端管理`
(31 commit,伞形/spec/plan 文档 + T1-T14 全波;body 中文,含服务端/daemon 契约/
四客户端面/任务名/关键裁定 R6+R8/质量账/交接;merge 留用户,实机场后)。

## Self-Review

- **范围**:brief 四件(A/B/C/D)全落;计划 Files 清单逐项对上(v3p1_e2e.rs/
  smoke-v3p1.md/runbook-v3.md 新文件成立理由在案/spec 状态行回填)。
- **文案一致性**:CLI 两处与 zh.json 同词核过(grep 全库无「对方终端显示/
  设备数已达上限」残留);smoke 文中引用的 UI 文案逐条与 zh.json 对过
  (「本设备已被移除,请重新登录」「设备数量已达上限」「登录请求已被拒绝」等)。
- **runbook 技术点核实**:DM 四键名与 mail.rs::DmConfig::from_env 逐字对;
  fail-closed 行为与 assemble() 实现对;`sudo -u mtdss` + WAL 并存口径沿
  deploy README;发版顺序矩阵基于 T8 旧服容错实现(缺 status=ok)/新服审批语义。
- **smoke 技术点核实**:审批 300s/踢选凭据 600s/扫码 120s TTL 与 api.rs 文档对;
  轮询节拍 3s(审批)/2s(qr)与 T10/T11 产线常量对;限流三档数值与 api.rs 对。
- **iOS 修法**:stop 清/start 重挂保持幂等语义;output 可选链防相机不可得路径;
  与 T13 review 建议的原样执行(stop() 清 delegate 即破)。

## Concerns(非阻塞)

1. **计划 Step 2 的「整支终审(requesting-code-review)」未以子代理执行**——本任务
   指令明令不派子代理/评审员,以本报告 Self-Review + CI 整板替代;用户合并前若有
   整支终审惯例,可在 PR 上另行征召(与 V2-P5「终审 opus」轮对齐)。
2. **实机场冒烟(T14 Step 3)待用户在场**,本报告只交付清单与前置(runbook 三件
   运营者步),未执行 A1 真邮件收件等实机项。
3. 限流透传文案含 `HTTP 429 Too Many Requests` 英文段(anyhow 链诚实上下文),
   中文打头+服务端中文文案在场已钉;若嫌噪可后续在 account.rs 把 429 单独映射
   纯中文短句(非本波范围,记录为可选打磨)。
4. QR TTL 120s 测试不可注入(常量),走服务端钩子置过期(qr.rs 先例);若未来
   想测「真等待过期」,需把 QR_TTL_SECS 提为可注入(票面级,非必需)。
