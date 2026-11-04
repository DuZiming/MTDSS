# MTDSS V3-1 运维手册(账号安全:邮件发码 / 限流 / 审批 / 踢选 / admin 定值)

> 基线:V3-1 波(`docs/superpowers/specs/2026-11-04-mtdss-v3p1-account-security-design.md`)。
> 覆盖:阿里云 DM 发信装配(§1-§2)、admin 终端数定值(§3)、**发版顺序**(§4,
> 三端先发→服务端后升)、升后核验与回退(§5-§6)。
> 服务端资产与既有部署:`scripts/deploy/README.md`;上一轮运维手册:`docs/runbook-1.1.0.md`
> (部署/更新源/签名/传输冒烟,本手册只写 V3-1 增量,不重复)。
> 实机冒烟清单:`docs/smoke-v3p1.md`。

## 0. V3-1 服务端增量一览(为什么这些配置存在)

| 面 | 行为 | 配置依赖 |
| --- | --- | --- |
| 验证码投递 | 真实邮件(阿里云 DM SingleSendMail);**发信成功才落库**,失败 500 且无半态 | DM 四 env(§2);缺任一 = request-code 一律 500 fail-closed |
| dev 回显门 | `--dev-echo-code` 开 = 响应带 code、不发信、**不限速**;关 = 真实投递 + 限速 | **生产必须关**(unit 不带该旗);开=免费管道无限轰(§5 核验) |
| 限流 | request-code per-email 3/min、10/h(2026-11-05 用户裁定放宽)+ per-IP 10/h → 429+`Retry-After`「稍后再试」;approve-code 30/min、login-status 120/min per-IP;qr-begin 10/h per-IP | 无配置,进程内滑动窗口(重启清零) |
| 新终端审批 | 账号已有在线受信终端时,新设备 verify 挂审批(300s TTL;弹窗同意 / 6 位审批码直输两路) | 无配置 |
| 终端数上限 | `accounts.device_limit`(建库默认 3;0=不限);verify/enroll 收口撞门 → limit 形态 + 踢选(单次凭据 600s) | `user set-device-limit`(§3) |
| 扫码登录 | PC 铸 128bit 单次 qr_token(120s)→ 已登录手机 claim/approve → PC 领票 | 无配置 |
| 设备管理 | `device list` / `device unbind`(懒踢:被摘设备重连被拒 → 客户端自清会话回登录态) | admin CLI(§3) |

## 1. 阿里云 DM 控制台步(运营者前置,一次性;实机场前完成)

1. **开通邮件推送**:阿里云控制台 → 搜索「邮件推送(DirectMail)」→ 开通服务
   (按量计费;每天免费额度 200 封,登录验证码量级绰绰有余)。
2. **发信域名**:邮件推送 → 发信域名 → 新建域名(如 `mail.mtdss.top`,
   须与主站不同级亦可;**不得**直接用主域根,避免与既有 SPF 冲突)→ 按控制台
   给的四条记录去 DNS 解析加 **SPF(`v=spf1 include:...` TXT)/ DKIM(pubkey
   TXT _domainek)/ MX / _dmarc(可选但建议)**,回控制台点「验证」全绿。
3. **发信地址**:邮件推送 → 发信地址 → 新建(如 `noreply@mail.mtdss.top`,
   类型=发信地址);SMTP 密码可留空(本产品走 API 不走 SMTP);记下该地址
   = `MTDSS_DM_ACCOUNT`。发件人显示名 `MTDSS_DM_ALIAS`(如「星枢/MTDSS」)
   在 API 参数里,控制台不设。
4. **AccessKey**:阿里云 RAM 访问控制台 → 用户 → 创建用户(如
   `mtdss-directmail`,仅编程访问)→ 授权 `AliyunDirectMailFullAccess`
   (最小面可自建只读发送策略,量小从宽)→ 生成 AccessKey,**Secret 只显一次**,
   立即入密码库(§2 用)。

## 2. VPS unit EnvironmentFile 步(每次部署/升级核对一次)

1. 建 env 文件(四键齐;任一缺失 = request-code 500 fail-closed):

   ```bash
   cat >/etc/mtdss/dm.env <<'EOF'
   MTDSS_DM_ACCESS_KEY_ID=LTAI...（§1.4）
   MTDSS_DM_ACCESS_KEY_SECRET=...（只显一次那串）
   MTDSS_DM_ACCOUNT=noreply@mail.mtdss.top（§1.3 发信地址）
   MTDSS_DM_ALIAS=星枢/MTDSS
   EOF
   chown root:mtdss /etc/mtdss/dm.env && chmod 640 /etc/mtdss/dm.env
   ```

2. unit 挂载:`/etc/systemd/system/mtdss-signaling.service` 的 `[Service]` 段加一行
   (仓内 `scripts/deploy/mtdss-signaling.service` 已带注释占位):

   ```ini
   EnvironmentFile=/etc/mtdss/dm.env
   ```

   并确认 `ExecStart` **不带** `--dev-echo-code`(生产形态;dev 门开则不发信
   不限速,§0 表)。`ProtectSystem=full` 下 /etc 对进程只读可读,env 可加载。
3. 生效与核验:

   ```bash
   systemctl daemon-reload && systemctl restart mtdss-signaling
   systemctl show mtdss-signaling -p EnvironmentFiles      # 应含 /etc/mtdss/dm.env
   journalctl -u mtdss-signaling -n 20 --no-pager          # 不得有「验证码邮件投递未配置」warn
   ```

   装配逻辑:dev 门关 + 四键齐 = 真实投递;dev 门关 + 缺键 = 启动 warn +
   request-code 一律 500(fail-closed,不会静默半配)。
4. 真发信核验(即 smoke-v3p1 §A1):预置账号 → 客户端领码 → 邮箱收件
   (标题「MTDSS 登录验证码」,正文「你的验证码是 XXXXXX,10 分钟内有效。
   若非你本人操作,请忽略。」);未到先查垃圾箱,再查 `journalctl` 的 DM 错误。

## 3. admin 终端数定值与设备管理(存量账号建议)

```bash
# db 路径与用户沿既有口径(WAL 并存,运行中可直接执行)
M="sudo -u mtdss mtdss-admin --db /var/lib/mtdss/signaling.db"

$M user list --with-usage            # 全量账号;--with-usage 带当期中继用量
$M user set-device-limit <email> 5   # 终端数上限;0=不限;建库默认 3
$M device list <email>               # 该账号设备清单(device_id/名字/平台/last_seen)
$M device unbind <email> <device_id> # 摘除设备(丢机/换机处置):行删+全账号通告
```

- **定值建议**:家庭自用 3(默认)起步;给外人开的账号 `set-device-limit 2-3`
  并配合限流;不确定就 3,被 limit 形态挡住的用户走踢选屏自选移出一台,
  无需运维介入。0(不限)仅留给运维自用账号。
- **unbind 的懒踢语义**:被摘设备若在线, devices_changed 通告 + 绑定复核
  (keepalive tick,≤60s)会踢断;若离线,其下次启动重连被拒 → 客户端连续
  两次鉴权拒后**自动清会话回登录态**(identity.seed 保留),不会无限重试空转。
  无需手动清理其盘上 session。
- 审批链给运维的兜底:**同意权在受信终端**(弹窗/审批码),admin 无法越过
  审批给新设备放行;遭遇「用户进不来」先查受信端是否在线(不在线=快照空=
  直入,反而无审批)。

## 4. 发版顺序(刚性):三端客户端先发 → 服务端后升

**原因(双向容错矩阵)**:

| 服务器 \ 客户端 | 旧客户端(1.1.0) | 新客户端(V3-1) |
| --- | --- | --- |
| **旧服务器** | 现状 | ✅ 容错(verify 响应缺 `status` 字段按 ok 解析;`request-code` 响应 `{}` 无回显按「码走邮件」手输) |
| **新服务器** | ❌ **加新设备会卡审批态**(受信端在线时 verify 回 `status:"approval"` 无票据,旧客户端不识别直接挂死);**存量已登录设备不受影响**(JWT/refresh/收发链路零变更) | ✅ 全语义 |

**顺序步**:

1. **三端客户端先发**:Windows(更新链 /dl)、Android(APK)、iOS(TestFlight)
   各自按既有发布渠道推 V3-1 客户端;客户端对旧服务器完全兼容,用户无感。
2. **观察窗**(建议 ≥1 天):等三端装机面铺开(更新链自动升级 + 手动装机),
   期间新旧客户端混跑、服务器仍在旧版——一切照旧。
3. **服务端后升**:按 `docs/runbook-1.1.0.md` §2 既有部署流程替换
   `signaling-server` + `mtdss-admin` 二进制(§2 的 DM env 与 unit 步就位),
   `systemctl restart mtdss-signaling`。**升级窗口内注意**:此时仍在线的旧
   客户端存量登录不受影响;唯「旧客户端 + 新服务器」组合下**新设备登录**
   会卡审批态——升级排在装机面铺开之后就是为了把这窗口压到最小。
4. 若必须立即升级服务端(跳过观察窗):升级后立刻催升各端;对来不及升的
   端,告诉用户新设备登录前先在其受信端上把旧客户端升掉(或临时把账号
   受信端全下线——快照空 = 直入,绕开审批面)。

## 5. 升后核验(§4 步 3 之后立即做)

```bash
# ① 无回显核验(刚性):响应体必须无 code 字段(有 = dev 回显门还开着)
curl -s -X POST https://www.mtdss.top/auth/request-code \
  -H 'content-type: application/json' -d '{"email":"<预置邮箱>"}'
# 预期:200 + {};未预置邮箱:404 unknown account

# ② 限流核验:同邮箱 1 分钟内第二发
curl -s -i -X POST https://www.mtdss.top/auth/request-code \
  -H 'content-type: application/json' -d '{"email":"<同上>"}' | head -15
# 预期:HTTP/1.1 429 + Retry-After: <秒> + 体「稍后再试」

# ③ 新面存活:审批/扫码端点在位(形态错≠不存在,404/400 都证明路由挂上)
curl -s -X POST https://www.mtdss.top/auth/approve-code \
  -H 'content-type: application/json' -d '{"session_id":"x","code":"000000"}'
curl -s "https://www.mtdss.top/auth/qr-status?t=x"
# 预期:404/401/400 一类业务态(非 nginx 404/Caddy 空响应)

# ④ admin 新命令
sudo -u mtdss mtdss-admin --db /var/lib/mtdss/signaling.db user set-device-limit <email> 3
```

全过 → 按 `docs/smoke-v3p1.md` 走实机冒烟(§A 邮件/限流起步)。

## 6. 回退

- **服务端**:备份旧二进制(runbook-1.1.0 §2.2 同款 `backup-<日期>` 目录)→
  回拷 → `systemctl restart mtdss-signaling`。新客户端对旧服务器兼容(§4 矩阵),
  回退无客户端连锁动作;新库列(`device_limit` 等)向后兼容,旧二进制可带新库跑。
- **DM env**:临时摘 `EnvironmentFile` 行重启 = request-code 500 fail-closed
  (登录不可用!)——除非同时挂回 `--dev-echo-code`(临时回显形态,登录可用
  但无邮件无限速,**只准短期排障用,用完必摘**)。
- **客户端**:Windows `mtdss update <旧版>`(钉扎回滚,需 /dl 旧版 manifest
  在位);Android/iOS 重装旧包。

## 7. 已知边界与跟进票

1. `login.cancel` 是**本地作废**+服务端 TTL 自灭(300/600/120s);服务端无
   取消端点(Ruling R6)——取消后受信端弹窗残留至多 300s,裁决残留会话回报
   409/冲突面,属预期。`/auth/login-cancel` 端点在跟进票池。
2. 限流器为进程内内存态,重启清零(本规模接受,spec §3.10 钉面)。
3. 验证码 600s TTL、错满 5 次作废;邮件投递失败 = 500 无半态(先发后落)。
4. 老账号 `device_limit` 升级后保持建库默认 3,如需个别调整走 §3。
