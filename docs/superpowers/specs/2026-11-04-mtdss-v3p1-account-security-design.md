# MTDSS V3-1 账号安全与服务端管理 设计稿

日期:2026-11-04 | 状态:**已实施+首轮实机场主链过(2026-11-05)**(T1-T14 全量落地+整支终审+修复波;实机场 A/B/C1/D/E/F1/F3 主链真机全过,战果与当晚四枚热修回填 docs/smoke-v3p1.md §4——iOS 弹层参数序/DM TLS 读循环/JWT 密钥入库/限速放宽 3min+10h;B3/B4/C2/D4/F2 为可选余项;待用户 merge PR #23)
来源:V3 伞形设计(2026-11-04-mtdss-v3-design.md §3/§9)的 V3-1 波细化;用户级语义已在伞形 brainstorm 全部裁定,本 spec 钉协议与工程面。
基准:main@69b2821(V3 伞形 spec 已入库)。

## 1. 意图与范围

把「受信终端模型 + 扫码登录 + 新终端审批 + 终端数上限 + admin 命令 + 账号数据存储 + 任务命名统一」从伞形裁定落成可实施设计。**范围=服务端(signaling)+ proto + daemon/IPC 契约 + 三端登录面 UI**;不含常用目录本身(V3-3)、不含品牌改名(V3-2)。

## 2. 现状盘点(2026-11-04 核,文件锚)

- **验证码**:`server/signaling/src/auth.rs::request_code` 生成 6 位码存 hash 入 `codes` 表(TTL 600s),**码直接随 HTTP 响应返回**(api.rs:38 `CodeResp{code}`);服务端零邮件发送代码。daemon [account.rs:61](crates/daemon/src/account.rs#L61) 依赖回显(dev 自动预填),e2e 同样依赖。
- **verify 即入**:`/auth/verify`(email+code+pubkey+name+platform)校验码后直接 upsert `devices` 表并铸 JWT(access 24h/refresh 30d)。**无任何审批环节**。
- **换绑洞**:auth.rs:83-88 注释明示——同一 pubkey 用另一账号邮箱走完验证码即整体迁入新账号(`ON CONFLICT DO UPDATE SET account_id`),无目标账号审批。
- **presence**:内存表 `state.presence`(device_id→PeerHandle),WS Hello 过 JWT+账号存在+device 归属三验后注册;断线 `remove_if_current`;`account_peers()` 可按账号广播(Presence 帧先例);keepalive ping 20s/silent 60s。
- **推送面**:WS 出站帧 proto `WsServerFrame`(HelloOk/Presence/RelayAllocated/RelaySession/Error);服务端可对任意在线设备 `tx.try_send`(尽力而为先例:RelaySession 通告)。
- **admin**:`mtdss-admin` 已有 user_add/user_remove/user_list/relay_*(admin.rs);库为 SQLite,rusqlite,`CREATE TABLE IF NOT EXISTS` 建库。
- **任务名**:TaskRec 已有 `name` 字段(现=首项名);接收端会话级行无对端下发名(PC「乱名」根因);`ConnectRequest`(e2e.proto)与 `OrchestrateRequest`(ctrl.proto)现不带显示名。
- **proto 纪律**:增量=新消息包裹/新字段带缺省(protoc 3.12 兼容);IPC golden 单一真源 `crates/ipc/contract/`,三端契约测试锁定,变更走 REGEN。

## 3. 账号安全模型(承接伞形 §3,协议面落定)

### 3.1 信任判定与登录状态机

设备凭据=既有形态(pubkey 派生 device_id + JWT 对),**不新增凭据种类**。`devices` 表行=受信终端。

新终端(email 路径)登录状态机:

```
POST /auth/request-code {email}
  → 服务端:(预置账号校验照旧)生成 6 位码入 codes 表
  → 邮件投递真实送达(§3.5);dev 回显门(§3.6)开启时响应仍含 code,否则响应 {}
POST /auth/verify {email, code, pubkey, name, platform}
  ├─ 码错/过期 → 401(照旧)
  ├─ device 已在 devices 表(老终端重登)→ 直接铸票 {status:"ok", jwt, refresh, device_id, account_id}
  ├─ 新终端,verify 时刻 presence 快照无本账号在线受信终端
  │    → 自动受信:插入 devices 行 → 过配额门(§3.3)→ ok / limit
  └─ 新终端,存在 ≥1 在线受信终端
       → 创建审批会话 approvals 表(id, account_id, candidate{pubkey,name,platform}, code_hash, state=pending, expires_at=now+300s)
       → 对全部在线受信终端 try_send AuthNotice(§3.4)
       → 响应 {status:"approval", session_id, expires_at};新终端轮询 POST /auth/login-status {session_id}
settling(先到先裁,单次有效):
  - 受信终端点「同意」:POST /auth/approval-respond {session_id, allow:true}(Bearer)
  - 受信终端点「拒绝」:同上 allow:false → state=denied → 新终端轮询得 {status:"denied"},本次终止
  - 新终端直接输码:POST /auth/approve-code {session_id, code} → 对上 session code → state=approved
  - 超时 300s 无裁决 → state=expired → {status:"expired"};不自动降级(伞形反目标)
settled approved → 插入 devices 行 → 过配额门 → ok / limit
```

**presence 快照语义(安全边界钉死)**:是否需要审批以 **verify 处理时刻**的 presence 为准——快照时有在线受信终端,则即使它们随后全部掉线,本会话仍必须被裁决或超时(fail-closed,不存在「掉线后降级为直进」)。快照时无在线受信终端 → 直进自动受信(伞形路径 3,风险已接受)。

**换绑=新终端接入**:pubkey 已挂在账号 A、以账号 B 邮箱走 verify → 按「B 的新终端」完整走上述状态机(B 的在线受信终端审批);approved 后 upsert 迁移 account_id=A→B(既有 `ON CONFLICT DO UPDATE` 语义保留,但**仅经 approved 路径可达**——直入分支要求「device 不在 devices 表 OR 已属本账号」,已属他账号的 pubkey 一律走审批)。旧账号侧自动失去该设备(单行迁移,无吊销残留)。

### 3.2 扫码登录(仅「新终端=PC」方向)

```
PC:  POST /auth/qr-begin {pubkey, name, platform} → {qr_token, expires_at=now+120s}(免认证,单次有效)
     二维码内容 = https://www.mtdss.top/qr?t=<qr_token>
     PC 轮询 GET /auth/qr-status?t=<token>(2s 间隔,免认证)
手机(已登录,Bearer):
     POST /auth/qr-claim {qr_token} → {candidate:{name, platform}, expires_at}(幂等一认领者)
     UI:「<name>(<platform>) 请求登录,是否同意?」
     POST /auth/qr-approve {qr_token, allow}(Bearer)
settling:allow=true → 走插入+配额门 → PC 轮询得 ok(jwt/refresh/device_id/account_id)或 limit
         allow=false → PC 轮询得 {status:"denied"}
         过期/重复认领 → {status:"expired"}
```

- **扫码即知情即授权,不叠加输码**(伞形裁定);qr-claim 后 120s 内未 approve 即过期。
- 安全属性:qr_token 128bit 随机、单次、短时效;approve 需 Bearer(已登录态);钓鱼面=攻击者自摆二维码诱导已登录用户点同意——缓解=同意屏展示 candidate 设备名/平台,用户目视核对(与微信/Telegram Web 同级,接受)。
- 手机端入口=「我的」页扫码图标(iOS v1 已留占位);Windows/PC 侧=登录页「扫码登录」模式切换。**通用链接自动拉起 App(apple-app-site-association 等)不进 V3-1**,先做应用内扫描。

### 3.3 终端数上限与强制踢选屏

- `accounts` 表新增列 `device_limit INTEGER NOT NULL DEFAULT 3`(0=不限;**默认 3 为记录人裁量,admin 可改**);既有账号迁移取默认值。
- 配额门:设备行插入前查 `count(devices WHERE account_id)+1 > device_limit`(device_limit=0 跳过)。
- 超限 → **不插行、不铸正式票**,返回 `{status:"limit", enroll_token, devices:[现有清单]}`;enroll_token=单次凭据(TTL 600s,只授权踢选操作):
  - 新终端 UI:强制选择屏(列设备清单:名称/平台/在线态)→ `POST /auth/evict {enroll_token, device_id}` → 服务端删该 devices 行 + **主动关闭其在线 WS**(§3.7)→ 立即完成插入与铸票 → `{status:"ok", jwt,…}`;
  - 不选/取消/关闭 → enroll_token 自然过期,**新终端保持未登录**(伞形「不踢就退出登录状态」);
  - 被踢设备:WS 即断 + 重连被拒(device not bound),daemon 呈现「本设备已被移除」并回登录页。
- 配额门位置:email 直入路径、审批 approved 路径、扫码 approved 路径**三处同一函数**,行为一致。

### 3.4 WS 推送帧(proto 增量)

`ws.proto` 新增(全部带缺省,旧客户端不解即忽略——RelaySession 先例):

```proto
message AuthNotice {
  string kind = 1;        // "join_request" | "device_evicted" | "data_changed" | "devices_changed"
  string session_id = 2;  // join_request
  string code = 3;        // join_request:6 位展示码
  string candidate_name = 4;
  string candidate_platform = 5;
  int64  expires_at = 6;
  string data_key = 7;    // data_changed
}
```

- join_request:审批会话创建/重发(新终端重试 verify 产生新会话=新推送);**尽力而为**(try_send,与 Presence 同款),不回滚会话本身——弹窗送达依赖受信终端在线,在线判定已在快照内。
- device_evicted:被踢设备的临别通告(WS 随即被关,客户端以「WS 关+重连拒」为正身,此帧为锦上添花文案源)。
- data_changed:账号数据 KV 变更通告(§3.8),V3-3 消费。
- devices_changed:**devices 表任何增删(直入插入/审批插入/evict/unbind/换绑迁移)时对全账号在线设备广播**,客户端据此刷新设备快照——修 builder 指出的现状缺口:hub.rs 仅在 Presence 命中未知设备时刷新快照,**移除类变更现状只翻在线位、死行残留到重连**;此帧补齐移除路径(2026-11-04 builder 审读 Q-末条)。

### 3.5 邮箱码真实投递(阿里云邮件推送)

- 接入 **阿里云邮件推送(DirectMail)SingleSendMail RPC**:手写签名调用(AccessKeyId/Secret HMAC-SHA1,HTTP POST;仅新增 hmac/sha1 依赖,不引 SDK)。
- 配置经环境注入(不落库):`MTDSS_DM_ACCESS_KEY_ID / MTDSS_DM_ACCESS_KEY_SECRET / MTDSS_DM_ACCOUNT(发信地址noreply@mtdss.top) / MTDSS_DM_ALIAS(发件人名"星枢/MTDSS")`。
- request_code 流程:生成码 → **发信成功才落库**(发信失败=本次请求 500,不产生「发了响应却收不到」的半态);发信为异步最优路径?**否——同步发,失败即 500**,验证码是登录本体,静默失败不可接受。
- SPF/DKIM/发信地址配置=运营者侧一次性动作(阿里云控制台+DNS),进 runbook 清单。
- 邮件文案:中文模板「你的验证码是 {code},10 分钟内有效。若非你本人操作,请忽略。」

### 3.6 dev 回显门(测试与本地开发)

- signaling-server 启动参数 `--dev-echo-code`(默认**关**):开启时 request_code 响应保留 `{code}` 回显且**不发信**(本地/e2e/冒烟);关闭时响应 `{}` 且必须配好 DM 凭据。
- 产线 VPS unit 恒不开启;runbook 加核验步(`curl request-code` 响应无 code 字段)。
- daemon account.rs 客户端:回显字段缺失=正常(去掉自动预填依赖,验证码手输);dev 服务器(测试)回显仍在→dev 自动预填 UX 保留但改为「有则填」。

### 3.7 踢设备/解绑的服务端正身(2026-11-04 builder 审读 Q1 裁定)

架构事实:`mtdss-admin` 是独立进程直接开 SQLite 文件(admin_main.rs),presence 与 WS 发送柄只在 signaling-server 进程内——**admin 进程无法直接踢连接**。裁定分两档:

- **evict(交互路径,服务端进程内)**:HTTP 处理器直接 presence 查柄 → `tx.send(Error code=9 "device unbound")` → `remove_if_current` 关连接,**即时**。
- **admin unbind / user_remove(跨进程路径)**:**懒式踢**——WS keepalive 20s ping tick 上加一步**绑定复核**(每连接每 tick 一次 `SELECT account_id FROM devices WHERE device_id=?` 便宜查询;本规模几十设备无成本),行没了/归属变了 → 同 code=9 关闭。语义=**≤60s 内被踢**(ping 20s+silent 窗),运营动作可接受,不做 admin→server 进程间通道。
- **device list 在线列** = 近似:devices 表增 `last_seen` 列(presence 注册/注销时写,keepalive tick 滚动续写),admin CLI 以 `last_seen ≤ 70s` 判在线;标注「近似」。
- daemon 侧:WS Error code=9 / 重连 code 1「device not bound」→ 清本地会话(identity.seed 保留,设备身份不变)→ UI 回登录页 + 文案;`devices_changed` 通告触发设备快照刷新(§3.4)。

### 3.8 账号数据存储(KV,V3-3 消费)

- 表 `account_kv(account_id TEXT, key TEXT, value TEXT(json), updated_at INTEGER, PRIMARY KEY(account_id,key))`。
- REST(Bearer):`GET /data/:key → {value, updated_at}`;`PUT /data/:key {value} → {updated_at}`;404=无此键(合法态)。
- 变更通告:PUT 成功后对本账号**其他在线设备** try_send `AuthNotice{kind:"data_changed", data_key}`(尽力;客户端兜底=登录/上线全量拉)。
- V3-1 交付=服务端面 + daemon IPC 透传(`kv.get/kv.put`,无 UI);值语义(常用目录清单 schema)归 V3-3。

### 3.9 admin 命令(mtdss-admin 增量子命令,镜像既有风格)

```
mtdss-admin user set-device-limit <email> <n>     # 0=不限
mtdss-admin device list <email>                   # device_id/名称/平台/创建时间/在线
mtdss-admin device unbind <email> <device_id>     # 删行+踢 WS(§3.7)
```

user_list 输出增列 device_limit。

### 3.10 防滥用限流(2026-11-04 builder 审读 Q4 补面)

现状:/auth/verify 错猜不限次,6 位码在 600s TTL 内可暴力;V3-1 又新增免认证面(approve-code 同为 6 位码、qr-begin 铸 token、双 status 轮询、request-code 每发一封付费邮件=邮件轰炸面)。开账号给外人前必须收口:

- **码错次门**:codes 表与 approvals 会话各记 `attempts`;**错 5 次即作废该码/会话**(须重新 request-code / 重新发起登录);
- **request-code 限速**:per-email 3 次/分钟、10 次/小时(2026-11-05 用户裁定放宽,原 1/min+5/h 实机场偏紧);per-IP 10 次/小时(超限 429+`Retry-After`,文案「稍后再试」);
- **qr-begin**:per-IP 10 次/小时;**approve-code/qr-claim/qr-approve**:per-IP 30 次/分钟;
- **login-status/qr-status 轮询**:token 绑定+过期即拒(自带限),另加 per-IP 120 次/分钟兜底;
- 实现:进程内存计数器(单进程,重启即清——本规模接受,不建表)。

## 4. proto / IPC 契约增量

### 4.1 proto(ctrl/e2e 增量,纪律=缺省字段)

- `ConnectRequest` 增 `string display_name = 5`(缺省空=旧端回退现行为);
- `OrchestrateRequest` 增 `string display_name = 7`(**7**——5=policy、6=old_rels 已占,protoc 拒重号;builder 审读勘误);
- 接收端(responder/被动收件会话行)任务名 = `display_name` 非空取之,空则回退现状——两端同名。

### 4.2 IPC 新 op(golden REGEN,三端契约同步)

| op | 请求 | 响应 | 说明 |
|---|---|---|---|
| `login.verify`(改) | 不变 | 增判别 `status`:"ok"/"approval"/"limit";**ok 形态沿用现 golden 响应面 `{device_id,name}`(jwt/refresh 不出 IPC,daemon 内部持有——builder 勘误)**;approval 带 `session_id,expires_at`;limit 带 `enroll_token,devices` | golden 全变体 REGEN |
| `login.submit_code` | `{session_id, code}` | 同 verify 新形态 | 新终端输码路径 |
| `login.status` | `{session_id}` | 同上(approved/denied/expired/limit/ok) | 轮询收口(服务端 push 不进 V3-1) |
| `login.evict` | `{enroll_token, device_id}` | 同 verify ok 形态 | 强制踢选屏动作 |
| `login.cancel` | `{}` | `{}` | 取消进行中登录(审批会话作废) |
| `login.qr_begin` | `{server, ca?, name}`(镜像 request_code 参数面——builder 审读 Q2:daemon 唯一 server 来源=登录表单,session.json 未登录不存在;QR tab 复用登录页高级选项的 server/CA/设备名) | `{qr_url, token, expires_at}` | PC 登录页二维码 |
| `login.qr_status` | `{token}` | 同 verify 新形态 | PC 轮询 |
| `approval.respond` | `{session_id, allow}` | `{}` | 受信终端弹窗应答 |
| `kv.get` / `kv.put` | `{key}` / `{key,value}` | `{value}` / `{ok}` | §3.8 透传,V3-3 消费 |

事件增:`event.approval {session_id, candidate_name, candidate_platform, code, expires_at}`(受信终端弹窗;来源=AuthNotice join_request);`event.device_evicted {}`(本机被移除,清会话回登录页);`event.devices_changed {}`(设备清单增删,刷快照——§3.4)。

## 5. 三端登录面 UI(V3-1 的客户端范围)

- **Windows(GuideView 扩展)**:登录页增「扫码登录」tab(二维码+轮询+过期重生成);邮箱路径增三个中间态——输审批码屏(session_id 保持)、强制踢选屏(设备清单+「退出登录」取消钮)、失败/超时/拒绝文案。受信终端侧:托盘/主窗弹「新终端登录请求」模态(设备名+平台+6 位码+同意/拒绝)。
- **Android / iOS**:同三中间态 + 受信终端弹审批(BottomSheet/Alert);iOS「我的」页扫码占位图标点亮=应用内扫码器(AVFoundation/CameraX),扫后调 qr-claim→确认弹层→qr-approve。**审批弹窗只在 App 前台生效(无推送,伞形已裁定)**;手机端后台审批不达=预期,由桌面常驻端兜。
- 文案统一中文;错误码→文案映射表进各端 i18n/词表。
- **CLI(mtdss login)文本态全支持**(builder 审读 Q3:**Linux 客户端=纯 CLI,是真实登录面**,不容降级):新中间态在 CLI 走文本交互——approval=打印「需已登录终端同意,等待中(每 3s 轮询,可 Ctrl+C 取消)」;输码路径=提示手输 6 位审批码;limit=编号列表选踢(`输入编号踢出对应设备,q 退出`);扫码=打印二维码字符画(ASCII QR)+轮询。

## 6. 任务命名统一(引擎侧)

- 生成规则升级(daemon 发起方,jobs.rs):单文件=本名;单目录=顶层目录名;多项=「首项名 等 N 项」(现仅首项名,补后缀);写 TaskRec.name(既有字段)。
- 下发:ConnectRequest/OrchestrateRequest 携 display_name(§4.1);接收端会话行/任务行取用。
- `transfer.list`/事件 golden 内容随 REGEN;PC「接收乱名/ID 串」显示面全部收敛到 name 字段(UI 改为恒显 name,不再从路径拼)。
- 桌面 CLI(mtdss status/ls)任务名同步取 name。

## 7. 兼容与迁移

- **已登录存量设备零影响**(devices 行已在=受信;refresh/WS 路径不变)。
- **旧客户端加新设备会卡**:旧客户端不懂 `status:"approval"`,新终端审批流将表现为登录失败——**不设旧形态回退**(回退=绕审批洞,伞形反目标);运营者升级服务端与三端客户端同批进行(自有产线,runbook 编排:先发三端更新→再升服务端)。
- accounts 加列迁移:`ALTER TABLE ADD COLUMN` 幂等包(现 db.rs 无 alter 先例,补 migration 小节,重复执行安全)。
- 既有用户(运营者本人)device_limit 取默认 3,不足则 admin 自调。

## 8. 测试策略

- **服务端单测/集成**:审批状态机全分支(approve-by-respond/approve-by-code/deny/expire/presence 快照 fail-closed/单次有效);换绑三态(直入拒/审批过/迁移后旧账号失去);配额门三路径同函数一致;evict(在线踢断 WS/离线踢/自踢不可/enroll_token 过期);qr(claim 幂等/单次/过期/未登录 approve 401);kv CRUD+data_changed 通告;DM 发信(传输 trait mock:成功/失败→500/回显门双态);admin 三命令;**限流全面(码错 5 次作废/429/Retry-After);keepalive 绑定复核(懒踢 ≤60s);devices_changed 移除刷新**。
- **daemon e2e**:dev-echo 服务器起双 daemon——新终端审批全链(B 弹窗应答/A 输码两路)、配额踢选全链、qr 全链(mock claim 由测试驱动 HTTP)、被踢设备会话清理、display_name 两端同名断言、任务名「等 N 项」断言。
- **golden**:§4.2 全量 REGEN + Kotlin/Swift 契约测试同步(既有双端锁定流程)。
- **CI**:既有 job 面零新增;服务端套件扩容照常。

## 9. 运营者侧前置(runbook 增补)

1. 阿里云邮件推送:开通、发信地址 noreply@mtdss.top、域名 SPF/DKIM 配置、AccessKey 创建(只授 DM 权限)→ 四个环境变量入 VPS unit(EnvironmentFile)。
2. 三端发版顺序(§7)+ 首发后 `curl request-code` 核验无回显。
3. `mtdss-admin user set-device-limit` 对存量账号定值。

## 10. 记录人裁量清单(交审一并过目)

1. device_limit 默认 **3**,0=不限(admin 可改任意值);
2. TTL:审批会话 300s / qr_token 120s / enroll_token 600s / 验证码 600s(照旧);
3. 二维码内容 `https://www.mtdss.top/qr?t=<token>`;通用链接自动拉起不做,V3-1=应用内扫描;
4. 发信**同步**(失败即 500),不落「已发未达」半态;DM 未配置且非 dev 门=服务端启动告警+request-code 500(fail-closed);
5. 旧客户端加新设备不兼容即失败,不设回退(§7);
6. 换绑已属他账号的 pubkey 恒走审批(直入分支不放行);
7. kv 通告=尽力推送+客户端上线全量拉兜底(不做强一致);
8. 任务名「等 N 项」后缀从 2 项起(1 项=纯名);
9. **admin unbind/user_remove=懒踢 ≤60s**(keepalive 绑定复核),evict=即时;admin `device list` 在线列=last_seen ≤70s 近似(builder Q1);
10. `login.qr_begin` 参数面 `{server, ca?, name}` 镜像 request_code,QR tab 复用登录页高级选项(builder Q2);
11. **CLI 文本态全支持**(Linux 纯 CLI 是真实登录面;ASCII 二维码/轮询等待/编号踢选)(builder Q3);
12. **防滥用限流进 V3-1**(§3.10,取值如列;builder Q4——「开账号给外人前安全必须硬」的应有之义)。

## 11. 留给实施计划的

- approvals/enroll/qr 会话的内存态与表态取舍(纯表即可)、幂等与并发细节;
- daemon login 流程状态机重构(account.rs/api.rs/login op 的接线顺序);
- 三端 UI 组件级设计(Android/iOS 审批弹窗与踢选屏过审 gate 依伞形 §7-7 仅约束大改版,V3-1 增量屏不触发);
- DM 模板细节与重试策略(失败重试一次与否)。

## 12. 实施回填(2026-11-04,T14 收口)

- **落地范围**:T1-T14 全量完成(服务端 T1/T2/T4-T7 七件:审批/配额/扫码/KV/限流/DM/admin;
  daemon T8/T8b/T13b:登录状态机 + 11 IPC op + 3 事件;T9 任务代表名两端同源;
  T10-T13 四客户端面 Windows UI / CLI / Android / iOS 含扫码器与 QR 确认弹层;
  T14 边角 e2e + 冒烟/运维手册收官)。
- **T13b 增量(计划缺口裁定 R8 入正文)**:手机侧 qr-claim/qr-approve 走 **daemon 透传
  op** `qr.claim {token}→{candidate,expires_at}` / `qr.approve {token,allow}→{status}`
  (Bearer=手机 daemon 会话 JWT 代打),而非 §3.2 原设的客户端直打 REST+Bearer——
  JWT 收在 daemon 内的令牌卫生红线(V3-1)与「客户端持票」架构冲突,T13b 裁定以此收口;
  IPC golden 随 REGEN 再生,四端契约测试锁定。
- **R6 cancel 语义(与 §3.1 原设的偏差正身)**:`login.cancel` 实现为**本地作废
  (daemon 续接槽清)+ 服务端 TTL 自灭**(审批 300s/踢选凭据 600s/扫码 120s),
  服务端无取消端点——取消后受信端弹窗残留至多 300s、裁决残留会话回报冲突面,
  属已知边界;`/auth/login-cancel` 端点立跟进票候选。
- **余项**:实机场冒烟按 `docs/smoke-v3p1.md` 逐项(真邮件收码/双端审批/扫码登 PC/
  踢选/任务名两端一致,需用户在场);运营者前置与发版顺序见 `docs/runbook-v3.md`。
