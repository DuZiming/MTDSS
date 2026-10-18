# M3a 冒烟测试手册

配套 `docs/release-checklist.md`「M3a 手动冒烟(收尾终检)」使用;本文覆盖环境要求、部署步骤、完整命令表与逐项测试判定标准。M3a = daemon 地基:登录/设备列表/CLI 传输/托盘/UI 壳;**资源管理器浏览界面是 M3b,窗口里的工具栏/地址栏/树均为占位,属预期**。

## 0. 当前已支持的能力边界

| 能力 | 状态 |
|---|---|
| 账号登录(邮箱+验证码,dev 码回显)/登出 | ✅ |
| 设备列表与在线状态(CLI/UI/事件) | ✅ |
| 文件传输:CLI `send` → 对端 daemon 落盘到其收件目录 | ✅(M2 语义:收端统一目录,**尚无按路径选择目标——M3c**) |
| 常驻 daemon + Windows 托盘 + 开机自启 | ✅(托盘/自启仅 Windows;Linux/macOS 无头) |
| Tauri UI 壳(设备列表/状态栏/登录引导) | ✅(仅 Windows) |
| 浏览远端文件/拖拽/复制粘贴 | ❌ M3b/M3c |

## 1. 测试环境要求

### 1.1 软件前置(每台 Windows 设备)

| 项 | 要求 | 说明 |
|---|---|---|
| OS | Windows 10 1809+ / Windows 11(UI 需 WebView2,Win11 自带) | Linux/macOS 设备可参与传输,但只有 CLI、无托盘无 UI |
| Rust 工具链 | stable-msvc(本机经 rsproxy 已装) | M3a 无安装包,源码构建;安装包是 M4 |
| protoc | winget 安装;**需设 `PROTOC` 环境变量**(见下) | mtdss-proto 构建必需,不设则 build 报 `Could not find protoc` |
| Node.js | ≥20(本机 v26 已装;UI 仅 Windows 设备需要) | `npm run tauri dev` 用 |
| 磁盘/权限 | 用户目录可写 | 数据目录 `$env:MTDSS_HOME`(默认 `~/.mtdss`) |
| 防火墙 | 放行 mtdssd.exe / mtdss.exe(专用/公用网络按拓扑勾) | 首跑会弹窗 |

### 1.2 网络要求

| 链路 | 协议/端口 | 条件 |
|---|---|---|
| 客户端 → 信令服务器 | TCP 8787(TLS,https/wss) | 所有设备可达 |
| 客户端 → STUN | UDP 3478 **和** 3479(双腿) | 所有设备可达;两端口都要放行 |
| 客户端 ↔ 客户端(P2P) | UDP 任意出口端口(QUIC) | NAT 打洞;对称 NAT 直连成功率低(会明确报错+提示),属已知行为 |

### 1.3 是否需要公网服务器?

**不需要**(对 M3a 清单而言):

| 拓扑 | 服务器位置 | 能测什么 |
|---|---|---|
| A. 单机回环(推荐先做) | 本机 127.0.0.1 | M3a 全部 7 项清单(登录/托盘/自启/CLI/UI);「第二台设备」用第二个 `MTDSS_HOME` 模拟(传输走回环) |
| B. 局域网双机 | 任一台机器上 | 上述全部 + 真实第二设备在线状态、跨机真实传输(推荐,离产品最近) |
| C. 公网跨网(可选,发布前必做一次) | 公网 VPS | 跨 NAT 真实打洞传输;对应 release-checklist 的「真机双腿」已知项(**STUN 必须 `--bind` 显式公网 IP 通告,0.0.0.0 会回包源漂移**,见检查单) |

公网部署要点(仅拓扑 C):8787/3478/3479 对公网开放;信令建议真实域名+正式证书(没有则用 dev CA,客户端 login 带 `--ca`);`--stun-addr <公网IP>` 必须显式。

## 2. 部署

### 2.1 服务端(信令 + STUN,一台即可)

**先设 PROTOC(每个新 PowerShell 终端,或做永久设置):**

```powershell
# 一次性(本终端):
$env:PROTOC = "C:\Users\Administrator\AppData\Local\Microsoft\WinGet\Packages\Google.Protobuf_Microsoft.Winget.Source_8wekyb3d8bbwe\bin\protoc.exe"
# 永久(推荐,设完重开终端生效):
[Environment]::SetEnvironmentVariable("PROTOC", "C:\Users\Administrator\AppData\Local\Microsoft\WinGet\Packages\Google.Protobuf_Microsoft.Winget.Source_8wekyb3d8bbwe\bin\protoc.exe", "User")
```

```powershell
cd <repo>
cargo build --release -p signaling -p stun-server -p mtdss-cli -p mtdss-daemon

# 一次性:开发证书(公网正式部署换真证书)
mkdir smoke\tls, smoke\db | Out-Null
target\release\signaling-server.exe --dev-gen-tls smoke\tls

# 常驻终端 1:STUN(拓扑 C 用 --bind <公网IP>:3478 --bind-alt <公网IP>:3479)
target\release\stun-server.exe

# 常驻终端 2:信令(拓扑 C 加 --stun-addr <公网IP>)
target\release\signaling-server.exe smoke\db\signaling.db `
  --tls-cert smoke\tls\server.crt --tls-key smoke\tls\server.key
```

服务器就绪标志:终端 2 打印 `signaling server (TLS) on 0.0.0.0:8787`,终端 1 打印 `stun on 0.0.0.0:3478 and 0.0.0.0:3479`。

### 2.2 客户端(每台 Windows 设备)

```powershell
cd <repo>          # 或:git clone 后同目录结构
cargo build -p mtdss-cli -p mtdss-daemon    # debug 即可;UI dev 会找 target\debug\mtdssd.exe

# (可选,强烈建议)冒烟期间用沙盒数据目录,与日常隔离:
$env:MTDSS_HOME = "<repo>\smoke\home_<设备名>"

# 首次登录(见 §3 命令表)——冷启动会自动拉起 daemon(托盘亮色图标出现)
target\debug\mtdss.exe login --server https://<服务器IP>:8787 --email me@test.dev `
  --ca smoke\tls\ca.crt --name <本设备名>
# → 打印 `dev code: XXXXXX`,输入该码回车 → 输出 device_id 即成功
```

Linux/macOS 设备(仅 CLI):同仓库 `cargo build -p mtdss-cli -p mtdss-daemon` 后,`mtdssd --no-tray --log-file ...` 常驻(建议 systemd 用户服务托管,M3a 阶段手动起即可),再 `mtdss login ...`。

**多设备要点:所有设备 `--email` 用同一个 = 同一账号,才会互相可见。**

### 2.3 UI(每台 Windows 设备,可选但推荐)

```powershell
$env:MTDSS_HOME = "<repo>\smoke\home_<设备名>"   # 继承给 tauri dev 拉起的 daemon
cd ui ; npm install(首次) ; npm run tauri dev
```

## 3. 使用:命令全表(M3a 全部已支持入口)

### CLI `mtdss.exe`(全部经本机 daemon,不直连服务器)

| 命令 | 参数 | 行为 |
|---|---|---|
| `mtdss login` | `--server <URL>` `--email <E>` `[--ca <PATH>]` `[--name <N>]` | 请求验证码(dev 码直接打印)→ stdin 输码 → 登记设备;**daemon 未运行时自动拉起** |
| `mtdss logout` | — | 删除本机会话,托盘图标转灰 |
| `mtdss devices` | — | 账号设备列表:`device_id  name  platform  online` |
| `mtdss status` | — | daemon 状态:登录态/在线态/服务器/本设备/收件目录 |
| `mtdss send` | `<文件或目录...>` `--to <对端device_id>` | 传输(多文件/目录混合;进度行输出;失败输出逐文件报告) |
| `mtdss recv` | `--out <DIR>` | 设置本机收件目录(持久化;接收是常驻自动的,无需守着命令) |

### daemon `mtdssd.exe`(通常无需手动运行——CLI/UI 自动拉起)

| 旗标 | 默认 | 说明 |
|---|---|---|
| `--recv-dir <DIR>` | `<MTDSS_HOME>\inbox` | 收件目录 |
| `--log-file <PATH>` | stderr | JSON 行日志(排障看这里) |
| `--no-tray` | 带托盘 | 无头运行(Linux/macOS 恒无头) |

### 环境变量

| 变量 | 作用 |
|---|---|
| `MTDSS_HOME` | 数据目录(身份/会话/设置/IPC socket);**测试多设备同机隔离的关键** |
| `MTDSSD_BIN` | 显式指定 mtdssd 路径(自动拉起找不到时) |
| `RUST_LOG` | daemon/CLI 日志级别(默认 info) |

### 托盘(仅 Windows)

登录=亮色图标,登出=灰色;右键:打开主窗口(M3a 置灰)/ 登录·登出(动态文案)/ 设置 ▸ 开机自启 / 退出。

### UI 窗口(M3a 壳)

侧栏「MTDSS设备」→ 设备列表(名称/平台/在线,本机置顶标注「(本机)」);底部状态栏「信令:在线/离线/未登录 · N 台设备」;未登录时为登录引导表单。其余(工具栏/地址栏/常用目录)全部为占位。

## 4. 测试项清单(逐项判定标准)

> 建议顺序执行;每项记录 通过/失败 + 现象;失败时附 `--log-file` 日志文件。
> 单机拓扑:A1–A7 全部可测;B 组需第二设备(真实或同机第二 home)。

### A. 生命周期与集成(M3a 核心新增面)

- **A1 CLI 自动拉起**:确保无 mtdssd 进程(任务管理器查)→ `mtdss status` → 数秒内返回状态且托盘图标出现。判定:不报「找不到 mtdssd」,进程存在。
- **A2 登录/登出联动**:login 成功 → 托盘亮;`mtdss logout` → 托盘灰;再 login → 亮。判定:三次状态变化均正确且即时(≤2s)。
- **A3 托盘右键四项**:打开主窗口(置灰)、登录/登出(登录态下文案=「登出」)、设置▸开机自启(可勾选)、退出。判定:四项齐全、文案随登录态正确。
- **A4 托盘优雅退出**:点「退出」→ 图标消失、`<MTDSS_HOME>\daemon.lock` 文件已删除、无 mtdssd 进程残留;随后 `mtdss status` 能重新自动拉起。判定:三条件全满足。
- **A5 开机自启**:勾选 → `reg query HKCU\Software\Microsoft\Windows\CurrentVersion\Run /v MTDSS` 出现带引号 mtdssd 全路径;取消勾选 → 值消失。(注:受管机 EDR 拦写属预期降级——勾选报业务失败也算验证了失败面,但**该项最终判定需在常规 Windows 复测**。)
- **A6 UI 壳**:`npm run tauri dev` → 窗口出、连 daemon、登录后设备列表可见、`mtdss logout` 后 UI 回引导态、列表随设备上下线刷新。判定:四处状态联动正确。
- **A7 UI 冷启自动拉起**:A4 之后(无 daemon)直接重开 UI → 数秒内出登录态/设备列表,无手动干预。判定:无需先跑任何命令。

### B. 双设备与传输(CI 已覆盖逻辑,此处验真实环境)

- **B1 设备互见**:第二设备(真机或同机 `$env:MTDSS_HOME=...\home_b` + `mtdssd --no-tray` + login)→ 两侧 `mtdss devices` 互见且 online=true;UI 列表同步。
- **B2 单文件传输**:`mtdss send <文件> --to <DEV_B>` → 对端收件目录出现同名文件、内容一致(send 输出 `sent N bytes`)。
- **B3 目录混合传输**:send 一个目录+一个文件 → 对端按 `<目录名>/<内部路径>` 结构落盘。
- **B4 断点续传(可选,破坏性)**:传输大文件(≥500MB)中途任务管理器杀对端 mtdssd → send 应报重试并失败(或仍在跑则等其退出)→ 对端重新 `mtdssd` → 重发 → 只补缺失部分完成。
- **B5 收件目录切换**:`mtdss recv --out 新目录` → 再收一次 → 落新目录。

### C. 记录格式

每项:`编号 | 通过/失败 | 现象一句话 | (失败时)日志文件路径`。日志:daemon `--log-file` 指定的文件 + CLI 终端输出截图/文本。

## 5. 故障排查速查

| 现象 | 先查 |
|---|---|
| login 报连不上服务器 | 服务器终端是否存活;客户端到 8787 TCP(`Test-NetConnection <ip> -Port 8787`);`--ca` 是否给对 |
| `dev code` 后输码报错 | 服务器 `verification code` 日志;码 10 分钟有效、一次性 |
| devices 一直 offline | 对端 daemon 是否存活、是否同一 `--email`;对端 `mtdss status` 的 online 字段 |
| send 报 symmetric NAT | 双方至少一方对称 NAT,属已知行为;换网络或同网段复测 |
| 找不到 mtdssd | `MTDSSD_BIN` 指向二进制;或把 mtdssd.exe 放到 mtdss.exe 同目录 |
| 托盘不出现 | 是否 `--no-tray`;`<MTDSS_HOME>` 是否一致;explorer 重启后图标偶失——A4 流程重走 |
