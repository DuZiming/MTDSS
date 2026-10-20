# MTDSS M4(发布与更新)设计

- 日期:2026-10-20
- 状态:待用户批准(brainstorm 定稿口径见 §6 决策记录)
- 上游:PR #8 债序、`docs/smoke-m3b.md` §E、`docs/release-checklist.md`「M3c 后置项」、项目记忆 M4 待办

## 1. 意图与为什么

M1–M3c 已收官(功能面齐、公网真机冒烟过)。M4 把「能跑的工程」变成「能交付的商业产品」:清掉终审判定的债,打可信的包,建可运营的更新通道与服务端统一入口。收官形态:用户双击 Windows 安装包,装完即用(桌面/开始菜单入口、托盘、双击打开);Linux/macOS 发 tar(cli+daemon);后续版本经 daemon 主导的签名更新通道下发。

## 2. 目标

1. **打包前必须项收口**:SDDL 跨用户 §E 手工验证(smoke-m3b.md E1–E4,纯人工);托盘唤起 D2/D3 在 release 布局下补验。
2. **清债**(顺序沿 PR #8 终审裁量 + checklist):CSP 硬化 → jobs 注册表终态保留上限 → 毒锁恢复/驱动监督 join → ctrl copy/move 冲突策略 → devices 单飞+类型化错误码 → ConfirmDialog 焦点陷阱 → spawn_blocking 清扫 → put `--to` 别名。
3. **Windows 安装包**:NSIS exe、per-user 安装、三件套(ui.exe + mtdss.exe + mtdssd.exe)、桌面/开始菜单快捷方式、支持静默安装。
4. **Linux/macOS tar**:mtdss + mtdssd,不带更新能力。
5. **更新体系(仅 Windows)**:daemon 全权编排(检查/下载/验签/执行安装器);ed25519 签名 manifest;UI 确认更新页 + 托盘三档设置;CLI `updatecheck` / `update [version]`(支持钉版本与回滚);空闲门(活跃传输时不装)。
6. **服务端**:Caddy 统一 443 入口(`/dl/*` 静态更新目录,其余反代信令);信令加 loopback 明文监听选项;部署资产(Caddyfile/systemd unit)入库;tag 驱动 release workflow 三平台构建→签名→发布到 VPS。
7. **客户端默认地址**:编译期常量(域名),`--server` 变可选;dev/测试可覆盖,测试永不触生产端点。

## 3. 非目标

- Linux/macOS 检测/自动更新(rpm/deb/dmg 时代再议)。
- macOS 公证、Windows 代码签名证书(未购;开放项 §10)。
- 设备重命名端点、真实邮件、Web 前端、邮箱验证、版本管理服务端(后续服务端迭代)。
- V2 项(文件重命名、跨会话续传、常用目录、按设备开放范围)。
- rpm/deb/dmg 打包本身;开机自启功能(仅沿 release-checklist 验证既有行为)。

## 4. 反目标(做了即失败)

- 沿用既有:数据不经系统资源管理器经手、对端无确认弹窗、UI 崩溃不杀传输、不绕过 daemon。
- 新增:
  - **UI 进程执行更新**——下载/验签/跑安装器只能是 daemon;UI/CLI 只是触发与展示入口。
  - **更新打断活跃传输**——空闲门对全部档位生效(含自动安装)。
  - **更新失败留半安装态**——验证全通过才 spawn 安装器;失败点系统未被改动。
  - **更新通道无验签**——不得只押 TLS;manifest 必须 ed25519 验签,公钥编译进客户端。
  - **测试触碰生产端点**——dev/测试一律本地静态服务或显式覆盖 URL。

## 5. 约束

- 门禁:`cargo test --workspace` + `cargo clippy --workspace --all-targets -- -D warnings` 双平台(本机 + WSL/CI)+ nat-matrix + ui job;每任务提交前全绿。
- 环境:cargo 走 rsproxy.cn;PROTOC/gh PATH 自救(项目记忆);UI 是独立 Tauri **v2** crate(自带 src-tauri/target,终门禁含 `npm run build` + vitest)。
- **现有解析序即发布布局**:`ipc::find_daemon_bin` 与 `daemon::uilaunch::find_ui_bin` 均以「exe 同目录」为第一候选——安装布局 = 三件套同目录,零代码适配,托盘唤起债随布局收口(剩验证)。
- 国内网络:VPS 境内;域名注册+ICP 备案进行中——备案完成前 dev/测试走 IP+非标端口/自签证书/本地服务,正式走域名+正式证书;Caddy 用 Let's Encrypt 或用户提供证书均可。
- CI:现有 ci.yml 仅 push→main 与 PR;release workflow 独立文件、tag `v*` 触发。
- 信令现形态:axum 裸 `0.0.0.0:8787` 自终止 TLS(rustls,证书必填,无明文模式);STUN 独立 UDP 3478/3479 不动。

## 6. 决策记录(brainstorm 定稿,2026-10-20)

| # | 决策 | 归属 |
|---|---|---|
| D1 | Windows = NSIS exe + per-user(免 UAC、可静默;MSI per-machine 留企业版后路) | 用户拍板 |
| D2 | 更新入口唯一 = daemon;UI/CLI 只是触发器,下载/验签/执行安装器全在 daemon | 用户拍板 |
| D3 | M4 更新只做 Windows;Linux/macOS tar 只发不更 | 用户拍板 |
| D4 | 托盘设置三档:自动更新关(纯手动)→ 开+自动安装关(每日检查+提醒+手动确认)→ 开+自动安装开(无提醒静默装,仍守空闲门);子开关随父开关置灰/启用 | 用户拍板 |
| D5 | 更新分发 = 境内 VPS,Caddy 统一 443,`/dl` 静态目录;流量大后挪对象存储(manifest 存完整 URL,客户端零改) | 用户拍板 |
| D6 | 自研统一更新器,不用 tauri-plugin-updater(它管不了 daemon 主导编排与 tar 平台,且引入第二套签名/清单体系) | Claude 建议 |
| D7 | 发布版本 1.0.0;工作区 version 与 tauri.conf.json 同步,CI 校验一致 | Claude 裁定 |
| D8 | 默认档 = 自动更新开、自动安装关 | Claude 裁定 |
| D9 | CLI 形状 = 子命令 `mtdss updatecheck`、`mtdss update [version]`(功能即用户所述 --updatecheck / --update;沿用既有子命令文法) | Claude 裁定 |
| D10 | manifest 验签后客户端才信任其中 URL/hash;私钥用户保管,GH secret 注入 CI;生产公钥入仓、构建期 env 可覆盖供测试 | Claude 裁定 |
| D11 | 安装契约:daemon 冲刷/标记/通知并等 UI 断开(≤5s)后才 spawn 安装器;NSIS 钩子装前轮询等待 mtdssd/ui 退出(≤15s,超时强杀)再复制;`update-in-progress` 标记(含目标版本)防半装态下拉起,新 daemon 按版本判定消费/清除 | Claude 裁定(builder check) |
| D12 | 空闲门口径:「活跃」= 未到终态的 job,含 paused 与 stalled;手动更新硬拒(文案指引先取消作业),M4 无越过开关 | Claude 裁定(builder check) |
| D13 | 三档落点:托盘「设置」子菜单两个可勾选项 + 托盘「检查更新…」拉起 UI 更新页(含手动检查);确认更新页承担版本/notes/进度/安装 | Claude 裁定(builder check) |
| D14 | 生命周期:安装钩子装完即启 mtdssd(新装/升级同);卸载杀进程 + 清 HKCU Run 键,保留 MTDSS_HOME 用户数据 | Claude 裁定(builder check) |
| D15 | 备案前演练 = VPS Caddy 443 自签(dev CA 生成器);updater TLS 信任走自定 CA 配置(与 login --ca 同型,无 accept-invalid);信令与更新两个独立编译期常量 + 独立 env 覆盖 | Claude 裁定(builder check) |

## 7. 架构

### 7.1 清债面(每项独立可测、独立提交;细节进计划)

1. **CSP 硬化**:`tauri.conf.json` `csp: null` → 严格策略(`default-src 'self'` 基线,Tauri v2 自动注入 nonce/hash);`npm run build` 后核对无 CSP 违规;vitest 回归。
2. **jobs 终态保留上限**:注册表仅保留最近 N(默认 100,可配)条终态 job,超出淘汰最旧;`transfer.list` 语义不变。
3. **毒锁恢复 / 驱动监督 join**:按 PR #8 债序定位修法(具体机制从 M3c 作业/编排代码考据,计划展开)。
4. **ctrl copy/move 冲突策略**:同设备 copy/move 接入与传输面同口径的 skip/overwrite 策略,清「device_copy 静默覆盖」债;CLI/UI 交互沿既有冲突面(ask/skip/overwrite)。
5. **devices 单飞 + 类型化错误码**:devices RPC 现拉独立化(清 M3b 冒烟发现的改名不可见问题的一半);错误码类型化替代 marker 串。
6. **ConfirmDialog 焦点陷阱**:模态焦点圈定(Tab 循环不出框、初始焦点、Esc/遮罩语义不变)。
7. **spawn_blocking 清扫**:接收端预扫、CLI 树扫等阻塞面 offload 到 spawn_blocking。
8. **put `--to` 别名**:`mtdss put … --to <dev:dir|本机dir>` 与 get/cp 对称统一。

### 7.2 打包

- **布局**:安装目录三件套同目录(ui.exe、mtdss.exe、mtdssd.exe)+ WebView2 依赖(downloadBootstrapper 模式)。
- **Windows**:tauri bundler NSIS(bundle > windows > nsis),`installMode = currentUser`,桌面+开始菜单快捷方式,`installerHooks` 后置钩子(见 7.3 拉起编排)。
- **Linux/macOS**:tar.gz(mtdss + mtdssd;**macOS 仅 aarch64**,用户定案 2026-10-20;内容布局与说明进计划)。
- **版本**:1.0.0 起步;根 `Cargo.toml` workspace version 与 `tauri.conf.json` 一致性由 release workflow 校验。
- **生命周期(D14)**:安装钩子装完即启动 mtdssd(新装与升级同——托盘立即可见,不待首次 UI/CLI);卸载:杀 mtdssd/ui 进程 + 清 HKCU 自启动 Run 键(防登录报错),**保留 MTDSS_HOME 用户数据**(卸载不清数据,业界惯例)。

### 7.3 更新体系(仅 Windows)

**manifest**(最新版 `/dl/manifest.json`;版本钉扎 `/dl/win/<ver>/manifest.json`):

```json
{
  "version": "1.0.1",
  "notes": "…",
  "pubdate": "2026-11-01T00:00:00Z",
  "platforms": {
    "windows-x86_64": {
      "url": "https://<domain>/dl/win/1.0.1/mtdss-1.0.1-x64-setup.exe",
      "sha256": "…",
      "size": 12345678
    }
  },
  "signature": "<ed25519,签其余域的 canonical JSON(键排序、无空白)>"
}
```

客户端剥 `signature` 域 → canonical 序列化 → ed25519 验签(公钥编译进 daemon,生产公钥入仓;构建期 env 覆盖供测试)→ 才信任其中 URL 与 hash → 下载后 sha256 复核。签名助手 = 仓内小工具(工作区新成员 `mtdss-sign`,ed25519,双平台可跑);私钥用户保管,CI 从 GH secret 注入。

**daemon 更新状态机**:

```
Idle → Checked(启动+每24h;auto_check)→ Downloading(进度事件)
     → Verified → Applying | PendingInstall(空闲门:「活跃」= 未到终态
        job,含 paused/stalled;job 全终态事件或下轮检查时重试)→ Installing → (新版本起)
                                                  ↘ Failed(可重试;系统未被改动)
```

- **拉起编排(安装契约,D11)**:Installing 时 daemon 依次完成:jobs 注册表冲刷 → 写 `restart-ui` 标记(仅当触发时有 UI 连接)→ 写 `update-in-progress` 标记(含目标版本)→ 广播 `event.update`(applying)→ 等 UI 断开(≤5s,超时不候)→ detached spawn `setup.exe /S`(per-user 免 UAC)→ daemon 退出。NSIS 钩子装前**轮询等待** mtdssd/ui 进程退出(≤15s,超时 taskkill 强杀)再复制文件——「优雅」指 spawn 前的冲刷与通知;强杀只作超时兜底,空闲门已保证此时无传输可杀。装后钩子启动 mtdssd(托盘回归);新 daemon 启动按 `update-in-progress` 版本判定:运行版本 = 目标 → 更新成功清标记,不等 → 失败残留 warn 后清除。UI 挂死(模态卡住)→ 5s 超时后由安装器 15s 窗收尾;用户在复制中途拉起 ui/cli → spawn 路径检查 `update-in-progress` 标记,提示「更新进行中」不拉起(防在半覆盖目录里起进程)。
- **回滚**:`mtdss update <旧版>` 走版本钉扎 manifest;M4 允许降级(无防回滚需求)。
- **IPC 面(新增)**:`update.check()`(强制即时检查)、`update.state` 查询、`update.apply(version?)` 触发;设置 get/set 增 `update.auto_check` / `update.auto_install`(daemon 配置持久化)。事件 `event.update`(状态/进度,节流沿 ≥400ms 既有口径)。
- **CLI**:`mtdss updatecheck`(即时检查,打印当前/最新与可用性);`mtdss update [version]`(确认后经 daemon;空闲门**硬拒**:报「N 个传输中(含已暂停),可先取消作业后重试」,退出码 1——M4 无越过开关,反目标「更新不杀活跃传输」的直接推论)。
- **UI(D13)**:托盘「设置」子菜单增两个可勾选项「自动更新」「自动安装」(与既有自启动项同区;自动安装项随自动更新勾选启用/置灰,勾选态经 IPC 持久化到 daemon 设置);托盘「检查更新…」项拉起 **UI 更新页**(无已知新版时含手动「检查更新」按钮);检测到新版 → toast/通知 + 确认更新面(版本、notes、下载进度、安装按钮);安装中状态流转与失败可重试。

### 7.4 服务端与发布

- 信令增 `--plain-http <addr>`(如 `127.0.0.1:8787`)监听选项;TLS 模式保留(直连/过渡期/dev CA 场景)。
- 部署资产入库 `scripts/deploy/`:Caddyfile(`handle /dl/*` → file_server `/var/www/mtdss/dl`;其余 reverse_proxy `127.0.0.1:8787`)、`mtdss-signaling.service`(plain-http 形态)、`mtdss-stun.service`、目录布局与发布说明。
- **过渡**:备案完成前 VPS 维持现状(8787 直连 TLS + 现网客户端);域名+证书就绪后切 Caddy 形态——发布说明含切换步骤与回退;`--server` 既有客户端不受影响(直连 8787 仍可用)。
- **备案前演练拓扑(D15)**:VPS 上 Caddy 443 + 自签证书(复用信令 `--dev-gen-tls` 产物);updater 的 TLS 信任走自定 CA 配置/env(与 login `--ca` 同型,不设 accept-invalid 后门);信令默认地址与更新 base URL 为**两个独立编译期常量**(域名就绪后同源合一;信令直连 8787 期间二者天然不同源),各自独立 env 覆盖。演练前置 secrets:①更新签名密钥对(开发侧生成,私钥交用户离线保管,公钥入仓)——M4b 冒烟前;②部署 ssh key(用户在 VPS authorized_keys 加公钥 + GH secret)——首次发布演练前;③域名+正式证书——商业发布前。
- **release workflow**(`.github/workflows/release.yml`,tag `v*` 触发):三平台矩阵构建 → 版本一致性校验 → `mtdss-sign` 签 manifest → rsync/ssh 上 VPS `/dl`(部署私钥 = GH secret)→ 附 GitHub Release 归档(审计/兜底下载)。
- **客户端默认地址**:编译期常量(域名定案后注入;构建期 env 覆盖);`--server` 变可选(缺省走常量);`--ca` 可选(正式证书走系统根,dev CA 才需显式)。

### 7.5 验证与冒烟

- §E(SDDL 跨用户 E1–E4)执行并回填 release-checklist——纯人工,约 30 分钟,**可先行**(不依赖安装器)。
- 托盘唤起 D2/D3:`MTDSS_UI_BIN`/release 布局下补验(M4a 即可做,不必等安装器)。
- `docs/smoke-m4.md` 新手册 + checklist「M4 手动冒烟」段:安装/卸载/快捷方式/升级安装;更新全链(手动检查→确认→装→重启→版本翻新;自动安装静默档;回滚;空闲门;失败注入);三档开关行为;tar 平台解包可用;公网真机更新至少一例。

## 8. 里程碑切分建议

- **M4a 清债+硬化**:§7.1 全部 + §E 执行 + 托盘唤起 D2/D3 补验(MTDSS_UI_BIN 形态)。
- **M4b 打包+更新+服务端**:§7.2–7.4 + smoke-m4 + 发布演练(备案未完则用 IP+自签走通全链,域名就绪后切正式)。
- 两刀各自走 plan → subagent-driven 执行;门禁同全局约束。

## 9. 留给实现者的

- jobs 上限 N 值与淘汰测试口径;毒锁/驱动监督 join 的具体修法(从 M3c 代码考据);CSP 具体指令集;NSIS installerHooks 模板细节(Tauri v2 语法);manifest canonical 序列化实现;`update.*` RPC 的 proto 字段号分配;tar 内容布局;mtdss-sign CLI 形状;`--plain-http` 参数与 serve_tls 的分叉方式;restart-ui 标记的原子写/消费。

## 10. 开放项(不阻塞开发)

1. **域名**(备案中)——定案后注入编译期常量;发布前须就位。
2. **Windows 代码签名证书**——未购;首装 SmartScreen「未知发布者」警示为已知体验债,商业放量前建议购证(独立决策)。
