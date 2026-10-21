# M4 冒烟测试手册

配套 `docs/release-checklist.md`「M4 手动冒烟」使用;本文覆盖 M4b 增量面与逐项测试判定标准,并附**发布演练底稿**(§3 五步,公网真机更新 §I 是其中第④步的验收面)。M4a 已验的托盘唤起/退出/同设备粘贴冲突/CA 选择器/CSP 不重复(见 checklist「M4a 手动冒烟」段)。

M4b = 打包与更新:NSIS per-user 安装器(装/卸/升级/静默)、更新全链(托盘「检查更新…」/UI 更新页/CLI `updatecheck`·`update` 三入口 → 下载验签 → 静默安装 → 重启版本翻新)、三档开关、空闲门、版本钉扎回滚、tar 平台、发布流水线(tag `v*` 驱动)。

## 0. 当前已支持的能力边界

| 能力 | 状态 |
|---|---|
| NSIS per-user 安装器 `MTDSS_<ver>_x64-setup.exe`(installMode currentUser,**免 UAC**;桌面+开始菜单快捷方式;静默 `/S`) | ✅ M4 新增 |
| 安装器生命周期钩子:装前轮询 ≤15s 等 mtdssd/ui 退出(taskkill /F 兜底)/ 装后拉起 mtdssd / 卸前停进程+清自启 Run 键 / 卸后**保留** MTDSS_HOME | ✅ M4 新增 |
| 更新全链(仅 Windows):检查 → ed25519 验签 → SHA-256 复核下载 → Installing 序列(标记+`/S` 静默安装)→ 重启翻新 +「已更新到 X」通知 | ✅ M4 新增 |
| 托盘「检查更新…」(强制检查,三分支通知,有新版拉起更新窗)与「设置 ▸ 自动更新/自动安装」三档 | ✅ M4 新增 |
| UI 更新页覆盖层(`--update-page`;当前/最新/notes/进度/立即安装/**安装指定版本**回滚入口) | ✅ M4 新增 |
| CLI `mtdss updatecheck` / `mtdss update [version]`(y/N 确认梯;空闲门硬拒) | ✅ M4 新增 |
| 空闲门:有传输作业(**含已暂停**)拒绝安装 → `pending_install`,作业终态/下轮定时自动重试 | ✅ M4 新增 |
| 版本钉扎:定时/开机检查拉 `{base}/win/{当前版}/manifest.json`(404=无更新常态);`mtdss update <旧版>` 降级回滚(M4 无防回滚) | ✅ M4 新增 |
| 失败注入面:断网/坏签名清单/半装态标记,各入口守卫(cli exit 2 / ui-bridge warn / uilaunch 拒) | ✅ M4 新增 |
| tar 平台(x86_64-linux / aarch64-macos)解包即用;**无自动更新能力**(更新入口报「M4 更新仅支持 Windows」) | ✅ M4 新增 |
| 发布流水线:tag `v*` 三平台 + `mtdss-sign` 签名清单 + 可选 VPS 上传 + GH Release | ✅ M4 新增 |
| 设备重命名、跨会话断点续传、tar 平台 GUI/托盘 | ❌ 后置(V1/V2,沿 m3b/m3c 边界) |
| 自动更新的防降级保护(回滚是有意能力)、非 Windows 平台更新 | ❌ M4 设计裁定 |

## 1. 环境与准备

### 1.1 环境矩阵

| 环境 | 覆盖测试项 | 准备要点 |
|---|---|---|
| Windows 桌面(物理机/常规 VM,有托盘) | §A–§G、§I | 安装器 + 构建期公钥注入(见 1.3) |
| VPS(公网 Ubuntu) | §B–§F 更新源、§I、§3 演练①②④ | `scripts/deploy` 部署(见 §3 第②步;回环演练可用本地静态服务器替代) |
| Linux x86_64(真机/无头) | §H1/H2 | GH Release tar 或本地 `package-tar.sh` 产物 |
| macOS Apple Silicon | §H3 | 同上(aarch64-macos tar) |

### 1.2 更新源目录形态(本机回环演练最小形)

任意静态文件服务器伺服以下布局(与 `scripts/deploy/README.md`「更新发布目录规约」一致;本地可 `python -m http.server 8000` 起根目录):

```
<root>/dl/
├── manifest.json                      # 最新版签名清单(force 检查读这里)
└── win/
    ├── 1.0.0/MTDSS_1.0.0_x64-setup.exe
    └── 1.0.1/MTDSS_1.0.1_x64-setup.exe
```

- daemon 运行时 env `MTDSS_UPDATE_URL` 指到 `<root>/dl`(回环演练如 `http://127.0.0.1:8000/dl`);`MTDSS_UPDATE_CA` 仅自签 https 需要(回环 http 可省)。
- manifest 用 `mtdss-sign manifest --version <ver> --url-base <同基址> --notes-file <txt> --file <setup.exe> --out unsigned.json` 生成 → `mtdss-sign sign --key <priv.hex>` → `mtdss-sign verify --pubkey <pub.hex>` 后放位(命令细节见 §3 第①/⑤步)。
- **钉扎位**(测 §C 自动档时才需要):把新版 manifest 复制一份到 `win/<旧版>/manifest.json`——定时/开机检查拉的是 `{base}/win/{当前版}/manifest.json`,404 即判「无更新」(常态)。

### 1.3 客户端构建(演练用安装器,须带公钥)

更新验签公钥是**构建期**注入(`option_env! MTDSS_UPDATE_PUBKEY`,hex;缺失/非法 → 更新 fail-closed「更新未配置」):

```powershell
$env:MTDSS_UPDATE_PUBKEY = "<update-pub.hex 内容>";   # 见 §3 第①步 keygen
cargo build --release -p mtdss-cli -p mtdss-daemon -p mtdss-sign
cd ui; npm ci; npx tauri build
```

产物:`ui/src-tauri/target/release/bundle/nsis/MTDSS_1.0.0_x64-setup.exe`(bundle resources 打入同目录三件套 ui.exe/mtdss.exe/mtdssd.exe;改版本号再构建即得 1.0.1 件)。

1. 判定基线:以下各步骤的「判定」均以 §1 环境为前提;失败先查 daemon `--log-file`、CLI 终端输出与 ui 桥日志。

## 2. 测试项清单(逐项判定标准)

> 建议顺序执行;每项记录 通过/失败 + 现象一句话;失败时附日志与截图。§B 起需要 §1.2 更新源在线。

### A. 安装/卸载/快捷方式/升级安装(NSIS)

- **A1 全新安装(per-user 免 UAC)**:双击 `MTDSS_1.0.0_x64-setup.exe` 走完向导。判定:**全程无 UAC 弹窗**(currentUser 安装);安装目录(向导所示默认路径)内 `ui.exe`/`mtdss.exe`/`mtdssd.exe` 三件套**同目录**;安装器收尾后托盘图标自动出现(POSTINSTALL 拉 mtdssd),`mtdss status` 正常。
- **A2 快捷方式**:判定:桌面与开始菜单各出现 MTDSS 快捷方式,双击可拉起主窗。
- **A3 覆盖/升级安装(进程在跑,PREINSTALL 路径)**:保持 mtdssd+ui 运行,再跑一遍安装器(同版或改版重装均可)。判定:安装页出现「Waiting for MTDSS processes to exit (up to 15s)...」——进程自然退出则放行;15s 未退则「Timeout ... force killing」强杀后继续;安装完成后三件套就位、托盘回归(强杀的 mtdssd 被 POSTINSTALL 重新拉起)。
- **A4 静默安装 `/S`**:先卸载(A5),cmd 执行 `MTDSS_1.0.0_x64-setup.exe /S`。判定:无界面静默完成(退出码 0),快捷方式/三件套/托盘回归与 A1 一致。
- **A5 卸载**:先托盘勾「设置 ▸ 开机自启」(让 HKCU Run 键在位)→ 系统设置/控制面板卸载 MTDSS。判定:卸载即停 mtdssd/ui 进程;安装目录移除;`reg query HKCU\Software\Microsoft\Windows\CurrentVersion\Run /v MTDSS` 报找不到(PREUNINSTALL 清键);`%USERPROFILE%\.mtdss` **仍在**(POSTUNINSTALL 留空,数据保留,重装后登录态/设置还在)。
- **A6 首装 WebView2 引导(可选,仅无 WebView2 的干净系统)**:判定:安装器经 downloadBootstrapper 联网装 WebView2 后继续完成安装(有网环境;已装 WebView2 的机器无此步)。

### B. 更新全链(手动检查 → 确认 → 下载 → 安装 → 重启 → 版本翻新)

> 前提:客户端 1.0.0(§1.3 带公钥构建);更新源在位 1.0.1 manifest + setup(§1.2)。当前版本口径以 `mtdss status`/托盘为准。

- **B1 CLI 检查**:`mtdss updatecheck`。判定:输出「当前 1.0.0 / 最新 1.0.1」+「有更新可用,运行 mtdss update 安装」,退出码 0;负例:撤下更新源 `manifest.json` 再查 →「当前 X,已是最新」(force 路径 404=无更新),退出码仍 0。
- **B2 CLI 确认梯**:`mtdss update`。判定:先打「目标版本:1.0.1」+ notes 首行「更新说明:…」+「确认更新? [y/N]」;答 N(或直接回车/EOF)→「已取消」,退出码 0,系统零改动。
- **B3 CLI 全链安装**:B2 答 y。判定:下载落地 `{home}\downloads\mtdss-setup-1.0.1.exe`(.part 临时名 → 就位);随后「已开始安装,守护进程退出」退出码 0;安装器静默接管 → daemon 重启后弹系统通知「MTDSS 已更新 / 已更新到 1.0.1」;`mtdss status` 显示 1.0.1;**更新前若 UI 主窗开着** → 更新退出时 UI 同闭、新版 daemon 起来后主窗被唤回(restart-ui 标记;仅 CLI 在场时无窗可唤,不判失败)。
- **B4 UI 更新页链**:托盘右键「检查更新…」。判定:系统通知「MTDSS 更新可用 / 发现新版本 1.0.1」+ 更新窗自动拉起(`--update-page` 覆盖层,独立于登录态);页内当前/最新版本、notes、下载进度条齐全;点「立即安装」→ 状态行转 installing → UI 关闭 → 重启后版本翻新、主窗唤回。
- **B5 托盘检查三分支补全**:已最新态(撤下 1.0.1 产物)点「检查更新…」→ 通知「已是最新版本」且**不**拉更新窗;停掉更新源再点 → 通知「检查更新失败」。判定:三分支文案与「失败细节只进日志」口径正确。
- **B6 未配置 fail-closed(负例)**:用**未注入** `MTDSS_UPDATE_PUBKEY` 构建的 daemon → `mtdss updatecheck`。判定:「更新未配置(缺少验签公钥)」退出码 1;托盘检查 →「检查更新失败」;检查不翻状态机(不落 failed 常驻)。

### C. 自动安装静默档(空闲门生效)

> 前提:钉扎位在位——`win/1.0.0/manifest.json` 放 **1.0.1** 的清单(§1.2);自动检查默认已开(D8),本节开「自动安装」。

- **C1 开自动安装**:托盘 设置 ▸ 自动安装 勾上。判定:勾选落勾(设置持久化);此项仅在自动更新开启时可勾(D4 父子耦合)。
- **C2 静默直装**:重启 daemon(定时检查开机首拍即查钉扎位)。判定:**无任何确认交互**,发现 1.0.1 → 直接进下载/安装序列(不弹「更新可用」提醒通知)→ 版本翻新 +「已更新到 1.0.1」通知;全程无需人工。
- **C3 空闲门生效(自动档)**:先造一个大文件传输并**暂停**(托盘 tooltip/传输面板确认「1 个传输中」)→ 重启 daemon 触发钉扎检查。判定:检查发现新版但**不下载不装**(空闲门在下载前拒绝,系统零改动),转 pending_install;恢复/取消该作业(终态)后 daemon **自动重试**完成安装——终态后无需任何操作。

### D. 回滚 `mtdss update <旧版>`(版本钉扎)

> 前提:当前 1.0.1;`win/1.0.0/` 下有 1.0.0 的 setup **与** manifest(旧版钉扎清单;发布演练 §3 第⑤步含手工生成法)。

- **D1 降级回滚**:`mtdss update 1.0.0` → 确认 y。判定:M4 无防回滚,降级被接受:拉 `win/1.0.0/manifest.json` 验签 → 下载 1.0.0 setup → 安装 → 「已更新到 1.0.0」通知,`mtdss status` 回 1.0.0,数据/设置保留。
- **D2 版本形态白名单**:`mtdss update 1.0`(两段)。判定:「版本非法(须为严格 X.Y.Z): 1.0」退出码 1(顺带封路径注入)。

### E. 空闲门手动档(paused 也拒)

- **E1 硬拒**:造 1 个传输并暂停 → `mtdss update` → y。判定:**确认后仍被拒**,stderr 原样「1 个传输中(含已暂停),可先取消作业后重试」退出码 1;状态转 pending_install(UI 更新页仍显示可安装),不下载零改动。多个作业(运行中+暂停混合)时 N 为总数,文案同形。
- **E2 终态自动续装**:E1 后取消/完成该作业。判定:daemon 监听作业终态事件自动重试 pending_install,下载安装完成——**无需再确认**(重试走 daemon 内部路径,CLI 早已退出)。

### F. 失败注入

- **F1 下载中断(断网时机)**:`mtdss update` → y → 状态进 downloading(UI 更新页进度条在走)时**断网**。判定:下载请求失败落 `failed`(文案含「下载失败/下载中断」)+ `event.update` 广播;`{home}\downloads\` **无 `.part` 残留**;系统零改动(标记未写=未进 Installing);恢复网络后 `mtdss update` 重试成功(清单缓存保留,无需重新检查)。
- **F2 清单篡改一字节**:改更新源 `manifest.json` 任一字节(notes 亦可)→ `mtdss updatecheck`。判定:「清单验签失败」failed,拒绝进入下载;还原文件后恢复。负例补充:`win/1.0.1/` 的 setup 改一字节 → 下载后 SHA-256 复核失败「下载复核失败」,同样零改动。
- **F3 半装态模拟(手写 `{home}\update-in-progress` 标记,内容随意如 `1.0.1`)——各入口守卫逐一验**:
  - **F3a CLI(需拉起路径)**:先停掉 mtdssd(托盘「退出」)→ `mtdss status`。判定:退出码 **2**、stderr 恰「更新进行中,请稍候」,**不自动拉起** daemon(daemon 在场时不拦,命令照常——对照)。
  - **F3b UI 桥**:同态下启动 `ui.exe`。判定:ui 桥日志 warn「拉起 daemon 失败: 更新进行中,请稍候」,UI 停留在离线重试节奏,不拉新 daemon。
  - **F3c 托盘唤起**:同态点托盘「打开主窗口」。判定:不拉新窗,daemon 日志「更新进行中,暂不唤起主窗口(安装器即将接管)」。
  - **F3d mtdssd 直接拉起(开机消费)**:同态改标记内容为**当前版本** `1.0.0` → 启动 mtdssd。判定:IPC 就绪后标记被消费——系统通知「MTDSS 已更新 / 已更新到 1.0.0」且标记文件删除;再把内容写成 `9.9.9`(不等版本)重启 mtdssd → 无通知,日志「上次更新未完成(目标 9.9.9,当前 1.0.0),已清除标记」+ 标记删除;**叠加手写 `{home}\restart-ui` 空文件**再重启 → 标记清除且主窗被唤回。三种形态均不拦启动。

### G. 三档开关(托盘 设置 ▸ 自动更新 / 自动安装)

- **G1 默认档**:全新 `MTDSS_HOME` 首启。判定:自动更新=勾、自动安装=未勾且**禁用**(D8:检查默认开/安装默认关)。
- **G2 关检查连带关安装**:点掉「自动更新」。判定:「自动安装」连带去勾+禁用(三档退化纯手动);重启 daemon 后勾选态保持(持久化)。
- **G3 提醒档(检查开+安装关)**:更新源放新版 → 重启 daemon 触发定时检查。判定:系统通知「MTDSS 更新可用 / 新版本 X 已发布,可在更新设置中安装」+ UI toast(「查看」直达更新页);**不自动安装**,等用户手动。
- **G4 静默档(检查开+安装开)**:即 §C2,判词从简:发现即静默直装(仍守空闲门)。

### H. tar 平台(Linux / macOS)

- **H1 Linux 解包可用**:`tar xzf mtdss-<ver>-x86_64-linux.tar.gz` → `./mtdss --help` 正常、`./mtdssd` 可后台跑、`mtdss login/devices` 与 Windows 端互通(tar 内 README.txt 有解压即用说明)。判定:CLI/daemon 全功能可用(除更新)。
- **H2 更新入口报不支持**:同机 `mtdss updatecheck`。判定:「M4 更新仅支持 Windows」退出码 1(`data.kind=UNSUPPORTED_PLATFORM`;无托盘面故无托盘更新项)。
- **H3 macOS(Apple Silicon)**:`mtdss-<ver>-aarch64-macos.tar.gz` 重复 H1/H2。判定:同 H1/H2。

### I. 公网真机更新一例

> 前提:§3 演练①②③④完成(VPS 更新分发在位);至少一台**公网环境** Windows 真机装 1.0.0(构建注入同一公钥;登录 `mtdss login --server https://<VPS_IP> --ca ca.crt`;daemon 侧用户级环境变量 `MTDSS_UPDATE_URL=https://<VPS_IP>/dl` + `MTDSS_UPDATE_CA=<路径>\ca.crt`)。

- **I1 真机全链**:真机托盘「检查更新…」→ 确认安装(或 CLI)。判定:经公网 https(自签 CA 信任)拉清单验签、下载 setup、静默安装 → 1.0.0→1.0.1 版本翻新 +「已更新到 1.0.1」通知 + UI 唤回;`mtdss devices` 在 VPS 拓扑下全程在线。回环拓扑不算数——本项验收的就是真实网络形态。

### 记录格式

每项:`编号 | 通过/失败 | 现象一句话 | (失败时)日志/截图路径`。日志:daemon `--log-file`、CLI 终端输出、ui 桥日志、安装器截图(§A)。全部通过后回填 `docs/release-checklist.md`「M4 手动冒烟」段勾选。

## 3. 发布演练底稿(五步;真 tag/真 VPS/secrets 由用户择时执行,不在分支门禁内)

**Sequencing 警示(Task 18 裁定,先读)**:

1. **workflow_dispatch 干跑要在 PR 合并 main 之后**——GitHub 只在默认分支的 workflow 上暴露 dispatch 入口,分支期间 Actions 列表看不到 release workflow。
2. **首个真 tag 前必配 vars.MTDSS_DL_BASE**(或逐次 dispatch 入参 `dl_base`)——三级回退末位是 `https://mtdss-dl.invalid/dl`(RFC 2606 保留域):忘配不报错,但 manifest 内资产 URL 天然不可达,fail-closed 等于白发。
3. 近同时打两个 tag 无并发组兜底,可能竞态——**依次发布**,上一个 publish 完再打下一个。

### ① 密钥与 GH 配置

1. 离线机 `mtdss-sign keygen --out <离线目录>` → 产 `update-pub.hex` + `update-priv.hex`(同路径已存在则拒,防误覆盖);**私钥离线保管**,不入仓不进 CI。
2. 公钥 hex 提交入仓 `keys/update-pub.hex`(备案复核用;构建注入以 GH vars 为准)。
3. GH 仓库配置清单:

| 名 | 类 | 值 | 缺失后果 |
|---|---|---|---|
| `MTDSS_SIGNING_KEY` | secret | update-priv.hex 内容(hex) | build-windows 显式报错,流水线红 |
| `MTDSS_UPDATE_PUBKEY` | vars | update-pub.hex 内容(hex) | 同上显式报错;且 daemon 构建无钥=fail-closed |
| `MTDSS_DL_BASE` | vars | 如 `https://<VPS_IP>/dl` | manifest URL 落 `.invalid`(见警示 2) |
| `MTDSS_DEPLOY_KEY` / `MTDSS_DEPLOY_HOST` / `MTDSS_DEPLOY_USER` | secrets | VPS SSH 私钥/地址/用户 | **可缺省**:publish 跳过 VPS 上传,仅发 GH Release(演练不失败) |

### ② VPS 部署(scripts/deploy)

沿 `scripts/deploy/README.md`「首次部署」七步执行:构建上传 `signaling-server`/`stun-server` → `useradd -r mtdss` → 拷两个 unit 与 Caddyfile(**`mtdss-signaling.service` 内 `<公网IP>` 占位必须替换**,Task 17 备忘)→ 证书:域名未就绪走自签 `signaling-server --dev-gen-tls /etc/mtdss/tls --tls-san <VPS_IP>`(产 ca.crt/server.crt/server.key;`--tls-san` 可重复,生成的 server.crt 已含 localhost/127.0.0.1/::1)→ 防火墙放行 TCP 443 + UDP 3478/3479(8787 仅回环,**严禁对外开放**)→ `systemctl daemon-reload && systemctl enable --now mtdss-stun mtdss-signaling caddy` → 回环 curl 验证 200。再 `mkdir -p /var/www/mtdss/dl/win/1.0.0 /var/www/mtdss/dl/win/1.0.1` 预建产物位。多宿主 VPS 的 STUN `--bind` 告诫见 checklist「STUN / 部署」。域名就绪后的切换/回退照 README「域名就绪切换步骤与回退」。

### ③ 发 / 装 v1.0.0

1. 确认四处版本文件(根 `Cargo.toml` / `ui/src-tauri/tauri.conf.json` / `ui/src-tauri/Cargo.toml` / `ui/package.json`)均为 1.0.0 → 打附注 tag `v1.0.0`(tag message 即清单 notes)并推送。
2. workflow 三平台构建 + `mtdss-sign` 签名验签 + VPS 上传(`manifest.json` → `/dl/manifest.json`;setup → `/dl/win/1.0.0/`)+ GH Release。无 deploy secrets 的演练形态:从 GH Release 手工下载后按「更新发布目录规约」上传(放前 `mtdss-sign verify` 复核)。
3. Windows 真机按 §A 安装 1.0.0,按 §I 前提配 server/CA/更新 env;`mtdss updatecheck` 应报「已是最新」(1.0.1 未发,钉扎 404 + force 清单 version=1.0.0 不大于当前)。

### ④ 真机 1.0.0 → 1.0.1

1. 四处版本翻 1.0.1 → tag `v1.0.1`(依次发布,见警示 3)→ workflow 产物上 VPS(`/dl/manifest.json` 与 `/dl/win/1.0.1/`)。
2. 测 §C 自动档才需要:把 1.0.1 manifest 复制一份为 `/dl/win/1.0.0/manifest.json`(钉扎位)。
3. 真机触发全链:托盘「检查更新…」/ UI 更新页 / CLI `updatecheck`+`update` 任一入口 → 按 §B/§I 判定:版本翻新 + 通知 + UI 唤回。

### ⑤ 回滚

1. 旧版钉扎位 `/dl/win/1.0.0/` 需有 **1.0.0 的 manifest**(workflow 只上传根 manifest 与 setup,per-version 清单手工生成):
   `mtdss-sign manifest --version 1.0.0 --url-base <base> --notes-file <txt> --file MTDSS_1.0.0_x64-setup.exe --out unsigned.json` → `sign`(私钥)→ `verify`(公钥)→ 上传 `/dl/win/1.0.0/manifest.json`(setup 已在位)。
2. 真机 `mtdss update 1.0.0` → y → 按 §D 判定:降级被接受、版本回 1.0.0、数据保留。
3. 服务端回退(证书/域名形态切换出问题时):恢复 `/etc/mtdss/tls/` 旧证书 + `systemctl reload caddy`;客户端基址回退沿 README「回退」节。

## 4. 故障排查速查(M4 增量)

| 现象 | 先查 |
|---|---|
| 「更新未配置(缺少验签公钥)」 | 构建命令上是否真带了 `MTDSS_UPDATE_PUBKEY`(构建期 option_env!;装旧包/漏 env 都会中) |
| 有新版却查不出(无更新) | 两路 URL 分工:托盘/CLI/UI 是 **force** 读 `/dl/manifest.json`;定时/开机是**钉扎**读 `/dl/win/<当前版>/manifest.json`(404=无更新常态)——文件放没放对位 |
| 下载失败/超时 | `MTDSS_UPDATE_URL` 可达性;自签 https 时 `MTDSS_UPDATE_CA` 是否指对 ca.crt;manifest 内 `url_base` 与实际伺服基址是否一致 |
| 安装器卡 15 秒才继续 | 即 PREINSTALL 超时兜底(mtdssd/ui 未自行退出被强杀)——属预期路径,装完核对托盘回归(A3) |
| 更新后版本没翻新 | `{home}` 下 `update-in-progress` 残留?(开机消费会自愈清除)安装器 `/S` 是否跑完;日志「上次更新未完成」即半装态被消费 |
| CLI 退出码 2「更新进行中,请稍候」 | 标记在场且 daemon 不在场的拉起守卫——等安装收尾;误留标记直接启动 mtdssd 消费即清(F3d) |
| 自动档从不触发 | 钉扎位 manifest 是否放了;「设置 ▸ 自动更新/自动安装」两档是否都开(24h 周期,重启 daemon 即查) |
| dispatch 干跑找不到入口 | PR 未合并 main(警示 1);合并后再看 Actions |
| tag 推上去流水线红 | check job 版本一致性:tag 名(去 v)≠ 四处版本;或 secrets/vars 未配(§3① 表) |
