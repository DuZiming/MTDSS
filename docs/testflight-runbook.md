# MTDSS TestFlight 产线 runbook(v2-P5 T13)

> 手册风格沿 `docs/runbook-1.1.0.md`。角色标注:**[用户]** = 需本人操作
> (网页/密码面);**[自动]** = `ios/build.sh device-sign` 已封自动化。
> Bundle 面:`top.mtdss.ios`(App)/ `top.mtdss.ios.share`(分享扩展)/
> App Group `group.top.mtdss.shared`——三者**首次归档时由
> `-allowProvisioningUpdates` 自动注册**,无需手动去开发者门户建。

## 0. 前提(一次性)

- [用户] Apple Developer Program 会员在册(付费——TestFlight 上传与外部组
  只有 Program 账号可用;免费个人 team 无 App Store Connect 记录面)。
- [用户] Mac mini 的 Xcode → 设置 → 账户 已登 Apple ID(T13a 时点已就绪)。
- [自动] 双 target 自动签名(`CODE_SIGN_STYLE=Automatic`),team ID 从
  Xcode 记录自探;探不到时 `MTDSS_DEVELOPMENT_TEAM=<10位Team ID>` 明传
  (Team ID 查法:App Store Connect → 会员资格详情)。

## 1. App Store Connect 建 App 记录 [用户,网页]

1. 打开 https://appstoreconnect.apple.com → 「我的 App」→ 左上 ➕ →「新建 App」。
2. 表单:
   - 名称:`MTDSS`(若重名提示,可加后缀如 `MTDSS - 设备互传`,名称唯一即可);
   - 主要语言:简体中文;
   - 套装 ID / Bundle ID:选 `top.mtdss.ios`
     (下拉里没有它 = App ID 还没注册——先在 Mac mini 跑一次
     `ios/build.sh device-sign` 让自动注册落地,再回本页刷新);
   - SKU:`mtdss-ios`;
   - 用户访问权限:默认(全部用户可访问 TestFlight)。
3. 建成即收构建,无需再填任何面(TestFlight 元数据建群组时补,见 §6)。

## 2. App 专用密码存 Mac 钥匙串 [用户,一条终端命令]

上传凭据不用账号主密码(Apple 规定二步验证账号必须用 App 专用密码):

1. 浏览器开 https://appleid.apple.com → 登录 → 「登录与安全」→
   「App 专用密码」→ 生成密码,备注名如 `MTDSS altool`,得到 16 位密码。
2. Mac mini 终端一行(替换尖括号内容;Apple ID = 你登 Xcode 的邮箱):

   ```bash
   security add-generic-password -a "<你的AppleID>" -s MTDSS_ASC -w "<16位App专用密码>"
   ```

   等价备选(altool 自带条目写入器,二选一即可):
   `xcrun altool --store-password-in-keychain-item MTDSS_ASC -u "<你的AppleID>" -p "<16位App专用密码>"`

3. 自验(只看条目在不在,不回显密码):
   `security find-generic-password -s MTDSS_ASC` 退出码 0 即可。

## 3. 归档与上传 [自动]

```bash
ssh mac 'source ~/mtdss-env.sh && cd ~/mtdss-ios && bash ios/build.sh device-sign'
```

一条链完成:引擎 device slice → xcodegen → `xcodebuild archive`
(Release,自动签名,双 target,App Group 随 provisioning 更新注册)→
entitlements 校验(钉 `group.top.mtdss.shared`)→ `-exportArchive`
(method=app-store-connect,含符号)→ `.ipa` → 凭据在场则
`xcrun altool --upload-app -f <ipa> -t ios` 直传。

- build 号缺省日期制 `YYYYMMDD.1`;**同日重传递增**:
  `MTDSS_BUILD_NUMBER=20261101.2 bash ios/build.sh device-sign`。
- 已有 `.ipa` 只补传:`bash ios/build.sh upload`。
- 图形化兜底:macOS「Transporter」App 拖入
  `ios/app/build/export/*.ipa` → 交付(上传命令跑不通时的备路)。
- 归档/导出耗时 5-15 分钟,建议 nohup 后台跑(日志见下)。

## 4. 构建落点与处理 [自动后观察]

`altool` 回 `No errors uploading` 即已入苹果管道:

- 位置:App Store Connect → 我的 App → **MTDSS → TestFlight 标签 → iOS**;
- 处理 10-30 分钟(邮件「App Store Connect: 构建已就绪」);状态先为
  **处理中**,完成后显示 `1.1.0 (build 号)`;
- 测试信息(What to Test)可即时填;外部组提交见 §6。

## 5. 出口合规(已烤入,免手答)

App `Info.plist` 已带 **`ITSAppUsesNonExemptEncryption = false`**
(仅标准 TLS/HTTPS 加密面:信令 HTTPS、QUIC+TLS 1.3、无自研/非标算法),
TestFlight 与 App Store 面均自动取豁免。若 ASC 网页仍弹出合规问答,
与同口径勾选「标准加密豁免(不使用非豁免加密)」即可。

## 6. 外部测试组与首次 Beta 审核 [用户,网页](后续轮)

1. TestFlight 标签 → 「测试员与群组」→ ➕ →「新建群组」
   (名如 `外部-首轮`),添加测试员邮箱(账号持有人两台 iPhone 的
   Apple ID 邮箱)。
2. 「构建」区把 `1.1.0 (build号)` 加入该群组 → 首次触发**轻量 Beta 审核**。
3. 提审前补齐 Beta App 信息(会被审):
   - Beta App 描述:一两句功能面(局域网/中继设备间文件互传);
   - 反馈邮箱:用户实收邮箱;
   - 隐私政策 URL:`https://www.mtdss.top/privacy`(已上线,T10b);
   - 审核备注:本地文件传输工具;无登录墙外的内容;无特殊硬件需求;
     测试可按描述自足完成。
4. 审核通常 24-48 小时;通过后测试员收邀请邮件。

## 7. 账号持有人设备安装 [用户]

1. iPhone 装 **TestFlight** App(App Store 免费)。
2. 接受 §6 邮件邀请(或 TestFlight 内已登录同一 Apple ID 直接见 App)→
   MTDSS →「安装」。
3. 首启预期:本地网络权限弹窗 → 允许(smoke-p5 §A2:拒绝 = 静默断连);
   通知弹窗可拒(通知是增益面,F5 口径)。
4. 收件箱在「文件」App → MTDSS 区;版本随构建更新,构建 90 天有效,
   到期重传即续。

## 8. 故障速查

| 现象 | 先查 |
|---|---|
| altool 认证失败(-22910 / invalid credentials) | 钥匙串 `MTDSS_ASC` 的 acct 是否完整 Apple ID;**App 专用密码**≠账号密码(§2 重生成) |
| 「No suitable application records found」 | §1 的 App 记录没建 / Bundle ID 选错(必须 `top.mtdss.ios`) |
| 处理完变「二进制无效」邮件 | ASC 构建详情页看「意见」:出口合规答缺(§5 已烤入则少见)/ 图标缺失(1024 图标已入包) |
| device-sign 报 requires a development team | `MTDSS_DEVELOPMENT_TEAM=<Team ID>` 明传(会员资格页查) |
| 归档报 App Group 能力注册失败 | Apple 开发者门户偶发;重跑 device-sign(provisioning 更新幂等) |
| 构建长时间「处理中」(>1h) | 不重传,等邮件;超 2h 再查系统状态页 |

## 9. 本轮(2026-11-01,T13a)实况记录

- 归档+导出:`ssh mac` 上 `bash ios/build.sh device-sign`,日志
  `~/t13a-sign.log`;产物 `ios/app/build/export/MTDSS.ipa`。
- 签名自动注册:双 App ID + App Group 由首次归档 provisioning 更新完成
  (team 与 entitlements 实证见 T13a 报告)。
- 上传:凭据(钥匙串 `MTDSS_ASC`)到位后 `bash ios/build.sh upload`。
