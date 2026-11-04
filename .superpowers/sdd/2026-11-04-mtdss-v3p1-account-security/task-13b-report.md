# Task 13b 报告:扫码授权闭环透传(controller Ruling R8 补件,V3-1)

**Status: DONE**
分支 `worktree-worktree-mtdss-v3p1-account`,2 commits(35efad4 + c471da7),五件套齐:daemon 透传 op → golden REGEN → Kotlin 契约绑定 → Swift repo/模型/接线 → daemon e2e。门禁四路全绿(cargo 除 R3 豁免项/clippy/gradle/swift 251)。

## 1. 实现面(逐件)

### ① daemon(crates/daemon)
- `src/account.rs` — REST 客户端两 fn(紧随 qr_status 之后,新节「扫码授权透传(手机侧;Ruling R8)」;kv_get/kv_put 同款 `&Session` + `bearer_auth(&s.jwt)` 代打,令牌不出 IPC——T8 卫生口径):
  - `qr_claim(s, qr_token)` → `QrClaimOutcome`:200 → `Claimed(QrCandidate{name, platform, expires_at})`(服务端 candidate 对象原三字段,拍平留给 IPC 层);404 → `Gone`(不存在/已过期,服务端惰性落库同 404);409 → `Contested`(他端已认领或已 settle);其余(401/限速/网络)Err 带 HTTP 上下文。
  - `qr_approve(s, qr_token, allow)` → `QrApproveOutcome`:200 → `Accepted`;409 → `Dead`(单次裁决重放/已过期);403 → `Forbidden`(非认领者);其余 Err。
  - 模块头注释同步补 R8 增补行。
- `src/api.rs` — 两 op 注册 + 实现(按 kv.get/qr_status 邻座风格):
  - `qr.claim {token}` → `{candidate_name, candidate_platform, expires_at}`(候选面拍平投影);`Gone` → typed `not_found`(`data.kind`,文案「二维码已失效,请重新扫码」);`Contested` → typed `contested`(「该二维码已被其他设备认领」);未登录 code 1「未登录,无法授权扫码登录」。同认领者重扫幂等(服务端回同面)。
  - `qr.approve {token, allow}` → `{status}`(裁决状态投影):`Accepted` → `allow ? "ok" : "denied"`;`Dead` → `"expired"`(**正常投影非 RPC 错**——结果行文案面);`Forbidden` → typed `forbidden`;未登录 code 1 同上。**`limit` 不由本 op 投影**:服务端 qr-approve 只落 approved/denied 态,配额超额的 enroll_device 收口发生在 PC 侧 qr_status 单次领票(手机侧绝不轮询,否则会抢走 PC 的单次票)——`limit` 保留在契约全集与 UI 文案映射作防御面,api.rs doc 注释写明此裁定。
  - 分派表 + 模块头 doc 增补;`need_bool` 复用 approval.respond 同款。
- **裁定依据**:R8 原文(progress.md L49)+ 服务端实现面(auth.rs `qr_approve` 仅置 approved/denied;`qr_status` Approve 相位才跑 enroll_device 可出 limit)。任务书「pass through server settle status: ok|denied|expired|limit」按可观测面落地:200+allow→ok、200+拒→denied、409→expired,limit 为防御面不产出。

### ② golden REGEN(crates/ipc/contract,+6 文件)
- `qr.claim.req/resp.json`(主形态)、`qr.approve.req/resp.json`(主形态)、`qr.claim.resp.expired.json`(死码 typed not_found 错误帧)、`qr.approve.resp.expired.json`(单次裁决重放/死码 expired 投影)。
- fixture 扩展(`tests/contract_golden.rs`):桩增 `/auth/qr-claim`(**断言 Authorization 头为非空 Bearer**——钉死 Bearer 代打;固定 token 回固定候选面 `契约新机/windows/QR_EXPIRES`,platform 刻意不取宿主 OS,双平台逐字节一致;死码 404)+ `/auth/qr-approve`(固定 token 200 `{}`,其余 409);常量 `QR_DEAD`/`QR_CANDIDATE_NAME`;场景在已登录态(KV 节后)capture 四帧(变体 req 不入库,主形态已锁——kv.get.resp.notfound 同口径);桩注释同步。
- **REGEN 幂等**:`REGEN=1` 跑后 git status 仅 6 新文件;普通模式复跑绿;提交后复跑仍绿。

### ③ Kotlin(android/,契约绑定零 UI)
- `Contract.kt`:Ops 增 `QR_CLAIM`/`QR_APPROVE`;`QrClaimResult(candidateName/candidatePlatform/expiresAt)` + `QrApproveResult(status)`;params builders(qrClaimParams/qrApproveParams)+ parse(parseQrClaim/parseQrApprove)。
- `IpcClient.kt`:`qrClaim(token)` / `qrApprove(token, allow)` 类型化面(V3-4 消费 UI)。
- `ContractGoldenTest.kt`:forms 增两形态(req 帧逐字节 + resp typed);新用例「扫码授权变体」吃两变体帧(claim 死码 → RpcException code 1/kind not_found/文案逐字;approve → status expired);孤儿防护消费面增 `qrApprovalVariants` 集合(目录 6 新文件全消费,反向存在断言覆盖)。

### ④ Swift(ios/)
- Kit 契约(`Ipc/Contract.swift`):Ops 两常量;`QrClaimResult`/`QrApproveResult`(镜像 Kotlin);Contract.qrClaimParams/qrApproveParams。
- `Data/MtdssRepository.swift`:`qrClaim(token:)` / `qrApprove(token:allow:)` 两方法。
- `Ui/QrApprovalModel.swift`(新,@MainActor @Observable,LoginModel 同象):
  - `claim(token:)` → `Stage.confirming(QrClaimResult)`(候选面供确认弹层)/`Stage.failed`(daemon 文案直出);
  - `approve(allow:)` → `Stage.result(结果行)`:ok=已同意扫码登录 / denied=已拒绝该设备登录 / expired=二维码已过期,请重新扫码 / limit=对方需选择移除设备后完成登录(防御面映射;未知 status 兜底「二维码状态异常」)/ RPC 失败 → `Stage.failed`;相位守卫仅自 confirming 接受(取消/迟到裁决零外发);
  - `dismiss()` 清场;`callTimeout` 实例可注(测试 0.2s);RpcGuard.withTimeout 15s。
- App 接线(`Profile/ProfileView.swift`):ScanSheet onCode → 建 `QrApprovalModel` → claim;**候选确认 alert**(candidate_name(platform) + 同意登录/拒绝(destructive)/取消)→ approve → **结果行 alert**(result/failed 共用,好即清场)。alert 绑定细节:确认层 isPresented 加 `qrVote: Bool?` 同步闸——按钮动作先置值再散 Task,杜绝「绑定恒真重弹」与裁决 Task 晚于 set(false) 清场的竞态;失败面文案 daemon typed 直出。**ScanSheet/QrLoginCode 解析与权限零改动**(仅头注释的链路指向从「随 V3-4」改为「已通,见 QrApprovalModel/ProfileView」,防注释失真)。
- `ContractGoldenTests.swift`:forms 增两形态 + `qrApprovalVariants` 消费面 + 新用例 `testQrApprovalVariantsTypedParse`(死码 typed not_found / expired 投影)。

### ⑤ daemon e2e(crates/daemon/tests/login_v3p1.rs)
`qr_full_chain_claim_then_approve` 重写为 **IPC 全链双端驱动**(原 HTTP 直驱撤,头注释同步):受信手机 daemon(A)与 PC daemon(B)两端——
- qr_begin(IPC)→ qr_url 固定前缀断言 → 未认领 pending;
- **未登录端认领 → code 1「未登录」**(Bearer 面负例);
- `qr.claim`(IPC)→ 候选名/平台(std::env::consts::OS)/expires_at 拍平断言 → 同认领者幂等重扫回同面 → PC 轮询仍 pending;
- 死码 `qr.claim` → code 1 + `data.kind=not_found` + 文案;
- `qr.approve allow=true`(IPC)→ status ok → PC qr_status ok 领票 → session.json 落盘 + wait_online → 收口后槽清(重放 -32602);
- **拒绝路全链**(第二轮):qr_begin → claim → `qr.approve allow=false` → status denied → PC 轮询 denied 终态(不落新会话);
- settle 后重放裁决 → status expired(单次)。

## 2. TDD 证据

- **e2e 红先**:op 注册前跑新链 → 编译红(未知方法无编译期钩,链路以断言红呈现)→ 实现后首跑红于我方断言写错(deny 轮断言「session.json 不存在」与首轮 ok 已落会话矛盾——撤断言留注释)→ 修正后绿(10/10)。
- **Swift 红两轮(Mac mini 远程环)**:push#1(35efad4)Mac 红:`Self.callTimeout` 指向实例属性编译错(任务书注 testability 注入改实例后残留静态形)→ push#2(c471da7)改实例直取 → **绿 251/251**;push#3 补 Mac 侧漏 fetch(裸仓已新、checkout 仍旧,重跑前 fetch+reset)。
- golden:REGEN 写 6 文件 → 普通模式绿(幂等)→ 提交后复跑绿。

## 3. 门禁输出

| 门 | 命令 | 结果 |
|---|---|---|
| Rust 测 | `unset MTDSS_UPDATE_URL && cargo test --workspace` | 全绿;唯 **restart_marks_interrupted FAILED**(daemon_e2e.rs 150s 时限重启用例)——**EXEMPTION R3 任务书点名豁免**,与本变更无涉(qr 链不触任务重启面);其余 24/25 binary 全 ok,login_v3p1 10/10、contract_golden 1/1 |
| Rust lint | `cargo clippy --all-targets --all-features -- -D warnings` | 0 warning |
| Android | `MSYS_NO_PATHCONV=1 ./gradlew.bat :app:testDebugUnitTest :app:assembleDebug` | BUILD SUCCESSFUL(单测全绿含 4 新断言面) |
| iOS | Mac mini `swift test`(经 macmini 裸仓,fetch+reset 同步 c471da7) | **Executed 251 tests, with 0 failures**(含 QrApprovalModelTests 9 例 + ContractGoldenTests 新变体例) |

## 4. 文件清单

- 改:`crates/daemon/src/account.rs`、`crates/daemon/src/api.rs`、`crates/daemon/tests/contract_golden.rs`、`crates/daemon/tests/login_v3p1.rs`、`android/.../ipc/Contract.kt`、`android/.../ipc/IpcClient.kt`、`android/.../ipc/ContractGoldenTest.kt`、`ios/package/.../Ipc/Contract.swift`、`ios/package/.../Data/MtdssRepository.swift`、`ios/package/.../Ui/QrLoginCode.swift`(注释)、`ios/package/Tests/.../ContractGoldenTests.swift`、`ios/app/MTDSS/Profile/ProfileView.swift`、`ios/app/MTDSS/Profile/ScanSheet.swift`(注释)
- 新:`crates/ipc/contract/qr.claim.req.json`、`qr.claim.resp.json`、`qr.claim.resp.expired.json`、`qr.approve.req.json`、`qr.approve.resp.json`、`qr.approve.resp.expired.json`、`ios/package/Sources/MTDSSKit/Ui/QrApprovalModel.swift`、`ios/package/Tests/MTDSSKitTests/QrApprovalModelTests.swift`

## 5. 自检(对照任务书)

- 五件套齐 ✔;门禁四路全绿 ✔(cargo 唯 R3 豁免项红);REGEN 幂等 ✔(三跑两比对);无 UI scope creep ✔(Android 零 UI;iOS 仅 ProfileView 弹层接线,ScanSheet/QrLoginCode 解析/权限零改动);Kotlin 绑定必须件 ✔(孤儿守卫双向:未消费新文件与声明缺失都红);中文文案合房风(二维码已失效/请重新扫码、移除设备)✔。
- jwt 不出 IPC ✔(daemon 持 session.jwt 代打,rest 层 bearer_auth;IPC 投影仅候选面/状态面);手机侧不轮询 qr_status ✔(单次票归 PC,e2e 无染指路径)。

## 6. 顾虑(concerns)

1. **`limit` 状态本 op 恒不产出**(服务端把 enroll_device 收口留给 PC 的 qr_status 单次领票):契约全集与 UI 映射按任务书保留 limit 文案为防御面。若后续服务端把配额门前移到 qr-approve,daemon 投影需增一臂(现成映射已备)。
2. restart_marks_interrupted 为 R3 点名豁免的宿主时限 flake,未追(任务书口径);若后续根治属独立项。
3. `qrVote` 同步闸是 alert 驱动的 iOS 侧行为件(非契约面),Swift 单测不覆盖(弹层呈态归真机 T14 场验)。
