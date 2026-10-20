# MTDSS 发布检查单

## 跨真实网络对传（V1 发布前执行一次，记录归档本目录）

1. 两处不同 NAT 网络（如 家宽 × 手机热点）各一台设备,各自起 `mtdssd` 并 `mtdss login`
2. `RUST_LOG=info` 下多文件对传，记录两侧日志的 `candidates:` 行（srflx 值）与 `symmetric=` 预判
3. 记录：日期 / 两侧网络环境 / srflx 是否出现 / 传输结果（成功字节数或失败文案）

- [ ] 家宽 × 热点 对传记录：____

## STUN / 部署

- 自建 STUN 双腿部署须显式绑定对外通告 IP（`--bind <advertised-ip>:port`），
  `0.0.0.0` 绑定会让回包源地址随出口腿漂移（Task 8 根因：客户端按回复源
  地址比对候选，漂移即整条腿作废）。

## 已知问题(V1 发布前处理)

- [x] `.mtdss` 保留段匹配为大小写敏感——**已于 M3c Task 1 硬化**:保留名比较改大小写不敏感,
  三处同口径(`crates/core/src/fs.rs` 列表、`crates/core/src/protocol/manifest.rs`
  `validate_rel_path` 首段、`crates/daemon/src/flow.rs` `collect_items`/`walk_dir`);
  `.MTDSS`/`.Mtdss-part` 等变体一律滤除/拒绝/跳过,`x.mtdss-part` 后缀名不误伤。
  冒烟复核项:`docs/smoke-m3c.md` A11。同批偿清 `C:foo` 盘符相对路径拒绝(冒烟项 A10)。

## Windows 托盘与开机自启（Task 9）

单测覆盖纯函数/RPC/注册表往返；以下为不可单测的桌面交互面，发布前在常规 Windows 桌面环境人工冒烟：

- [ ] 托盘图标颜色随登录态翻转：`mtdss login` 后亮色（#3B82F6 蓝圆），`mtdss logout` 后灰色（#9CA3AF）
- [ ] 右键菜单四项：打开主窗口（唤起主窗，单实例二次点击聚焦既有窗——M3b 起可用，M3a 按设计置灰）、登录/登出（动态文案，可用：未登录点击拉起主窗引导登录）、设置 ▸ 开机自启（勾选态）、退出
- [ ] 退出优雅：托盘「退出」→ 图标消失、`daemon.lock` 文件已删除、进程收干净（此后 `mtdss` 命令报 daemon 缺失并可重新拉起）
- [ ] 开机自启开关真实生效：勾选后 HKCU `Software\Microsoft\Windows\CurrentVersion\Run` 出现 `MTDSS` = 带引号的 mtdssd.exe 全路径；重启资源管理器/重登录后 mtdssd 自启；取消勾选后该值删除（注：受管机器若 EDR 拦截 Run 键写入，RPC 会业务失败——属预期降级）

## M3a 手动冒烟（收尾终检）——2026-10-18 本机回环已验

冒烟环境:本机回环服务端 + 双客户端(双 MTDSS_HOME 模拟);手册 `docs/smoke-m3a.md`;结果记录:

- [x] 托盘亮/灰随登录态:`mtdss login` 后亮色,`mtdss logout` 后灰色(已验)
- [x] 托盘右键四项:打开主窗口、登录/登出 **M3a 按设计置灰**(设计 §5.3 修订句;M3b 跟进:UI 已落地后放开「打开主窗口」唤起主窗、「登录」唤起登录表单)、设置 ▸ 开机自启、退出(已验)
- [x] 托盘退出优雅:daemon.lock 清除、`mtdss status` 可重新拉起(已验)
- [x] 开机自启:HKCU Run 键写入/删除成功(**真实开机自启未验**——受管机限制,M3 后在常规 Windows 验)
- [x] CLI 自动拉起 daemon:冷启 `mtdss status` 直接拉起(已验;注:二进制首跑遇 Defender/EDR 扫描可能超 10s 就绪窗,重试即过——M3b 收紧为自适应窗口)
- [x] UI 壳交互:连接/登录/登出联动、设备列表与在线状态刷新(已验)
- [x] UI 冷启:自动拉起 daemon 并出设备列表(已验)
- [x] 双客户端传输:文件/目录 send 落盘正常(回环拓扑;跨公网/真机推 M3 后)

**推后到 M3 完成后的验证**(届时拓扑:公网 Ubuntu 服务器;客户端 Linux CLI / macOS CLI / Windows CLI+UI):
- [ ] 公网真机双腿(STUN `--bind` 显式公网 IP,见上「已知项」)
- [ ] 真机多客户端跨公网传输(含真实 NAT 形态)
- [ ] Ubuntu / macOS 客户端 CLI 运行(无头 daemon + login/devices/send/recv)
- [ ] 开机自启真实开机验证(常规 Windows)

## M3b 手动冒烟(2026-10-19 本机回环已验)

冒烟环境:本机回环服务端 + 三客户端(三 MTDSS_HOME;A 带托盘 UI,B/C 无头,其中 C 与 B 同名制造歧义现场);手册 `docs/smoke-m3b.md`。核心项判定:

- [x] CLI `ls`:根卷列表/目录列表/按 id 与按名定位均正确(A1–A3;排序前缀截断提示 A6 亦过:恰 10000 行+末行提示)
- [x] CLI 错误面:离线设备秒级快错「设备离线」(55ms,code 3)、同名设备歧义提示并指引改用 id(退出码 1)(A4/A5)
- [x] UI 浏览:树逐级展开(本机+远端)、三视图切换、排序升降(目录恒在前)均正确(B1–B3;搜索过滤/零匹配空态未逐项展开,逻辑有 vitest 覆盖)
- [x] UI 多选与状态栏:单击/Ctrl 增减/Shift 范围选择正确,状态栏计数与已选数一致(B5/B6)
- [x] UI 导航:面包屑分段跳转(当前段惰性)、←/→/↑、双击进目录、双击文件无操作(B7/B8)
- [x] UI 离线态:树灰化+横幅+内容保留+进入阻断,重启恢复(C1/C2)
- [ ] 托盘联动:托盘登出/登录表单唤起未测(D1/D2);「打开主窗口」拉起/聚焦在 dev 构建下应可达(ui.exe 已产出)但未验(D3/D4)——**待补验**
- [ ] SDDL 跨用户:同用户 CLI 正常(对照组过);他用户试连管道被拒**未执行**(步骤见手册 §E,对应下「已知安全备注」末项)——**待补验(M4 打包前必须)**

**冒烟发现(记 M3c/优化清单,非阻断)**:
1. 设备改名(重登换名)对其他在线设备的目录快照不可见——hub 仅自身重连/未知设备触发全量刷新,presence 翻转只就地改标志;`ls` 按名匹配与 UI 树直到本端 daemon 重启才收敛(A5 冒烟时实测)。候选修法:`devices` RPC 现拉 REST 或 hub 低频周期刷新。
2. UI 打磨项(用户反馈):左树加载偏慢;树节点文字/列宽超界;三视图样式待优化;Shift 范围选中时行文字蓝底观感差。
3. dev 模式下 UI 由 `npm run tauri dev` 拉起时托盘唤起路径可用但未验(见上 D3 待补)。

## M3c 手动冒烟(代码任务已完成;交互冒烟待执行)

冒烟环境:本机回环服务端 + 三客户端(三 MTDSS_HOME;dev_a 带托盘 UI,dev_b/dev_c 无头,A→B 用第三台);手册 `docs/smoke-m3c.md`。**环境前置:先退出全部旧 mtdssd 进程**(托盘计数/jobs 面需新编译 daemon,旧实例占 socket)。核心项判定(结果待回填):

- [ ] CLI put/get:定向落盘(默认收件目录无新增)、`--conflict ask` 问询(s/o/c,非法重问、取消退出码 0)与 skip/overwrite 直发、嵌套同名自动改名安全网(A1–A4 及附)
- [ ] CLI cp 四态:download/upload 糖、同设备糖(即时、自动改名+提示)、A→B 三 daemon 中转不经本机落地、本机↔本机拒答(A5)
- [ ] CLI rm/mv:静默回收、`--permanent`、部分失败逐条报(批不弃废)、mv 跨设备指引 cp(回收不可用确认梯视环境可 SKIP);卷根与 `.mtdss` 保留段操作对象拒绝(终审修复,A12)(A6–A9、A12)
- [ ] UI 冲突对话框:单条两键、批量四键(全部键只作用未决、决完自动放行)、目录恒合并不弹框、类型不匹配行注、Esc/遮罩取消 + 模态占线、预扫失败中止整批(B1–B6)
- [ ] UI 粘贴与拖拽:Explorer→MTDSS 上传、三向粘贴(本机 download/另远端 remote_copy/同设备 device_copy 无弹框)、**「下载到本机…」OS 目录选择器直落**(C8)、剪切上传删源(回收)、窗内拖(树节点/面包屑;跨设备=复制、同设备=移动、根节点不收)、OS 拖入(C1–C8)
- [ ] 拖出矩阵:桌面/普通资源管理器窗口(含驱动器根)落盘——Task 10 spike 结论**窗口落点留 V1**(探针 9 类×10=90/90,解析全路径 ≈70ms);库/此电脑/回收站/第三方窗 → 提示条;全程不交付数据(D1–D4)
- [ ] 传输面板与控制:卡片聚合、暂停/继续(双侧挂起+续传)、取消(download 取消传播到源端不重试)、done 5s 自隐、failed 留置、remote_copy 拒暂停提示、状态栏「↓ 速率(N 个传输中)」;排队/重试间隙暂停不丢——开轮保持冻结、继续后完成(终审修复,E9)(E1–E9)
- [ ] 托盘 tooltip 计数:「MTDSS — N 个传输中」随非终态数翻转、归零回「MTDSS」(E8;需新 daemon)
- [ ] 通知:被写方系统通知双向(upload→B 侧、download→A 侧 + UI toast)、全 skip 不通知(F1–F3)
- [ ] 删除与回收:UI Del 静默回收、离线友好文案「设备离线,无法完成操作」、部分失败提示;回收不可用确认梯视环境(G1–G4,联动 A6–A8)
- [ ] 携带项回归:C:foo 拒绝、`.MTDSS` 大小写滤除(后缀不误伤)、登出拆卸 ctrl(登出→登录→立即操作可用)、devices 现拉(重启对端后 presence 与目录一致)+ focus 重拉、StatusBar 加载态(A10/A11/H1–H4)

**M3b 遗留待补验(留空待用户确认,已补验则勾上节)**:托盘唤起 D1–D4 与 SDDL 跨用户 §E 仍未勾(见上「M3b 手动冒烟」段;步骤 `docs/smoke-m3b.md` §D/§E)——SDDL 项 M4 打包前必须。本次冒烟环境同时满足其前置,可顺带补验。

**EOF 2s 竞速重估(Task 14 Step 1)**:维持 2s 宽限仍充分,不立项修。终局/拒绝两族路径在定向+策略+暂停新流下均未新增 EOF 依赖:①终局——收端仅在 checkpoint 臂(`if !paused_now`)发 `[0x03]`,暂停期间该臂整体禁用,EOF 舞步在结构上不可能始于暂停态;发端收到 Final 后无任何门直通 `ctl_tx.finish()`,EOF 立即到来。②拒绝——策略经 PLAN 帧落地(命中策略不发 REJECT),`reject_exit`/`reject_with_stop` 与字段零改动。③暂停中段——双侧共享 PauseGate 只停新块,keep-alive 10s(< idle 30s)保活连接,轮内不断链即不触舞步;断链/取消走 `conn.close`/任务 abort,`wait_peer_eof_grace` 的 read 立即出错返回(不等满 2s),且有界化保证最坏 2s 干净退出。④download 取消经 ctrl cancel_job 两侧同 abort(`run_send_with` 重试循环一并杀,不复活),无「收端已放行、发端还在重试」的错位窗口。结论:2s 宽限在新流下仍覆盖全部在途帧投递窗口,无实质竞速。

**M3c 后置项(deferred,记录依据)**:
- CSP 硬化——M4 打包前
- 设备重命名——需服务器端点
- 接收端并发多传输——同向串行,~50s Accept 窗口限制已记(第二传窗外失败可重试)
- 远端源嵌套冲突全粒度预扫——自动改名 `name (1)` 安全网兜底
- 跨会话断点续传——V2
- `mtdss send` 遗留 RPC 保留未迁移 jobs——e2e/nat-matrix 兼容面

## 已知安全备注（Windows IPC ACL）

- [x] 管道 DACL 收紧已落地（M3b,`crates/ipc` `serve`）:经 interprocess 2.4.6
  的 `ListenerOptionsExt::security_descriptor` 挂显式 SDDL
  `D:P(A;;GA;;;{当前用户SID})(A;;GA;;;SY)(A;;GA;;;BA)`——仅当前用户 /
  SYSTEM / Administrators 拥有全部访问权,替代 `CreateNamedPipe` 默认 DACL
  对 Everyone/匿名只读的放行(此前同机他用户可被动接收事件广播:
  `event.devices` 设备名/在线态、`event.send_log` 计数;无文件路径、无凭据、
  不可发 RPC;凭据从不经 IPC 广播)。SID 查询/转换失败回退默认 DACL 并留痕,
  不因收紧失败破坏 IPC。
- [ ] 跨用户人工验证(Task 11 冒烟,步骤见 `docs/smoke-m3b.md` §E):另建本地用户登录 Windows,连接
  mtdssd 管道应被拒(读不到事件、发 RPC 被拒);CI 无法覆盖,须桌面环境人工执行。
