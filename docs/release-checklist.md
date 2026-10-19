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

## 已知问题（V1 发布前处理）

- `.mtdss` 保留段匹配为大小写敏感——Windows 大小写不敏感卷上 `.MTDSS`
  载荷可落进隐藏收件区；V1 发布前硬化为大小写不敏感拒绝
  （`crates/cli/src/flow.rs` `walk_dir` 与 `crates/core/src/protocol/manifest.rs`
  `validate_rel_path` 两处）。

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
