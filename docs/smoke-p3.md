# v2-P3 冒烟测试手册(Android 客户端)

配套 `docs/superpowers/specs/2026-10-25-mtdss-v2p3-android-design.md` §8 的逐项
展开;覆盖 T7-T12 全部实机验收面 + 非实机项(CI / 签名 / 16KB / 发布演练)的
现状记录。手册风格沿 `docs/smoke-m4.md`。

> ## ⚠️ 全部实机项 PARKED(待设备)
>
> **测试机 MIX 3(Android 10,API 29)尚未连接。** 下文 §A-§G、§I 标注
> **PARKED(待设备)** 的条目 = 代码/构建已交付(T7-T12),判定待设备场执行;
> 设备场由用户在场协作(装包/操作/MIUI 权限弹窗),逐项打勾 + 记录现象。
> 非实机项(§H、§J)已按记录完成,勾选态即现状。

## 0. 能力边界(当前已交付)

| 能力 | 状态 |
|---|---|
| 原生客户端:登录 / 设备列表 / 浏览对端 / 收发(选择器+授权根+分享)/ 任务列表(暂停/恢复/取消/清除/重新发起)/ 设置 | ✅ T9-T12 代码交付;实机判定 PARKED |
| 引擎子进程复用(mtdssd 入 APK,PDEATHSIG 随亡,意外退出退避重启) | ✅ 代码交付;实机判定 PARKED |
| IPC 契约移植 + 防漂移(51 golden 双端逐字节 + 孤儿防护) | ✅ JVM 套件绿 |
| 断点续传(重新发起 = P2 语义;bitmap 对账 + fd 直写收件箱) | ✅ 代码交付;实机判定 PARKED(headline 项 E2) |
| 应用内更新:M4 同源 manifest(ed25519 验签 + SHA-256/字节数复核)→ 下载进度 → FileProvider 拉起安装器 | ✅ 代码交付;实机判定 PARKED(§F) |
| CI android job(引擎交叉编译 + JVM 单测 + debug APK) | ✅ yaml 交付;首次运行待 push 后观察 |
| release 签名(keystore 离线保管,fail-visible)+ 16KB 对齐检查 | ✅ 命令/检查交付并本机验证;真实 keystore 待用户生成 |
| 常驻/前台服务/通知、自动续传、全盘浏览、静默自装、iOS | ❌ 设计裁定不做(§2 非目标) |

## 1. 环境与准备

### 1.1 环境矩阵

| 环境 | 覆盖测试项 | 准备要点 |
|---|---|---|
| MIX 3(Android 10 / API 29 / arm64) | §A-§G、§I | 开发者选项 + USB 调试;MIUI「USB 安装」可能要求登录小米账号;MIUI 双层权限弹窗(原生 + 安全中心)按真实环境验 |
| Windows PC(同账号对端) | §B-§G 的对端面 | M4 已发布的 Windows 客户端,登录同一账号 |
| 更新源(本地静态服务器或 VPS) | §F | §1.2 布局;`mtdss-sign` 手工发布(见 scripts/android/README.md §5) |

### 1.2 更新源布局(本机回环演练最小形)

```
<root>/dl/
└── android/
    ├── manifest.json              # 最新版签名清单(App 检查更新拉这里)
    └── <ver>/MTDSS_<ver>_arm64.apk
```

- 本机回环:`python -m http.server 8000` 起根目录,App 构建注入
  `-PMTDSS_DL_BASE=http://<PC_IP>:8000/dl -PMTDSS_UPDATE_PUBKEY=<keys/update-pub.hex>`
  (注入细节见 `scripts/android/README.md` §2;`HttpURLConnection` 走系统信任
  库,自签 https 需设备导入 CA 或演练期用 http)。
- 清单生成:`mtdss-sign manifest --platform android-arm64` → **改 asset url 的
  `/win/` 段为 `/android/`** → `sign` → `verify`(v1 已知绕法,见
  scripts/android/README.md §5)。

### 1.3 客户端构建口径

| 场景 | 命令口径 |
|---|---|
| 主链验机(§A-§E) | `./gradlew assembleDebug`(更新源不注入 = 检查更新呈未配置,§F1 顺带验) |
| 更新链验机(§F) | 同上 + `-PMTDSS_DL_BASE=… -PMTDSS_UPDATE_PUBKEY=…` |
| 更新链的目标版本(§F3) | 改 `android/app/build.gradle.kts` 的 `versionName`(严格 X.Y.Z)与 `versionCode` 再构建 |

## 2. 测试项清单

> 每项:前置 / 操作 / 预期;记录格式见文末。设备在场时逐项执行。

### A. 安装与引擎拉起(T7;PARKED 待设备)

- [ ] **A1 安装起屏**(PARKED 待设备)
  - 前置:adb 连接 MIX 3;`app-debug.apk` 在位。
  - 操作:`adb install -r app-debug.apk` → 桌面图标 → 点开。
  - 预期:进入登录屏(未登录态);无崩溃;无「应用未安装」(minSdk 29 ✓)。
- [ ] **A2 引擎进程与日志**(PARKED 待设备)
  - 前置:A1 通过。
  - 操作:`adb logcat -s MtdssEngine mtdssd` 观察。
  - 预期:见「引擎已拉起 home=… bin=…libmtdssd.so」;mtdssd 启动日志逐行
    转发到 logcat(tag `mtdssd`);App 首屏呈「连接引擎中…」毫秒级收敛为登录屏。
- [ ] **A3 hello/诊断面**(PARKED 待设备)
  - 前置:A1-A2。
  - 操作:登录后 设备列表 → 设置 → 诊断。
  - 预期:引擎「运行中」、IPC「已连接」、设备名/ID 显示;`status` 权威数据可达。
- [ ] **A4 引擎随亡 PDEATHSIG**(PARKED 待设备)
  - 前置:A2。
  - 操作:设置 → 开发者选项 → 正在运行的服务里强停 MTDSS(或 `adb shell am
    kill`);对照:正常退出 App(最近任务划走)。
  - 预期:App 进程亡 → logcat 见引擎进程退出(父线程消亡 SIGKILL 生效);
    PC 侧设备列表见 MIX 3 离线(§I 判定同源)。
- [ ] **A5 意外退出退避重启**(PARKED 待设备)
  - 前置:A2。
  - 操作:`adb shell pkill -f mtdssd`(模拟引擎崩溃)。
  - 预期:MtdssEngine 日志「引擎退出 code=… → N ms 后重启」;任务列表的
    running 行翻「中断」(P2 语义,引擎自愈);App 内连接自愈重连。

### B. 登录与设备列表(T9;PARKED 待设备)

- [ ] **B1 登录链**(PARKED 待设备)
  - 前置:账号已 `mtdss-admin user add` 预置;PC 与 MIX 3 同网络可达信令。
  - 操作:登录屏填 server(备案前演练形态 `https://<VPS_IP>` + 设备导入 CA,
    或回环拓扑)、邮箱 → 收码 → 填码 + 设备名。
  - 预期:logged_in 翻转;登录屏切设备列表;设置页账号区显示设备名/ID/在线。
- [ ] **B2 双向可见**(PARKED 待设备)
  - 前置:B1;PC 已登录同账号。
  - 操作:双端各看设备列表。
  - 预期:MIX 3 见 PC(在线),PC 见 MIX 3(在线);平台标识正确
    (windows / android)。
- [ ] **B3 登出与重登**(PARKED 待设备)
  - 操作:设置 → 退出登录。
  - 预期:回登录屏;PC 侧设备列表 MIX 3 离线;重登后恢复,历史浏览位不复活
    (回设备列表)。

### C. 浏览对端 + 下载收件箱(T9;PARKED 待设备)

- [ ] **C1 浏览 PC 目录**(PARKED 待设备)
  - 前置:B2。
  - 操作:点 PC 设备 → 逐级浏览目录。
  - 预期:目录树/文件行(名/大小/时间)与 PC 实况一致;分页/截断标志按
    golden 字段呈现;返回键回上级/设备列表。
- [ ] **C2 拉文件到收件箱**(PARKED 待设备)
  - 操作:选文件 → 下载。
  - 预期:任务列表见 download 行进度;完成后 MIX 3 系统文件管理器
    `Downloads/MTDSS/` 出现该文件(内容逐字节);嵌套目录落位为
    `RELATIVE_PATH` 多段(评审裁定② 落位语义)。
- [ ] **C3 IS_PENDING 揭开**(PARKED 待设备)
  - 前置:C2 传输进行中。
  - 操作:文件管理器刷新观察半成品。
  - 预期:传输中文件呈 pending(文件管理器不可见/零字节);完成瞬间揭开可见
    (MediaStore IS_PENDING 语义)。

### D. 发送(T10;PARKED 待设备)

- [ ] **D1 选择器发送**(PARKED 待设备)
  - 前置:B2。
  - 操作:MIX 3 设备行快发(或浏览页发送)→ 系统选择器多选 → 确认弹层 →
    填 PC 目标目录 → 发送。
  - 预期:PC 收到,内容逐字节一致;`fs.request_source` 逐文件懒开 fd(PC 侧
    daemon 日志验证);行内进度/完成态正确。
- [ ] **D2 授权根**(PARKED 待设备)
  - 操作:本机页 → 添加授权根(OPEN_DOCUMENT_TREE)→ 选大目录 → 整目录发送;
    再移除该根。
  - 预期:授权根内可浏览/发送(树枚举 App 侧完成);持久授权重启后仍在;
    移除后消失;超 512 条目被拒(词面「条目过多请分批」)。
- [ ] **D3 分享入口**(PARKED 待设备)
  - 操作:系统相册/文件管理器 → 分享到 MTDSS → 选设备 → 发送。
  - 预期:PC 收到;文案面注明「分享件为一次性,中断后重发 = 源已不存在」
    (ACTION_SEND 临时授权,评审裁定③)。
- [ ] **D4 KEEP_SCREEN_ON**(PARKED 待设备)
  - 前置:D1/D2 传输进行中。
  - 操作:放置不动屏幕(勿按电源键)。
  - 预期:屏幕保持常亮;传输完成/失败/取消后恢复可自动熄屏。

### E. 任务列表与断点续传(T11;PARKED 待设备)

- [ ] **E1 双向任务行**(PARKED 待设备)
  - 前置:B2。
  - 操作:PC 发 MIX 3、MIX 3 发 PC 各一任务。
  - 预期:任务列表行/进度/操作(暂停/恢复/取消/清除/重新发起)与 golden
    字段一致;接收行(会话级)与历史行(持久)同列。
- [ ] **E2 熄屏中断 → 手动续传(headline)**(PARKED 待设备)
  - 前置:大文件(≥1GB)传输中。
  - 操作:按电源键熄屏 → 等任务断(后台死活随系统,§3.1)→ 亮屏回 App →
    任务列「中断」态 → 点「重新发起」。
  - 预期:从 bitmap 断点续传(PC 侧 daemon 日志验证跳过已收块)→ done;
    断点前后文件内容逐字节一致。
- [ ] **E3 分享件重发退化**(PARKED 待设备)
  - 前置:D3 的分享件任务已中断。
  - 操作:该行点「重新发起」。
  - 预期:走「源已不存在」路径文案(一次性授权的预期退化,非缺陷)。
- [ ] **E4 清除语义**(PARKED 待设备)
  - 前置:E1 的发送任务在列(对端已收/或取消)。
  - 操作:点「清除」。
  - 预期:发送侧残留 `.mtdss` 清净(需对端在线回执;离线时清除保留记录并
    提示);接收任务清除本地行。
- [ ] **E5 同签名在途防连点**(PARKED 待设备)
  - 操作:中断行快速连点「重新发起」。
  - 预期:签名级防连点(不双发);双终态行双发窗口闭合(611de9b 修复面)。

### F. 检查更新全链(T12;PARKED 待设备)

- [ ] **F1 未配置 fail-closed**(PARKED 待设备)
  - 前置:不注入更新源的 debug 构建。
  - 操作:设置 → 检查更新区。
  - 预期:呈「未配置更新源(构建期未注入更新基址/公钥)」,无检查按钮;
    下载/重试均无效(系统零触网)。
- [ ] **F2 已是最新**(PARKED 待设备)
  - 前置:注入 `MTDSS_DL_BASE` + `MTDSS_UPDATE_PUBKEY` 的构建;更新源放
    **同版本** manifest(§1.2)。
  - 操作:点「检查更新」。
  - 预期:呈「已是最新版本(<当前>)」;源撤下 manifest(404)再查同样落
    已是最新(404 = 无更新口径),不落失败。
- [ ] **F3 升版本全链(headline)**(PARKED 待设备)
  - 前置:更新源放升版 manifest(目标版本 > 当前,`android-arm64` 资产 +
    实测 APK 的 sha256/size);MIX 3 已装低版本。
  - 操作:点「检查更新」→「下载并安装」→(可选:MIUI 未知来源授权)→ 系统安装器确认。
  - 预期:发现新版(版本号 + notes)→ 下载进度条(字节数增长,终值 = 清单
    size)→ SHA-256/字节数复核通过 → 系统安装器自动拉起 → 用户确认安装 →
    装新版本起,登录态/收件箱/授权根保留(同签名覆盖装)。
- [ ] **F4 坏签名拒绝**(PARKED 待设备)
  - 前置:更新源放**错误私钥签**的 manifest。
  - 操作:点「检查更新」。
  - 预期:「清单验签失败」,不进下载;改一字节(如 notes)同形拒绝(与 M4
    F2 同源语义)。
- [ ] **F5 下载中断重试**(PARKED 待设备)
  - 前置:F3 下载进行中。
  - 操作:断网/关更新源 → 下载失败 → 恢复 → 点「重试」。
  - 预期:失败文案(下载失败: …);无 `.part` 残留;重试**免重查直接重下**
    (M4 清单缓存口径);恢复后一次成功到安装器。
- [ ] **F6 MIUI 未知来源授权**(PARKED 待设备)
  - 前置:MIUI 12+(实测环境为准)。
  - 操作:F3 首次拉起安装器。
  - 预期:系统弹「未知来源/安装未知应用」授权(MIUI 可能双层:原生 + 安全
    中心);授予后安装器继续;拒绝后再点「重新拉起安装器」可重走。
- [ ] **F7 安装器取消后重拉**(PARKED 待设备)
  - 操作:F3 安装器弹窗点取消。
  - 预期:设置页仍呈「安装器已拉起」态;「重新拉起安装器」按钮可再次拉起;
    无重复下载。

### G. 退出与离线(T7/T9;PARKED 待设备)

- [ ] **G1 退出即离线**(PARKED 待设备)
  - 前置:B2 双端在线。
  - 操作:MIX 3 正常退出 App(最近任务划走,或设置内退出登录后杀进程)。
  - 预期:PC 侧设备列表 MIX 3 立即(数秒内,信令心跳口径)转离线;引擎
    PDEATHSIG 随亡(A4 同源)。

### H. 非实机项(本任务已执行,勾选即现状)

- [x] **H1 JVM 单测全绿**:`./gradlew testDebugUnitTest` 198/198(含 T12 新增
  47:清单核验 13 / 编排器 25 / 生产 HTTP 栈真跑 6 / 设置 VM 3)。
- [x] **H2 assembleDebug 绿**:APK 43.5MB 含 libmtdssd.so 15.7MB;FileProvider
  入 merged manifest(authority `com.mtdss.android.fileprovider`);BuildConfig
  更新源字段空值缺省 + `-P` 注入实测生效。
- [x] **H3 assembleRelease(未签名)绿**:签名属性未注入 → `app-release-unsigned.apk`
  35.3MB(fail-visible 路径);真实 keystore 发布待用户生成(§J)。
- [x] **H4 16KB 对齐检查**:引擎 ELF 全 LOAD 段 Align 0x4000(llvm-readelf);
  debug/release APK `zipalign -c -P 16 4` 均通过(当前压缩存档形态;检查命令
  与口径见 `scripts/android/README.md` §4)。
- [ ] **H5 CI android job 绿**:yaml 交付 + 本机 YAML 语法校验通过(`act` 不
  可用,静态审 + 首次 push 观察为准);门禁 = push→main & PR 串联(与 test /
  nat-matrix / ui 同 `on:` 面)。**首次运行观察点**:Tencent 镜像拉 Gradle
  发行、NDK 30.0.16248370 sdkmanager 安装、cargo-ndk 4.1.2 安装。

### I. 发布演练(直发渠道;PARKED 待 keystore/VPS 择时)

- [ ] **I1 keystore 生成与签名发布**(PARKED):离线机 keytool 生成(命令见
  scripts/android/README.md §3)→ 四项属性注入 → `assembleRelease` → 已签名
  `app-release.apk`;keystore 离线保管(与 ed25519 私钥同规格管理)。
- [ ] **I2 dl 路径演练**(PARKED):按 scripts/android/README.md §5 手工发布
  一版升版 manifest 到 VPS `/dl/android/`(或本机静态服务器)→ 真机走 F3 全链。

### 记录格式

每项:`编号 | 通过/失败 | 现象一句话 | (失败时)logcat/截图路径`。日志:
`adb logcat -s MtdssEngine mtdssd`、引擎侧 stderr(已转 logcat)、任务列表
截图。全部通过后回填本清单勾选 + spec 状态行(controller 收官时)。

## 3. 故障排查速查(P3 增量)

| 现象 | 先查 |
|---|---|
| 装后闪退 | `adb logcat -s AndroidRuntime`;ABI(arm64-v8a 限定)/ minSdk 29 |
| 「连接引擎中」不收敛 | logcat MtdssEngine:拉起失败 = nativeLibraryDir 下无 libmtdssd.so(检查 extractNativeLibs);MTDSS_HOME 权限 |
| 登录收不到码 | server 地址/CA(备案前自签 https 需设备导入 CA);信令可达性(`curl -s -X POST https://…/auth/request-code …`) |
| 收件箱不见文件 | 文件管理器是否看 Downloads;MediaStore IS_PENDING(传输中半成品不可见属预期) |
| 下载到收件箱失败 | PC 在线;`dir="inbox"` 哨兵仅安卓 daemon 认(桌面 daemon 回 INVALID_PATH) |
| 发送大目录被拒 | 条目 > 512(BridgeLimits,分批);授权根是否还在(重启后持久授权) |
| 断点续传不续 | 任务行是否「中断」态(重新发起仅 failed/interrupted 亮);分享件 = 一次性(预期退化) |
| 检查更新呈「未配置更新源」 | 构建是否注入 MTDSS_DL_BASE/MTDSS_UPDATE_PUBKEY(BuildConfig 直读);公钥须 64 位 hex |
| 清单验签失败 | manifest 是否用 update-priv.hex 签、公钥是否同对;asset url 改动不影响(签名域不含 signature 但含 url——改 url 必须重签) |
| 安装器不弹 | MIUI 未知来源授权;「重新拉起安装器」;`adb shell dumpsys package com.mtdss.android | grep -A2 uri` |
