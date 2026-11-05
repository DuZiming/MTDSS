# MTDSS V3-2 品牌切换(Synshu/星枢)Implementation Plan

> **For agentic workers:** REQUIRED SUB-SKILL: Use superpowers:subagent-driven-development (recommended) or superpowers:executing-plans to implement this plan task-by-task. Steps use checkbox (`- [ ]`) syntax for tracking.

**Goal:** UI 层全面切换为 Synshu 品牌(三端显示名/图标/托盘/安装器/邮件/下载页),daemon/CLI 恒为 mtdss;搭车收件箱默认落点统一与中间态核验。

**Architecture:** 品牌资产单一真源 = repo 根 `brand/`(SVG 源+导出 PNG),各端从它派生;改名只动显示层字符串与图标,协议/二进制名/env/数据布局零触碰;每个平台一个独立任务,资产任务先行。

**Tech Stack:** Rust(daemon/signaling workspace)、Tauri v2 + React(Windows UI)、XcodeGen/iOS、Gradle/Kotlin(Android)、NSIS 安装器、resvg(SVG→PNG)。

**Spec:** `docs/superpowers/specs/2026-11-05-mtdss-v3p2-brand-design.md`(已批准 2026-11-05;执行时 spec 随行阅读)

## Global Constraints

- **负面清单(spec §5,逐字)**:daemon/CLI 可执行名与服务注册、协议/proto、golden、`MTDSS_*` env(含 MTDSS_DM_*、MTDSS_UPDATE_URL、MTDSS_UI_BIN、MTDSSD_BIN)、更新清单 URL 结构(`/dl/win|android/<ver>/`)、`mtdss.top` 域与 `noreply@mtdss.top`、数据目录与收件箱目录名 `MTDSS`/`.mtdss`/`inbox-staging` 键空间、repo/组织名——**一律不改**。
- 品牌色板与形态以 spec §2 为准;徽记几何由 Task 1 的 SVG 源钉死,不得手调。
- 中文名「星枢」不进 App UI,**唯一例外**:定位表述「mtdss 是技术底座,Synshu(星枢)是基于 mtdss 底座的客户端产品」(登录页脚/关于面,Task 2)。
- 版本号本波统一 **1.2.0**(workspace + tauri + Android versionName/versionCode=3 + iOS MARKETING_VERSION)——记录人裁量,用户未逐条批准。
- 安装形态:per-user NSIS 不变;已装 MTDSS 1.1.0 = 手动先卸后装,旧更新链不推 Synshu(spec §3.1 迁移口径)。
- 收件箱目录名恒 `MTDSS`;Windows 新默认 `文档\MTDSS`,已设 `settings.recv_dir` 的用户不受影响。
- 邮件:TextBody(纯文本回退)与 HtmlBody 必须携带**同一个**验证码;V3-1 spec §3.5 模板条款被本波覆盖,需在其文件补修订注记(Task 4)。

## Review Focus

1. **旧新并存安装**:在已装 MTDSS 1.1.0 的机器上直接跑 Synshu 安装器(不先卸)不得静默损坏任何一侧——hooks 杀进程清单按新二进制名,旧 `ui.exe` 不归新安装器管。→ Task 2 步验证 + Task 9 冒烟 §迁移。
2. **托盘四态资产选择错配**:深色任务栏选到深图标=隐身。→ Task 3 单测钉死 `(logged_in, taskbar_light)` 四组合各自取到正确 PNG。
3. **邮件双体不一致 / 码不可复制**:HtmlBody 与 TextBody 码不同、或 HTML 里码与标点粘连选不中。→ Task 4 测试断言两体同码且码独占节点。
4. **老用户收件目录被改**:已设 `recv_dir` 的机器升级后目录漂移。→ Task 8 单测:`Some` 值原样保留,仅 `None` 走新默认。
5. **图标小尺寸糊死**:16/24px 下双弧并成一团。→ Task 1 导出全尺寸 + Task 9 真机过目 gate 钉 16px 可辨。

---

### Task 1: 品牌资产源入库 + 导出管线

**Files:**
- Create: `brand/assets/tile-deep.svg`、`brand/assets/tile-pearl.svg`、`brand/assets/adaptive-bg.svg`、`brand/assets/mark-free-light.svg`、`brand/assets/mark-free-dark.svg`、`brand/assets/mono-white.svg`、`brand/assets/mono-dark.svg`
- Create: `scripts/brand/export.ps1`
- Create(generated, 一并入库): `brand/exported/*.png`

**Interfaces:**
- Produces(后续任务按此清单消费,文件名精确):
  - `brand/exported/tile-deep-1024.png`(iOS AppIcon 与 Tauri `tauri icon` 源)+ `tile-deep-{48,72,96,144,192}.png`(安卓 API<26 mipmap 回退)
  - `brand/exported/adaptive-fg-1024.png`(裸双弧居中缩至画布 62%,四周留白=adaptive 安全区)
  - `brand/exported/adaptive-bg-1024.png`(源 `adaptive-bg.svg`:tile-deep 同款径向渐变铺满、无圆角、无双弧)
  - `brand/exported/mono-1024.png`(= mono-white)
  - `brand/exported/tray-light-on-32.png` / `tray-light-dim-32.png`(mono-white,不透明/40% 不透明)
  - `brand/exported/tray-dark-on-32.png` / `tray-dark-dim-32.png`(mono-dark 同上)

- [ ] **Step 1: 写 6 个 SVG 源**

全部 100×100 viewBox。几何核心(双弧 S,两段圆弧枢点相接、圆头端点、笔画宽 14–15)与瓷片贴边描边如下,**逐字使用**:

```svg
<!-- tile-deep.svg(主图标;rx=24 深空径向渐变 + 贴边) -->
<svg xmlns="http://www.w3.org/2000/svg" viewBox="0 0 100 100">
  <defs>
    <radialGradient id="sky" cx="0.5" cy="0.4" r="0.95">
      <stop offset="0" stop-color="#4556AB"/><stop offset="0.6" stop-color="#232A66"/><stop offset="1" stop-color="#10143F"/>
    </radialGradient>
    <linearGradient id="gold" x1="0" y1="0" x2="0" y2="1">
      <stop offset="0" stop-color="#FFE9A3"/><stop offset="0.6" stop-color="#FFCF5E"/><stop offset="1" stop-color="#EDA62C"/>
    </linearGradient>
  </defs>
  <rect width="100" height="100" rx="24" fill="url(#sky)"/>
  <path d="M62 22 A17 17 0 1 0 50 51" fill="none" stroke="#CCD6FF" stroke-width="15" stroke-linecap="round"/>
  <path d="M50 51 A17 17 0 1 1 38 78" fill="none" stroke="url(#gold)" stroke-width="15" stroke-linecap="round"/>
  <rect x="1.5" y="1.5" width="97" height="97" rx="22.5" fill="none" stroke="rgba(255,255,255,0.2)" stroke-width="1.4"/>
</svg>
```

其余 5 个由 tile-deep 按下表派生(改渐变/删瓷片矩形,双弧 path 逐字保留):

| 文件 | 底 | 上弧 stroke | 下弧 stroke | 贴边 |
|---|---|---|---|---|
| tile-pearl | linear `#FFF→#EEF0FA→#DFE3F4`(x1=0,y1=0,x2=1,y2=1) | `#2B3560` | `url(#gold)` | `rgba(27,33,72,0.16)` |
| mark-free-light | 无 | `#E6EBFF`(stroke-width 14) | `url(#gold)`(14) | 无 |
| mark-free-dark | 无 | `#2B3560`(14) | `#D99A2B`(14) | 无 |
| mono-white | 无 | `#F2F4FF`(14) | `#F2F4FF`(14) | 无 |
| mono-dark | 无 | `#1C2340`(14) | `#1C2340`(14) | 无 |

- [ ] **Step 2: 写导出脚本 `scripts/brand/export.ps1`**

用 `resvg` 渲染(`cargo install resvg` 装在先,脚本先探测 PATH,缺则报错退出并提示安装命令)。产出 Interfaces 清单全部 PNG:
- adaptive-fg = mark-free-light 渲 633px(1024×0.62)后,PowerShell `System.Drawing` 居中贴到 1024×1024 透明画布(合成步骤固定用 System.Drawing,不引入别的工具);
- tray 系列:mono-{white,dark} 渲 32px;dim 版=同源 SVG 文本替换 stroke 色值为 `stroke-opacity="0.4"` 后再渲(替换结果落 `$env:TEMP`,不入库)。

- [ ] **Step 3: 运行脚本,核对产物清单**

Run: `pwsh scripts/brand/export.ps1` 然后 `ls brand/exported/`
Expected: Interfaces 清单 9 个 PNG 全部在场,尺寸正确(resvg `--width`/`--height` 钉死)。

- [ ] **Step 4: 人眼抽检**

任选方式打开 `tile-deep-1024.png` 与 `tray-light-on-32.png`:双弧形态、冷金双色、圆头端点与头脑稿一致;16px 级缩放(资源管理器缩略图)S 形可辨。不合格回到 Step 1 修源,不得在导出端补救。

- [ ] **Step 5: Commit**

```bash
git add brand/ scripts/brand/
git commit -m "feat(brand): 双弧S品牌资产源与导出管线入库(V3-2)"
```

---

### Task 2: Windows 产品名/标题/exe 名/UI 文案/版本

**Files:**
- Modify: `ui/src-tauri/tauri.conf.json`(productName、app.windows[0].title、bundle.mainBinaryName、version)
- Modify: `ui/src-tauri/windows/hooks.nsh`(ui.exe → Synshu.exe,全部出现处)
- Modify: `crates/daemon/src/uilaunch.rs:23-30`(ui_bin_name)+ 其单测(grep `ui_bin_name|find_ui_bin` 的测试文件)
- Modify: `crates/daemon/src/api.rs`(AUTOSTART_RUN_VALUE "MTDSS"→"Synshu" + 启动清扫旧值)+ 对应测试
- Modify: `ui/src/i18n/zh.json`(5 处 MTDSS 字串 + 新增 positioning 键)
- Modify: `ui/src/App.tsx` 或欢迎页组件(渲染 positioning;grep `"welcome"` 定位)
- Modify: `Cargo.toml:9`(workspace version → 1.2.0)

**Interfaces:**
- Produces: exe 名 `Synshu.exe`;自启 Run 值名 `Synshu`(daemon 启动时若发现旧 `MTDSS` 值指向 mtdssd.exe 则删除);i18n 新键 `login.positioning` = 「mtdss 是技术底座,Synshu(星枢)是基于 mtdss 底座的客户端产品」。

- [ ] **Step 1: 改 tauri.conf.json**

productName→`Synshu`,windows[0].title→`Synshu`,顶层加 `"mainBinaryName": "Synshu"`,version→`1.2.0`。identifier `com.mtdss.ui`、bundle.resources(mtdss.exe/mtdssd.exe 映射)不动。

- [ ] **Step 2: 同步二进制名锚点**

hooks.nsh 中 `ui.exe` 全部(4 处 FindProcess/taskkill)→ `Synshu.exe`;`mtdssd.exe` 不动。uilaunch.rs `ui_bin_name()` → `"Synshu.exe"`/`"Synshu"`。跑 `cargo test -p mtdss-daemon uilaunch`(或 grep 到的测试模块)更新期望值。

- [ ] **Step 3: 自启 Run 值名迁移(TDD)**

先写失败测试:api.rs 的 autostart 测试模块(先 grep `AUTOSTART_RUN_VALUE` 找到现有断言)加断言——值名 == "Synshu";`sweep_legacy_autostart()`(新纯函数:入参 reg 值集,含指向 mtdssd 的旧 "MTDSS" 值 → 返回应删清单)对旧值返回删除、对新值/异值不返回。跑测试确认 FAIL → 实现 → PASS。

- [ ] **Step 4: zh.json 文案**

`app.title`→"Synshu";`sidebar.devices`「MTDSS设备」→「Synshu 设备」;`login.welcome`「欢迎使用 MTDSS」→「欢迎使用 Synshu」;`login.explain` 中两处 MTDSS→Synshu(语句其余不动);`login.scanHint`「使用已登录的 MTDSS 手机 App…」→「使用已登录的 Synshu 手机 App…」。新增 `login.positioning`(接口清单原文)。在登录表单下方渲染 positioning(小字、次要色)。代码注释里的 MTDSS 字样**不动**(技术注释,非显示层)。

- [ ] **Step 5: 全量验证 + Commit**

Run: `cargo test -p mtdss-daemon` 与 `cd ui && npm run build`(ts 编译过)
Expected: 全绿/构建成功。
```bash
git add ui/ crates/daemon/ Cargo.toml Cargo.lock
git commit -m "feat(ui): Windows 产品层切 Synshu——名/exe/自启键/文案/1.2.0(V3-2)"
```

---

### Task 3: 托盘品牌化(单色双弧 + 登录态保留)

**Files:**
- Modify: `crates/daemon/src/tray.rs`
- Modify: `crates/daemon/src/update.rs:753,806`(通知标题)
- Test: tray 相关测试同文件/同 crate(grep `tray_tooltip|tray_icon_rgba|icon_for`)

**Interfaces:**
- Produces: `tray_icon_png(logged_in: bool, taskbar_light: bool) -> &'static [u8]`(include_bytes 四资产,Task 1 清单);`taskbar_light() -> bool`(windows-sys RegGetValueW 读 `HKCU\…\Themes\Personalize\AppsUseLightTheme`,失败默认 true;cfg(windows) + 纯函数包装可测);`tray_tooltip`/`update_check_notify` 文案 MTDSS→Synshu。

- [ ] **Step 1: 写失败测试**

tray 测试模块新增:`tray_icon_png` 四组合 `(true,true)(true,false)(false,true)(false,false)` 各返回非空且**两两不同**的字节切片(指针/长度比对);`tray_tooltip(0)`=="Synshu"、`tray_tooltip(3)`=="Synshu — 3 个传输中";`update_check_notify` 三分支标题不含 "MTDSS" 且 Available 分支 == ("Synshu 更新可用", …)。跑测试 FAIL。

- [ ] **Step 2: 实现**

删 `tray_icon_rgba`/实心圆常量(及其旧测试);`make_icon(logged_in, taskbar_light)` 改 `Icon::from_rgba(tray_icon_png(...).to_vec(), 32, 32)`;`tray_thread` 双图标预建改为四资产 + 初始 `taskbar_light()` 选取,登录翻转分支同步带主题参量;`with_tooltip("MTDSS")`→`"Synshu"`。update.rs 两处标题字符串替换。跑测试 PASS。

- [ ] **Step 3: 全目标编译门**

Run: `cargo check --workspace` 与 `cargo test -p mtdss-daemon`
Expected: 全过(托盘装配面 cfg(windows),Linux 交叉不进依赖图——保持既有门)。

- [ ] **Step 4: Commit**

```bash
git add crates/daemon/ brand/exported/
git commit -m "feat(daemon): 托盘切 Synshu 单色双弧四态,登录态通道保留(V3-2)"
```

---

### Task 4: 验证码邮件品牌化 + HTML 模板

**Files:**
- Modify: `server/signaling/src/mail.rs`(`DM_SUBJECT`、`code_mail_body`、新增 `code_mail_html`、`signed_single_send_mail` 加 HtmlBody 参数)
- Modify: `server/signaling/tests/mail_throttle.rs:405-476`(参数序 golden 与正文断言)
- Modify: `docs/superpowers/specs/2026-11-04-mtdss-v3p1-account-security-design.md`(§3.5 修订注记一行)

**Interfaces:**
- Produces: `pub fn code_mail_html(code: &str) -> String`(自包含 HTML,无外链无图片);TextBody/HtmlBody 携带同一 `code`。

- [ ] **Step 1: 写失败测试**

mail_throttle.rs:`code_mail_body` 断言改为新纯文本模板(验证码独占一行:`"你的验证码\n\n{code}\n\n10 分钟内有效。若非你本人操作,请忽略。"`);新增 `code_mail_html` 断言——包含 `<div` 包裹且**内容恰为 code** 的独立行节点(`regex` 或包含性断言 `>012345<`)、含「Synshu」字头与 10 分钟文案;`signed_single_send_mail` golden 参数表插入 `("HtmlBody", …)`(排序位:`Format` 与 `FromAlias` 之后、`ReplyToAddress` 之前——按字典序核实后钉死)。跑测试 FAIL。

- [ ] **Step 2: 实现**

`DM_SUBJECT`→「Synshu 登录验证码」;`code_mail_body` 按新模板;`code_mail_html`——单列 `<table>` 布局、内联样式、`font-family:'Consolas','Courier New',monospace;font-size:30px;letter-spacing:6px` 的码放在带边框浅底 `<div>` 内、该 div 独占一行且前后无粘连文本;`AliyunDm::send` 同时传 TextBody 与 HtmlBody。跑测试 PASS。

- [ ] **Step 3: V3-1 spec 修订注记**

在 v3p1 spec §3.5 模板条目下加一行:「> 2026-11-05 修订:邮件模板随 V3-2 品牌化升级(HTML+纯文本双体),见 `2026-11-05-mtdss-v3p2-brand-design.md` §3.4;本节「逐字」条款作废。」

- [ ] **Step 4: 验证 + Commit**

Run: `cargo test -p mtdss-signaling`
Expected: 全绿。
```bash
git add server/signaling/ docs/superpowers/specs/2026-11-04-mtdss-v3p1-account-security-design.md
git commit -m "feat(server): 验证码邮件 Synshu 化——HTML 大码模板+纯文本回退(V3-2)"
```

> 部署侧(不入库,记 runbook):VPS env `MTDSS_DM_ALIAS` 值改 `Synshu` 后重启 signaling;在 QQ 邮箱/Gmail 各实收一封目检(HTML 渲染、长按整码复制)——归 Task 9 冒烟。

---

### Task 5: iOS 品牌化

**Files:**
- Modify: `ios/app/project.yml`(主/扩展 CFBundleDisplayName、4 条权限文案、MARKETING_VERSION)
- Modify: `ios/app/MTDSS/Assets.xcassets/AppIcon.appiconset/AppIcon-1024.png`(以 `brand/exported/tile-deep-1024.png` 替换,Contents.json 不动)
- Modify: Swift 文案清扫:`ios/app/MTDSSShare/SharePanelView.swift:144,240,259,316,324`、`ios/app/MTDSS/Profile/LoginView.swift:56`、`ScanSheet.swift`、`ProfileView.swift`、`SettingsView.swift`(执行时 `grep -rn "MTDSS" ios/app --include="*.swift"` 全列,逐个判断:用户可见→Synshu;注释/bundle id/entitlements/group id/目标名→不动)

**Interfaces:** 无代码接口;产物为显示名/图标/文案。

- [ ] **Step 1: project.yml 六处**

主 App 与 MTDSSShare 的 `CFBundleDisplayName: MTDSS`→`Synshu`(两处);`NSLocalNetworkUsageDescription`/`NSPhotoLibraryUsageDescription`/`NSCameraUsageDescription`(主 App 三条 + 扩展一条)句内「MTDSS」→「Synshu」;`MARKETING_VERSION`→`1.2.0`。bundle id `top.mtdss.*`、App Group、URL scheme、目标名不动。

- [ ] **Step 2: Swift 清扫**

grep 全列后逐处替换用户可见字符串(分享面板「发送到 MTDSS」「打开 MTDSS App…」、登录页头「MTDSS · 跨公网设备互传」→「Synshu · 跨公网设备互传」、扫码/审批/设置/照片提示句)。替换后 `grep -rn "MTDSS" ios/app --include="*.swift"` 剩余行应全为注释或技术标识。

- [ ] **Step 3: 图标替换 + 模拟器验证**

复制 `brand/exported/tile-deep-1024.png` 覆盖 AppIcon;按 `ios/build.sh` 现有档位跑一次 sim 构建通过;模拟器主屏:图标=深空瓷片双弧 S,名=Synshu,设置页头像区/登录页无 MTDSS 字样。

- [ ] **Step 4: Commit**

```bash
git add ios/
git commit -m "feat(ios): 显示名/图标/文案切 Synshu(V3-2)"
```

---

### Task 6: Android 品牌化(含图标从无到有)

**Files:**
- Modify: `android/app/src/main/AndroidManifest.xml`(label ×2、application 加 android:icon/android:roundIcon)
- Create: `android/app/src/main/res/mipmap-anydpi-v26/ic_launcher.xml`(adaptive:background/background、foreground/foreground、monochrome/monochrome)
- Create: `android/app/src/main/res/drawable/{background,foreground,monochrome}.png`(= Task 1 的 adaptive-bg/fg + mono-1024)
- Create: `android/app/src/main/res/mipmap-{mdpi,hdpi,xhdpi,xxhdpi,xxxhdpi}/ic_launcher.png`(tile-deep 48/72/96/144/192——API<26 回退,防老启动器无图标)
- Modify: `android/app/build.gradle.kts:77-78`(versionName 1.2.0、versionCode 3)

**Interfaces:** 无;manifest 引用资源名固定 `@mipmap/ic_launcher`(XML 在 anydpi-v26,实体 PNG 在 drawable)。

- [ ] **Step 1: 图标资源**

三个 PNG 从 `brand/exported/` 拷入 res/drawable;写 `mipmap-anydpi-v26/ic_launcher.xml`(三元素 adaptive-icon);manifest `<application>` 加 `android:icon="@mipmap/ic_launcher" android:roundIcon="@mipmap/ic_launcher"`。

- [ ] **Step 2: label 与版本**

`android:label="MTDSS"`→`"Synshu"`;share activity「发送到 MTDSS 设备」→「发送到 Synshu」;versionName/versionCode 如上。namespace/applicationId `com.mtdss.android` 不动。

- [ ] **Step 3: 构建 + 模拟器目检**

Run: `cd android && ./gradlew assembleDebug`
Expected: BUILD SUCCESSFUL。装模拟器:启动器图标=深空瓷片、名=Synshu;系统分享面板出现「发送到 Synshu」;长按图标出主题色单色 S(验证 monochrome 层,API 33+ 模拟器)。

- [ ] **Step 4: 收件箱落点核对(无代码)**

`InboxPlacement.kt` 注释已钉 `Download/MTDSS/`——与 spec §4 一致,零改动;在本任务 commit message 尾注「收件箱落点核对:Download/MTDSS 已合规,零改动」。

- [ ] **Step 5: Commit**

```bash
git add android/
git commit -m "feat(android): Synshu 品牌落地——label/adaptive 图标/1.2.0(V3-2)"
```

---

### Task 7: /dl 品牌下载页 + 部署文档

**Files:**
- Create: `scripts/deploy/dl-index.html`(自包含单文件:内联 CSS + 内联 SVG 裸双弧锁字——SYNSHU 大字距字标 + 星 枢 两端对齐副标 + 深空渐变底;无外部字体/图片,Poppins 不可用时回退系统细体)
- Modify: `scripts/deploy/README.md`(下载页上传位 `/var/www/mtdss/dl/index.html`;SmartScreen「更多信息→仍要运行」三步话术;**断链口径**:旧 `/dl/win` 更新 manifest 停推 Synshu,新包走新链,已装 1.1.0 手动先卸后装)

**Interfaces:** 无;纯静态页 + 文档。

- [ ] **Step 1: 写页面**

结构:锁字头图(Task 1 mark-free SVG 内联,颜色面随深色底取 light 变体)→ Windows 区(直链占位 `<ver>` 由部署时替换或指向 manifest 最新版说明)+ SmartScreen 引导三步(文字+行内符号,不用截图图)→ Android 区(APK 直装引导 + /dl/android 链接)→ 页脚定位表述。文案全中文。

- [ ] **Step 2: 本地目检 + Commit**

浏览器直接打开 `scripts/deploy/dl-index.html`:深色系渲染、两端对齐副标与主标同宽、无外链资源(断网可开)。

```bash
git add scripts/deploy/
git commit -m "feat(deploy): /dl 品牌下载页与 SmartScreen 引导、旧链断代口径(V3-2)"
```

---

### Task 8: Windows 收件箱默认落点 + 中间态核验

**Files:**
- Modify: `crates/daemon/src/lib.rs:257`(默认 recv_dir)+ `crates/daemon/Cargo.toml`(若无则加 `dirs` 依赖)
- Test: daemon 既有 recv/inbox 相关测试 + 新增默认落点单测
- Modify: 本计划文件(核验结论落档,见 Step 3)

**Interfaces:** `DaemonOpts::recv_dir` 语义不变;默认解析改为 `dirs::document_dir().map(|d| d.join("MTDSS"))` 失败回退 `mtdss_home().join("inbox")`。

- [ ] **Step 1: 失败测试**

新增单测(参数化 documents 目录):`resolve_default_recv(Some(doc), home)` == `doc/MTDSS`;`resolve_default_recv(None, home)` == `home/inbox`。既有 `settings.recv_dir = Some` 的路径测试保持原样并确认仍绿。跑 FAIL(函数未抽)。

- [ ] **Step 2: 实现**

lib.rs 抽 `fn resolve_default_recv(documents: Option<PathBuf>, home: &Path) -> PathBuf` 纯函数 + 默认调用点替换(装配处传 `dirs::document_dir()`)。**不做**旧默认目录(`home/inbox`)内容迁移——用户数据原地不动,新落点只影响未显式设置者(记录人裁量,冒烟项覆盖)。跑测试 PASS。

- [ ] **Step 3: 中间态逐行核验(spec §4,调查步,结论落档)**

核对 `crates/core/src/transport/intake.rs` 与 `crates/core/src/fs.rs` 的桌面 FsDestIo 布局,回答三问并**把结论写回本文件此处下方的「核验结论」小节**:
1. `.part` 中间文件落点在 `.mtdss` 记账区内还是收件目录可见层?
2. finalize 是否严格 rename(原子)且唯一失败路径不裸露半截件?
3. 冲突改名 `name (n).ext` 与断点续传是否引入可见中间态?
若三问全绿:仅注记即可;任一问发现裸露路径,补齐(中间文件挪入 `.mtdss` 区)并附回归测试。

- [ ] **Step 4: 验证 + Commit**

Run: `cargo test -p mtdss-daemon -p mtdss-core`
Expected: 全绿。
```bash
git add crates/ docs/superpowers/plans/2026-11-05-mtdss-v3p2-brand.md
git commit -m "feat(daemon): Windows 收件箱默认落点改文档\\MTDSS+中间态核验落档(V3-2)"
```

---

### Task 9: 冒烟清单 + 真机渲染过目 gate

**Files:**
- Create: `docs/smoke-v3p2.md`(沿 `docs/smoke-v3p1.md` 体例)
- Modify: `docs/superpowers/specs/2026-11-05-mtdss-v3p2-brand-design.md`(状态行补冒烟/收官注记)

**Interfaces:** 无。

- [ ] **Step 1: 写冒烟清单**

逐项(spec §6 验收线展开):①实机 Windows 全新装 Synshu——安装器/开始菜单/卸载项/窗口标题/托盘(登录/登出/浅/深任务栏四态)零 MTDSS、exe=Synshu.exe;②已装 1.1.0 机器先卸后装迁移路径;③收件箱默认落 `文档\MTDSS`+set_recv_dir 旧值不漂;④真邮件实收——Synshu 发件人/主题、HTML 大码、长按整码复制、纯文本回退;⑤iOS 模拟器+真机(TestFlight 构建上传后 ASC 档案名由用户改,清单标注用户侧);⑥Android 模拟器+真机(adaptive/主题色图标/分享面板 label);⑦/dl 页访问与话术;⑧桌面收发传输回归一对。

- [ ] **Step 2: 执行冒烟(实机部分与用户协同)**

可本地项全跑;真机渲染截图(三端主屏图标实际尺寸、托盘两态、安装器三屏、邮件整封)打包交用户过目——**此 gate 过后才算验收线 #2 达成**(spec §2.5)。

- [ ] **Step 3: 收官 Commit**

```bash
git add docs/
git commit -m "docs(v3p2): 冒烟清单与真机过目 gate 收官注记"
```

---

## 执行顺序与依赖

T1(资产)→ T2/T3/T5/T6 消费之;T4/T7/T8 独立可并行;T9 收官。建议序:T1 → T2 → T3 → T4 → T8 → T5 → T6 → T7 → T9(iOS/Android 真机件随用户时间窗排)。

## 核验结论(Task 8 Step 3 回填)

逐行核验 `crates/core/src/transport/intake.rs`(2026-11-05,worktree c4c2a27 后):

1. **.part/记录落点**:全部在 `<recv_dir>/.mtdss/`(INTAKE_DIR,intake.rs:70,134-137);开写前 `ensure_intake_dir` 带 Windows 隐藏属性(open_writer 注释)——**收件目录可见层零中间态** ✓
2. **finalize 原子性**(intake.rs:828-868, 356-394):整文件哈希校验 → .part fsync → unique_target `create_new` 原子占位终名 → `tokio::fs::rename` 原子改名(同卷 REPLACE_EXISTING)→ 两侧目录 fsync → 记录 done(tmp+rename 原子落盘)→ 防御清理。改名失败显式清占位重试;断电最坏 = partial 记录 + .part 重收。**无「半截内容裸露」路径** ✓
3. **冲突改名/续传**:`name (n).ext` 由 unique_target 的 create_new 竞争自然携带;续传读写都在 .mtdss 区。**不引入可见中间态** ✓

**残余理论边角(不补齐,记录在案)**:进程在「占位创建后、rename 前」的微秒级窗口被硬杀/断电,终名处会残留一个 **0 字节空壳**(非半截内容);重收时 unique_target 视其为冲突 → 终件落 `name (1)`,空壳永驻。补齐需识别「空文件是否本产物」,无法与用户合法空文件区分,误删语义风险大于微秒窗口的空壳残留——按伞形 §4「本就隐藏则仅改默认落点」口径,不改动。

