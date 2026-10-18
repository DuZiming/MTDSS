# M3b 冒烟测试手册

配套 `docs/release-checklist.md`「M3b 手动冒烟」使用;本文覆盖 M3b 增量面与逐项测试判定标准。环境要求与部署步骤(信令/STUN 服务端、客户端构建、双 `MTDSS_HOME` 隔离)**沿 `docs/smoke-m3a.md` §1–§2**,不重复。

M3b = 远端文件浏览:CLI `ls`、UI 浏览面(左树/三视图/排序/搜索/多选/导航/状态栏/离线态)、托盘唤起主窗与登录登出联动、IPC 管道 SDDL 收紧。M3a 已验的传输/托盘基础面/自启不重复覆盖。

## 0. 当前已支持的能力边界

| 能力 | 状态 |
|---|---|
| CLI `mtdss ls <设备> [路径]`(根卷/目录列表;按 device_id 或设备名定位) | ✅ M3b 新增 |
| UI 左树逐级展开(本机+远端,懒加载)+ 设备层/目录层双击进入 | ✅ M3b 新增 |
| 目录三视图:详细信息 / 列表 / 平铺(工具栏「查看」菜单) | ✅ M3b 新增 |
| 排序:名称递增/递减(目录恒在前)+ 搜索过滤(客户端即时) | ✅ M3b 新增 |
| 多选:单击单选 / Ctrl 增减 / Shift 范围;状态栏计数与已选数 | ✅ M3b 新增 |
| 导航:面包屑点击、后退/前进/上级(←/→/↑) | ✅ M3b 新增 |
| 离线态:树灰化 + 横幅提示 + 内容保留 + 进入阻断 | ✅ M3b 新增 |
| 托盘「打开主窗口」唤起/聚焦主窗(单实例);「登录」拉起主窗引导登录 | ✅ M3b 新增 |
| IPC 管道 DACL SDDL 收紧(仅当前用户/SYSTEM/Administrators) | ✅ M3b 新增(跨用户人工验证见 §E) |
| CLI 自动拉起自适应就绪窗(首跑安全软件扫描不再 10s 硬失败) | ✅ M3b 新增 |
| 双击文件打开/下载、复制粘贴、拖拽、右键菜单、传输速率汇总 | ❌ M3c |
| 设备重命名、`.mtdss` 大小写不敏感 | ❌ 后置(M3c/V1) |

## 1. 环境与准备(沿 M3a,增量如下)

1. 服务端照 M3a §2.1(回环信令 + STUN,`smoke\tls` 开发证书)。
2. 客户端照 M3a §2.2:`cargo build -p mtdss-cli -p mtdss-daemon`;**同一邮箱**登录两台「设备」:
   - 设备 A(本机,带托盘):`$env:MTDSS_HOME = "<repo>\smoke\home_a"` 后 `mtdss login ... --name dev_a`;
   - 设备 B(模拟远端,无头):另开终端 `$env:MTDSS_HOME = "<repo>\smoke\home_b"` 后 `target\debug\mtdssd.exe --no-tray --log-file smoke\home_b\d.log`,再同法 `mtdss login ... --name dev_b`(B 组命令在 home_b 环境终端执行)。
3. 两侧 `mtdss devices` 互见且 `online=true`(M3a B1 口径)。
4. UI:`$env:MTDSS_HOME` 指向 **home_a** 后 `cd ui ; npm install(首次) ; npm run tauri dev`。窗口出、登录态正常、设备列表两台在线。
5. 判定基线:以下各步骤的「期望结果」均以 §1 环境为前提;执行中失败先查 `--log-file` 日志。

> 本机收件目录里会出现 `.mtdss`(隐藏收件区)——列表**不应**显示它(设计保留段),B 组可顺带核对。

## 2. 测试项清单(逐项判定标准)

> 建议顺序执行;每项记录 通过/失败 + 现象一句话;失败时附 daemon `--log-file` 与 CLI 终端输出。
> CLI 命令均在 **home_a** 环境终端执行(浏览「远端」设备 B);标注「home_b 终端」的除外。

### A. CLI 浏览(`mtdss ls`)

- **A1 根卷列表(空路径)**:`mtdss ls dev_b`。判定:输出每个在位盘符一行,形如 `d<TAB>C:<TAB>0<TAB><毫秒时间戳>`(按 `A:`→`Z:` 顺序,仅本机在位卷);**无** `.mtdss` 类保留段干扰;退出码 0。
- **A2 按 id 与按名等价**:先 `mtdss devices` 记下 dev_b 的 `device_id`;`mtdss ls <device_id>` 与 `mtdss ls dev_b` 输出逐行一致。判定:两种定位等价。
- **A3 目录列表**:`mtdss ls dev_b C:\Users`。判定:每行 `{d|-}<TAB>名称<TAB>大小<TAB>mtime`(d=目录,-=文件);目录在前、组内按名称字节序;点一个已知文件核对其 size 与资源管理器一致。
- **A4 离线设备快错**:在 home_b 终端结束 mtdssd 进程(任务管理器杀或 Ctrl+C)→ 等设备 A 侧 `mtdss devices` 显示 B `online=false` → `mtdss ls dev_b C:\`。判定:**秒级**失败(不进入打洞等待),stderr 文案「设备离线: <device_id>」(daemon RPC code 3),CLI 退出码 1。
- **A5 设备名歧义报错**:再起第三台(同法:`$env:MTDSS_HOME = "<repo>\smoke\home_c"` + `mtdssd --no-tray` + `mtdss login ... --name dev_b`,与 B **同名**)→ 设备 A 终端 `mtdss ls dev_b C:\`。判定:报错「设备名「dev_b」对应多台设备,请改用 device_id:」并逐行列出全部同名的 device_id;退出码 1;此时按 id 的 `mtdss ls <device_id>` 仍正常。测完可留 home_c 在线不碍事。
- **A6 truncated 提示(可选,大目录)**:在任一卷建一个 >10000 个文件的目录(如 PowerShell:`1..10500 | % { New-Item -ItemType File "C:\temp\bigdir\f$_.txt" -Force } | Out-Null`)→ `mtdss ls dev_b C:\temp\bigdir`。判定:输出恰 10000 行条目,末行追加「…(结果已截断,仅显示前 10000 项)」;条目为排序后前 10000(确定子集,非 OS 乱序切片)。测完删目录。
- **A7 CLI 自动拉起自适应(可选,冷启首跑)**:结束全部 mtdssd 进程 → home_a 终端 `mtdss status`。判定:10s 内就绪则直接返回状态;若首跑遇 Defender/EDR 扫描,应先看到 stderr 一次提示「守护进程 10s 内未就绪但仍在运行:…继续等待(上限 60s)…」并最终成功,不再像 M3a 那样 10s 硬失败。

### B. UI 浏览(双 daemon 在线)

> 以下操作在 `npm run tauri dev` 的主窗内进行;设 B 组开始时 A4/A5 的临时下线/第三台已复原(至少 dev_a、dev_b 两台在线)。

- **B1 左树逐级展开**:侧栏树先见两台设备(dev_a 标注「(本机)」)→ 点 dev_a 展开显示在位盘符 → 逐级点开 `C:` → `C:\Users`…;对 dev_b 重复。判定:懒加载逐级拉取(点开才请求),层级与真实目录一致;「常用目录」区为灰置占位(V2)。
- **B2 三视图切换**:进入任一目录 → 工具栏「查看」菜单在 详细信息/列表/平铺 三者切换。判定:详细信息=名称/修改日期/类型/大小四列表格;列表=单列名称;平铺=图标+名称网格;数据同源,切换不重新拉列表。
- **B3 排序升降**:工具栏「排序」菜单切「名称递增/名称递减」。判定:文件组按名称升/降,**目录恒在最前**(降序也不沉底);状态栏计数不变。
- **B4 搜索过滤**:地址栏搜索框输入已知条目名的片段(如 `use`)。判定:客户端即时过滤,仅剩匹配行;输入必然无匹配的串(如 `zzz`)→ 列表清空显示空态(「此文件夹为空」),状态栏「0 个项目」;清空搜索框恢复全量;**换目录导航后搜索词自动清空**。
- **B5 多选**:单击=单选;Ctrl+单击=增减;Shift+单击=从最近单击行(锚点)到当前行的范围。判定:选中行高亮与状态栏「已选 N」随操作正确;点空白处清空选择与锚点。
- **B6 状态栏**:浏览目录时底部应显示「信令:在线 · N 台设备」与「M 个项目[,已选 K]」;M 与所见行数一致(搜索过滤后=过滤数),设备层不显示项目数。判定:计数、已选数与实际一致。
- **B7 导航**:面包屑「MTDSS设备 ▸ dev_b ▸ C: ▸ Users」——点中间某段跳到该层,**当前段(最后一段)点击无动作**;← 后退/→ 前退按历史栈可用(无历史时置灰);↑ 上级逐级上溯,目录根(`C:\`)再 ↑ 回设备层,设备层 ↑ 置灰。判定:各方向导航后列表、面包屑、树高亮三者一致。
- **B8 双击行为**:设备层双击 dev_b 行 → 进入其根卷(离线行禁入);目录视图双击目录行 → 进入;双击**文件**行 → 无任何动作(打开/传输是 M3c)。判定:进目录正常,双击文件确实 no-op。

### C. UI 离线态

- **C1 杀远端 daemon 的表现**:正浏览 dev_b 的某子目录时,home_b 终端杀掉 mtdssd → 等 UI 收到离线事件。判定:①顶部横幅「设备已离线,当前显示的内容可能已过期」;②**当前列表内容保留**(不清空不报错);③左树 dev_b 连同已展开子节点灰化,点击展开无效;④双击其他子目录行/回设备层再双击 dev_b 行均被阻断(no-op);⑤设备列表该行转「离线」。
- **C2 重启恢复**:home_b 终端重跑 `target\debug\mtdssd.exe --no-tray --log-file smoke\home_b\d.log`(会话已持久化,无需再登录)→ 等上线。判定:横幅消失、树恢复可展开、双击进入恢复、设备行回「在线」;两侧 `mtdss devices` online=true。

### D. 登录/登出与托盘联动

- **D1 托盘登出 → UI 回引导态**:主窗开着且已登录 → 托盘右键点「登出」。判定:托盘图标转灰、菜单文案变「登录」;UI 收到登录态事件,回到「MTDSS设备」顶层并显示登录引导表单(欢迎语+服务器地址/邮箱/验证码),不残留浏览内容。
- **D2 托盘「登录」→ 主窗登录表单**:保持登出态 → 托盘右键点「登录」。判定:主窗被唤起(未开则新开),内容为登录引导表单;完成表单登录后托盘转亮、UI 进设备列表。
- **D3 托盘「打开主窗口」唤起/聚焦**:主窗已关 → 托盘点「打开主窗口」→ 新拉起主窗;主窗开着(可最小化)再点一次 → 不新开窗,既有窗被带到前台聚焦。判定:两种场景均正确;二次点击后任务管理器中 `ui.exe` 进程仍只有 1 个(单实例,第二实例自退)。
- **D4 CLI 侧登出同联动(回归)**:已登录+UI 开着 → 终端 `mtdss logout`。判定:UI 同样回引导态(M3b 起登出全量复位,含浏览历史与搜索词)。

### E. SDDL 跨用户手工验证(对应 checklist「已知安全备注」末项)

> 思路:同一管道,同用户可连(对照组,§A/B 已隐含覆盖),**他用户**直接试连必须被拒。CI 无法覆盖,须桌面环境人工执行。

- **E1 定位管道名**(管理员 PowerShell):`Get-ChildItem \\.\pipe\ | Where-Object Name -like 'mtdss-*'` → 得 `mtdss-<8位hex>`(home_a 与 home_b 各一条;home 对应关系可按 `$env:MTDSS_HOME` 归一化路径的 SHA-256 前 8 位核对)。记下 home_a 的那条。
- **E2 同用户对照(应通过)**:home_a 终端 `mtdss status` 正常返回,即同用户对该管道读写正常(A/B 组全部通过本身就是对照)。
- **E3 建临时本地用户并侧试**:
  ```powershell
  # 管理员 PowerShell(密码用一次性口令,勿复用真实凭据):
  net user mtdsstest <一次性密码> /add
  runas /user:mtdsstest cmd    # 输密码,弹出以该用户身份运行的 cmd
  ```
  在弹出的 cmd(或其他用户会话)里试连管道(**只读连接**):
  ```powershell
  $p = New-Object System.IO.Pipes.NamedPipeClientStream(".", "mtdss-<8位hex>", [System.IO.Pipes.PipeDirection]::In)
  $p.Connect(3000)
  ```
  判定:`Connect` 抛 `UnauthorizedAccessException`/拒绝访问(ACCESS_DENIED)——SDDL `D:P(A;;GA;;;当前用户SID)(A;;GA;;;SY)(A;;GA;;;BA)` 不放行他用户;该用户也**收不到**事件广播(设备名/在线态/传输计数不可被动旁观)。
- **E4 清理**:`net user mtdsstest /delete`;关闭 runas 窗口。

### F. 记录格式

每项:`编号 | 通过/失败 | 现象一句话 | (失败时)日志文件路径`。日志:各 daemon `--log-file` 文件 + CLI 终端输出文本。全部通过后回填 `docs/release-checklist.md`「M3b 手动冒烟」段勾选。

## 3. 推后到 M3 完成后的验证

公网真机拓扑(公网 Ubuntu 服务器;客户端 Linux CLI / macOS CLI / Windows CLI+UI)——沿 `docs/release-checklist.md`「M3a 手动冒烟」末「推后到 M3 完成后的验证」清单执行,M3b 增量补测:

- [ ] 公网拓扑下 `mtdss ls` 对真实远端(跨 NAT 打洞)根卷/目录列表正常,离线快错语义不变
- [ ] Linux/macOS 无头端参与浏览(其作为被浏览方的根卷为 `/` 单条目)

## 4. 故障排查速查(M3b 增量)

| 现象 | 先查 |
|---|---|
| `ls` 报找不到设备 | `mtdss devices` 核对名/id;重名设备必须用 device_id(A5) |
| `ls` 长时间无响应 | 对端是否真在线;远端控制通道日志(ctrl);信令是否存活 |
| 树展开一直「加载中…」/报错 | 浏览的设备是否离线;daemon `--log-file`;用「重试」按钮重发 |
| 托盘「打开主窗口」无反应 | 是否已 `npm run tauri dev` 构建出 `ui\src-tauri\target\debug\ui.exe`(托盘按解析序找 ui 可执行文件);daemon 日志有「未找到 ui 可执行文件」即此因 |
| SDDL 测试连不上(同用户也拒) | 管道名 8 位 hex 是否取对(对应的是哪个 MTDSS_HOME);以管理员查询管道列表重试 |
