# MTDSS V3-2 品牌切换(Synshu / 星枢)设计 spec

- 状态:**已批准,实施收官待实机场**(2026-11-05;builder check 过,4 项交审确认;T1-T8 已实施全绿,冒烟手册 `docs/smoke-v3p2.md`,真机过目 gate 挂用户侧)
- 上游:V3 伞形 spec `2026-11-04-mtdss-v3-design.md` §6/§4/§9
- 日期:2026-11-05(品牌视觉本轮 brainstorm 定稿,视觉稿全程过用户目)

## 0. 品牌释义(用户原话,2026-11-05)

> 它把散落在各个地方、各个终端的数据连接起来;不需要频繁的数据搬运,不需要中转;数据被牢牢掌握在自己手上,跨越地理限制、硬件限制、网络限制。运行在每个终端上的 Synshu 就像我掌握数据的一个个枢纽。

定位表述(关于页/物料通用):**mtdss 是技术底座,Synshu(星枢)是基于 mtdss 底座的客户端产品。**

## 1. 命名规则(用户裁定 2026-11-05:方案 A「全球单名」)

| 面 | 值 |
|---|---|
| 全球 App 显示名(三端/安装器/托盘/登录页/关于页) | **Synshu** |
| 中文名「星枢」 | 仅作品牌物料副标(锁字/官网/宣传),**不进任何 App UI**;中文系统显示名同为 Synshu。**钦定例外**:关于页定位表述(§0)逐字含「Synshu(星枢)」 |
| daemon / CLI / 服务名 / 进程名(daemon 侧) | 恒为 mtdss,不改 |
| 数据目录/收件箱目录名 | 恒为 `MTDSS`(§4:不用中文名、不用品牌名) |

## 2. 视觉系统(2026-11-05 过审定稿)

### 2.1 徽记:双弧 S

- 一笔 S 由两段反向圆弧在中心**枢点**相接:上弧冷色 `#CCD6FF`,下弧鎏金渐变 `#FFE9A3 → #FFCF5E → #EDA62C`;圆头端点,笔画宽约画布 15%。
- 语义:两个终端、一条通路,交点即枢;旋转对称天然是「枢纽」的字形化。

### 2.2 形态四件套(同一骨形,高光细节统一裁定「只留贴边、去顶部弧光」)

1. **深空瓷片(主图标)**:圆角方 rx≈24%,深空径向渐变 `#4556AB → #232A66 → #10143F`,双弧同上,贴边描边 `rgba(255,255,255,.2)`;**三端 App 图标统一用它**。
2. **乳白瓷片(浅色版)**:乳白渐变 `#FFF → #EEF0FA → #DFE3F4`,上弧换深靛 `#2B3560`,金弧同,贴边 `rgba(27,33,72,.16)`;用于浅色物料,可作 iOS 替换图标,不进安卓默认。
3. **裸版徽记(无瓷片)**:深底亮金版(#E6EBFF+金)与浅底深金版(#2B3560+#D99A2B);应用内标题栏、启动页、水印、锁字用。
4. **单色版**:一笔连线单色 S,白 `#F2F4FF` 与深靛 `#1C2340` 各一;Windows 托盘(随任务栏深浅切换)、安卓 themed icon、单色印刷。

### 2.3 锁字

- 主标:**SYNSHU 全大写、极细字重(100)、大字距**;本轮示意用 Poppins 100,**定稿换专业授权字体或完全重绘字形**——Poppins 为 SIL OFL 授权,商用合法,**可随 V3-2 成品先出货,字体定稿不阻塞验收**,后续换字为纯资产替换(等宽/几何骨架保持)。
- 中文副标:**星 枢** 两字,宋体衬线细字重,**两端对齐与主标严格同宽**(仅品牌物料用)。
- 双色系:深色系 = 深渐变徽章底 + 裸徽记 + 白字(「甲2」骨);浅色系 = 乳白徽章底 + 深空瓷片 + 深字(「丁」骨)。玻璃质感(内缘高光/斜光带/细噪点)属徽章底部件,不属图标本体。

### 2.4 色板 token

| token | 值 | 用途 |
|---|---|---|
| brand-deep-1/2/3 | `#4556AB` / `#232A66` / `#10143F` | 深空瓷片渐变 |
| brand-gold-1/2/3 | `#FFE9A3` / `#FFCF5E` / `#EDA62C` | 鎏金弧 |
| brand-cool | `#CCD6FF` | 冷弧/浅字辅助 |
| brand-indigo | `#2B3560` | 乳白瓷片上弧/裸版深金上弧 |
| brand-pearl-1/2/3 | `#FFFFFF` / `#EEF0FA` / `#DFE3F4` | 乳白瓷片渐变 |
| brand-ink / brand-mist | `#1C2340` / `#6A7194` | 浅底字色/副标 |

### 2.5 资产管线

- SVG 源文件(四件套 + 锁字两版)入库 `brand/`(repo 根,目录名定稿由计划定,如 `assets/brand/`);导出件:Windows `.ico`(16–256)+ NSIS 用图、iOS asset catalog(1024 + 各型)、Android adaptive(bg 深空层 + fg 裸双弧层 + monochrome 单色层 + 各密度 mipmap)、托盘单色 PNG 深浅两套 × 登录/登出两态(登出=40% 透明变体,§3.1)。
- **过目 gate(伞形 §6 沿用)**:各端真机/实机渲染图(实际尺寸图标、安装器、托盘)落地前再过用户一眼。

## 3. 各端落地面(只改显示层)

### 3.1 Windows

| 项 | 现状锚点 | 动作 |
|---|---|---|
| 产品名/窗口标题 | `ui/src-tauri/tauri.conf.json` productName/title = MTDSS | → Synshu(identifier `com.mtdss.ui` 不动) |
| exe/安装器 | Tauri NSIS bundle(productName 驱动;`installMode: currentUser`) | 安装器/开始菜单/卸载项随 productName;安装目录 = `%LOCALAPPDATA%\Synshu`(维持 per-user,不翻 perMachine);主 exe 现名 `ui.exe`(Cargo 包名驱动),改名 `Synshu.exe` = 顺手项(tauri `mainBinaryName` + `ui/src-tauri/windows/hooks.nsh` + `crates/daemon/src/uilaunch.rs:24` 三锚同步),非验收强制 |
| 托盘 | `crates/daemon/src/tray.rs`(「MTDSS」「MTDSS — N 个传输中」、更新通知) | tooltip/通知文案 → Synshu;图标换单色版双套(白/深靛随任务栏深浅)+ **保留登录态视觉通道**:登录=单色不透明、登出=同形降透明(40%)变体,`icon_for_true_false` 既有语义与测试迁移(M3a §5.1 行为不丢);daemon 进程名/服务注册恒 mtdss 不动 |
| UI 文案 | `ui/src/`(App/Sidebar/Toolbar/i18n zh.json 等 MTDSS 字样) | 用户可见处 → Synshu;关于页加定位表述(§0,钦定例外可含「星枢」) |
| 登录页 | `ui/src`(邮箱验证码登录) | 品牌位显示 Synshu,无中文名 |

**已装 MTDSS 1.1.0 的迁移(2026-11-05 记录人裁量,交审确认)**:productName 改 Synshu 后 NSIS 是**另一个产品身份**(新卸载项/新目录),旧版不会自动消失;且旧版自带更新器拉 `/dl` 更新链——若把 Synshu 包推上旧链,旧机将自动装出双份并存。钦定口径:**已装机器手动「先卸 MTDSS 1.1.0 → 再装 Synshu」**(用户基数=实机场+受邀小规模,全部经手);**旧更新链不推 Synshu**(Synshu 从 /dl 下载页新链分发;旧链断代策略——停更或仅热修 1.1.x——由用户定,不阻塞本波)。安装器不做旧产品检测清理(NSIS hooks 扩展不值当)。

### 3.2 iOS

- 显示名 → Synshu(CFBundleDisplayName;锚 `ios/app/project.yml` 构建期生成 plist)。**分享扩展自己的显示名**(系统分享面板行,project.yml:103)同步 → Synshu。TestFlight 应用列表名来自 **App Store Connect 应用档案**(现名 MTDSS)——只能在用户 Apple 控制台手工改,列为**用户侧操作项**,不进仓库验收(验收线 #1 对 TestFlight 列表名豁免)。
- **应用内文案清扫**(显示层,与 Windows「UI 文案」同则):分享面板组「发送到 MTDSS」「打开 MTDSS App…」(`ios/app/MTDSSShare/SharePanelView.swift:144,240,259,316,324`)、登录页头「MTDSS · 跨公网设备互传」(`Profile/LoginView.swift:56`)、扫码/审批/版本/照片权限提示(`ScanSheet.swift`、`ProfileView.swift`、`SettingsView.swift`)、三条系统权限弹窗文案(`project.yml:56,59,62,107`)→ Synshu。
- 图标:asset catalog 换深空瓷片;乳白版可作替换图标(可选,不做不阻塞验收)。

### 3.3 Android

- `AndroidManifest.xml` `android:label` MTDSS → Synshu;share activity label「发送到 MTDSS 设备」→「发送到 Synshu」。
- adaptive icon 按§2.5 三层落地(默认深空,不用乳白)。

### 3.4 邮件验证码(用户新增需求,2026-11-05)

锚点 `server/signaling/src/mail.rs`:

- 主题 `DM_SUBJECT`「MTDSS 登录验证码」→「Synshu 登录验证码」;发件别名(MTDSS_DM_ALIAS,部署 env)→ Synshu。
- **HtmlBody 新增**(SingleSendMail HtmlBody 参数):品牌化模板——SYNSHU 字头(深空底/白字/金点缀,纯 CSS 兼容邮件客户端)、**验证码独立成行、大号等宽字体(≥28px)、卡片框内**,前后无粘连标点;正文含 10 分钟时效与「非本人忽略」句。
- **可直接复制**:验证码是唯一可选文本行的纯文本(不进图片、不拆字距、不加连字符),双击/长按即整码选中;TextBody 纯文本回退版同步改(验证码独占一行),保 DM 网关与老客户端降级可读。
- ⚠️ V3-1 spec §3.5「中文模板逐字」条款被本节覆盖(模板内容升级),落地时在 V3-1 spec 补一行修订注记。

### 3.5 /dl 下载页

- `/dl` 根(或 `/dl/win`)加品牌 index 页:锁字(深色系)+ Synshu 安装包直链 + **SmartScreen 引导话术**(「更多信息 → 仍要运行」步骤图/文,伞形 §6 裁定:不购签名证、接受首装警告)+ 安卓 APK 直装引导。纯静态 HTML,部署沿用 Caddy 静态目录(scripts/deploy/README.md 规约)。

## 4. 收件箱落点统一(伞形 §4 搭车项)

| 端 | 落点 | 动作 |
|---|---|---|
| Windows | `文档\MTDSS` | 默认落点核对/改为文档\MTDSS(现状基准与锚点计划首步核对;`set_recv_dir` 改名能力不变) |
| Android | `Download/MTDSS` | 核对现状,非则对齐 |
| iOS | App Documents | 维持现状(伞形裁量 #3) |

- **中间态核验**(伞形 §4:桌面 `.mtdss` 记账 + finalize rename 逐行核验):锚 `crates/core/src/transport/intake.rs`、`crates/core/src/fs.rs`。核验「半截文件裸露在收件目录」的路径——存在则补齐(中间态只存在于记账区,完成后以本名出现);本就隐藏则仅改默认落点。核验结论(含逐行依据)写进实施计划。

## 5. 不变项(负面清单)

daemon/CLI 可执行名与服务注册、协议/proto、golden、`MTDSS_*` env(含 MTDSS_DM_*、MTDSS_UPDATE_URL)、更新清单 URL 结构(`/dl/win|android/<ver>/`)、`mtdss.top` 域与 `noreply@mtdss.top` 发信地址、数据目录与收件箱目录名 `MTDSS`/`.mtdss`/`inbox-staging` 键空间、repo/组织名。改名只发生在**用户可见显示层**。

## 6. 验收线(V2 流程:spec → 计划 → TDD → 冒烟 → 实机)

1. 三端显示名 = Synshu;托盘 tooltip/通知 = Synshu;登录页无中文名(TestFlight **应用列表名**来自 ASC 档案,属用户侧操作项,豁免)。
2. 三端 + 托盘 + 安装器图标 = 双弧 S 深空瓷片;托盘登录/登出两态可辨;**真机渲染图过用户目后**才算过。
3. Windows 安装器全流程(安装/开始菜单/卸载)零 MTDSS 字样;已装 1.1.0 机器按「先卸后装」口径冒烟;daemon 进程名仍 mtdss。
4. 验证码邮件:Synshu 主题/发件人;HTML 版验证码醒目且可整码复制;纯文本回退版同步;限速(3/min+10/h)不回归。
5. /dl 页品牌化 + SmartScreen 话术在位。
6. 收件箱落点三端如 §4;`.mtdss` 中间态核验结论落档;桌面传输冒烟零回归。
7. TDD:命名/文案变更处的既有断言(如 tray tooltip/icon_for_true_false 测试、mail 模板测试)同步迁移,新增邮件 HTML/TextBody 双版模板测试。

## 7. 留给实施计划的面

- 各端锚点精确清单(尤其 iOS plist 生成链、Android mipmap 现状、Windows 默认收件目录现状);
- SVG→各端图标导出管线(工具链与脚本,含 .ico 多尺寸、iOS asset catalog、adaptive 三层);
- NSIS 安装器品牌定制深度(标题/横幅/许可页话术);
- 邮件 HTML 模板在主流客户端(QQ/163/Gmail/Outlook)的兼容性验证方式;
- 专业字体选型与商用授权核查(或重绘字形的落地方案);
- SmartScreen 话术文案与步骤图。

## 8. 记录人裁量清单(用户未见逐条批准,交审时一并过目;含 builder check 2026-11-05 修正)

1. **安装形态(builder check 纠偏)**:维持 per-user 安装,目录 `%LOCALAPPDATA%\Synshu`(原文「Program Files」是在误判 installMode 前提下写的,作废);主 exe `ui.exe` → `Synshu.exe` 为顺手项(三锚:tauri `mainBinaryName`/`hooks.nsh`/`uilaunch.rs`),不强制;
2. **旧版迁移口径**:已装 MTDSS 1.1.0 = 手动先卸后装;旧更新链不推 Synshu(防旧机自动装出双份);安装器不做旧产品检测清理;
3. **iOS 清扫扩面**:应用内文案 + 分享扩展显示名 + 权限弹窗全改 Synshu(与 Windows「UI 文案」同则,演示视频常拍分享流程,不漏旧名);
4. **托盘登录态保留**:单色版 × 登录(不透明)/登出(40% 透明)两态,`icon_for_true_false` 语义与测试迁移,不丢 M3a 行为;
5. **关于页例外**:定位表述逐字含「Synshu(星枢)」,是 App UI 唯一中文名出现点;
6. **ASC 应用名**=用户侧手工操作项(Apple 控制台),不进仓库验收;
7. **字体**:Poppins(SIL OFL)可随成品出货,字体定稿不阻塞验收,后续纯替换;
8. 邮件发件别名与主题用「Synshu」(不带中文名,与方案 A 一致;MTDSS_DM_ALIAS 值修改属部署 env 侧操作,计划挂部署步骤);
9. 下载页放 `/dl` 根,深色系锁字;
10. 乳白瓷片仅浅色物料/iOS 可选替换图标,不进安卓默认;
11. 中间态核验随 V3-2 收件箱落点一并做(伞形 §4「由 V3 计划逐行核验」的具体落点)。
