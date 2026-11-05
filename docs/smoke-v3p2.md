# V3-2 冒烟测试手册(品牌切换 Synshu/星枢)

配套 `docs/superpowers/specs/2026-11-05-mtdss-v3p2-brand-design.md`(§6 验收线)与
`docs/superpowers/plans/2026-11-05-mtdss-v3p2-brand.md`(Task 9)。手册风格沿
`docs/smoke-v3p1.md`。自动化面(全 workspace 测试、daemon/signaling 套件、
安卓 assembleDebug+真机装机、/dl 页几何断言、品牌资产像素断言)已收口;本手册
只收**实机/实机场判定**:三端真机渲染过目(gate)、安装器全流程、真邮件、
迁移路径——多数需用户在场。

> ## ⚠️ 执行前置:运营者四件事(实机场前完成)
>
> ① VPS env `MTDSS_DM_ALIAS` 值改 `Synshu` 后 `systemctl restart mtdss-signaling`;
> ② `/dl` 品牌下载页上线(`scripts/deploy/dl-index.html` → `/var/www/mtdss/dl/index.html`,
> `{{WIN_URL}}` 替换为当版安装包路径);③ **断代口径**:旧 `/dl/win/manifest.json`
> 冻结 1.1.0,永不上 Synshu 包;④ **新链就位**(deploy README「新链定义」节):
> `/dl/synshu/win/` 目录 + 1.2.1 起 `MTDSS_UPDATE_URL` 注入值换新基址——
> 1.2.0 本版不发布更新,故 ③④ 可在 1.2.1 发布前补,但**先记录在案**。

## 0. 能力边界(本波已交付,自动化面)

| 能力 | 状态 |
|---|---|
| 品牌资产:双弧 S 四形态 SVG 源 + 导出管线 + 像素断言 | ✅ T1(冷/金色簇、adaptive 居中、tray-32 可辨) |
| Windows:productName/标题/exe=Synshu.exe/自启键 Synshu+旧值清扫/文案/1.2.0 | ✅ T2(daemon 全测绿;注册表真写本机 EDR 拦截恒 SKIP,实机判定 §B-4) |
| 托盘:单色双弧四态(登录×任务栏深浅),tooltip/通知 Synshu | ✅ T3(四态 distinct + 资产解码单测;视觉 = 本手册 §B-3) |
| 验证码邮件:HTML 大码模板 + 纯文本回退同码 + Synshu 主题 | ✅ T4(模板四测 + 签名 golden 含 HtmlBody;实收 = §D) |
| iOS:显示名/分享扩展名/权限文案/应用内清扫/AppIcon/1.2.0 | ✅ T5 文件面(构建验证待 Mac mini,§C-0) |
| Android:label×2/adaptive 三层图标/回退 mipmap/1.2.0(versionCode 3) | ✅ T6(assembleDebug + 真机装机 1.2.0/3 + aapt2 label='Synshu';视觉 = §C-1) |
| /dl 品牌下载页 + SmartScreen 话术 + 断代口径 runbook | ✅ T7(几何断言:锁字同宽对齐、零外链) |
| 收件箱默认落点 文档\MTDSS + 中间态核验落档 | ✅ T8(纯函数 2/2;核验结论=可见层零中间态,见计划回填节) |

## 1. 环境与准备

| 环境 | 覆盖 | 准备要点 |
|---|---|---|
| Windows PC | §A/§B/§E/§F | Synshu 1.2.0 安装包(NSIS 产物);一台**已装 MTDSS 1.1.0** 的机器(§A 迁移) |
| Android 手机(MIX 3) | §C-1 | debug APK 已装机(1.2.0/3);视觉过目 |
| iPhone | §C-2 | TestFlight 1.2.0 构建(Mac mini 打包上传后;ASC 档案名改 Synshu=**用户侧**) |
| VPS | §D/§E | alias env 改后重启;下载页上线 |

## §A Windows 安装器与迁移路径(核心验收)

1. **全新安装**(无 MTDSS 残留的机器/干净账户):运行 `Synshu_1.2.0_x64-setup.exe`
   → 安装向导/安装目录 `%LOCALAPPDATA%\Synshu`、开始菜单「Synshu」、
   Add/Remove Programs「Synshu」——全程**零 MTDSS 字样**。
2. **exe 名**:任务管理器进程 = `Synshu.exe` + `mtdssd.exe`(daemon 恒名,负面清单)。
3. **已装 1.1.0 迁移**:先卸载 MTDSS 1.1.0(卸载项残留检查:旧目录/旧 Run 值由
   daemon 启动清扫自动删)→ 再装 Synshu → 开机自启项名 = Synshu(任务管理器
   「启动应用」页零 MTDSS)。
4. **不得直装覆盖**:在 1.1.0 之上直接装 Synshu = 双产品并存(口径:不支持的
   路径,引导用户先卸)。

## §B Windows 运行面

1. 主窗口标题/登录页 = Synshu;登录页脚定位表述含「Synshu(星枢)」(唯一例外);
   侧栏「Synshu 设备」;扫码提示「使用已登录的 Synshu 手机 App」。
2. 桌面/开始菜单/任务栏图标 = 深空瓷片双弧 S(16px 缩略可辨 S 形)。
3. **托盘四态**(过目 gate):深色任务栏登录=白 S 不透明/登出=40%;浅色任务栏
   换深靛版;tooltip「Synshu / Synshu — N 个传输中」;更新通知「Synshu 更新可用」。
4. 自启真写(本机 EDR 拦截的补偿验证):设置开机自启 → HKCU Run 出现
   `Synshu` 值(常规 Windows 机器上执行)。
5. **收件箱默认落点**:新账户/未设 recv_dir → 收到的文件落 `文档\MTDSS`;
   已设自定义目录的老机器升级后**落点不变**。

## §C 移动端渲染过目(gate)

- **C-0(iOS 构建前置)**:Mac mini `ios/build.sh` sim 构建 + 真机档签名构建;
  TestFlight 上传;**ASC 应用档案名 MTDSS→Synshu(用户控制台手工,验收豁免项)**。
- **C-1(Android)**:主屏图标=深空瓷片双弧 S、名=Synshu;系统分享面板
  「发送到 Synshu」;长按图标主题色单色 S(API 33+);登录页/设备页标题 Synshu;
  截图包 `.superpowers/sdd/2026-11-05-mtdss-v3p2-brand/shots/android-{app,home}.png`。
- **C-2(iOS)**:主屏图标/名=Synshu;分享扩展名=Synshu;登录页头
  「Synshu · 跨公网设备互传」;照片/相机权限弹窗文案 Synshu;
  Files App 文件夹名随显示名变 Synshu(平台等价物,伞形裁量 #3)。

## §D 真邮件实收

1. request-code → 收件:发件人 **Synshu**、主题「Synshu 登录验证码」。
2. HTML 版:SYNSHU 字头 + 大号等宽验证码卡片;**长按/双击验证码 → 整码一次选中**
   (无粘连标点);脚注定位表述。
3. 纯文本回退(客户端禁 HTML 时):验证码独占一行可复制。
4. 限速回归:3/min + 10/h(第 4 发 429,话术不回退)。

## §E /dl 下载页

1. `https://<host>/dl/` 深色品牌页:锁字副标与主标同宽两端对齐、深空渐变。
2. Windows 区:直链可下;SmartScreen 三步话术在位。
3. 安卓区:/dl/android 链接有效。
4. 断代:旧 1.1.0 客户端「检查更新」**不得**收到 Synshu 包。

## §F 传输回归(桌面冒烟一对)

PC↔手机 收发各一对:默认落点 `文档\MTDSS` 生效、完成件以本名出现
(无半截/无 .part 裸露)、任务名与托盘计数正常。

---

## 执行记录

- 2026-11-05 自动化面:T1-T8 全绿(见 ledger);Android 真机装机+截图已录;
  Windows 安装包构建中(本手册提交时附产物路径);其余 §A-§E 实机场项**挂用户侧**。
