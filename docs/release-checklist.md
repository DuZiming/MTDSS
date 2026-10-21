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
- [x] 托盘联动:D1(托盘登出→UI 引导态)/D4(CLI 登出联动)沿 M3c 已验;**D2/D3(托盘唤起)已随 M4a 补验(2026-10-21,release 布局)**——首验发现最小化后「打开主窗口」唤不回+双击托盘图标无反应,fix wave 修(single-instance 回调补 unminimize+托盘接 TrayIconEvent/DoubleClick,左键单击改无操作、右键菜单不变,72f2652),复验过(含双击);D2「登录」表单唤起/表单登录转亮亦过(登录表单另增 CA 证书选择器,271772a,见下「M4a 手动冒烟」段)
- [x] SDDL 跨用户:**E1–E4 全过(2026-10-21,M4a 补验)**。等效自动化执行(裁决:验证对象=DACL 拒绝行为,方法等价):New-LocalUser 建临时用户+`Start-Process -Credential` 跑管道连接探针——跨用户连 mtdssd 管道 30ms `ACCESS_DENIED`(0x80070005),反向对照连不存在管道为 `TimeoutException`(3s 满,证明拒绝源自 DACL 而非环境);事件广播不可读、RPC 不可发;临时用户与探针文件已清理(步骤见手册 §E,对应下「已知安全备注」末项)

**冒烟发现(记 M3c/优化清单,非阻断)**:
1. 设备改名(重登换名)对其他在线设备的目录快照不可见——hub 仅自身重连/未知设备触发全量刷新,presence 翻转只就地改标志;`ls` 按名匹配与 UI 树直到本端 daemon 重启才收敛(A5 冒烟时实测)。候选修法:`devices` RPC 现拉 REST 或 hub 低频周期刷新。
2. UI 打磨项(用户反馈):左树加载偏慢;树节点文字/列宽超界;三视图样式待优化;Shift 范围选中时行文字蓝底观感差。
3. dev 模式下 UI 由 `npm run tauri dev` 拉起时托盘唤起路径可用但未验(见上 D3 待补)。

## M3c 手动冒烟(2026-10-20 执行,主体通过)

冒烟环境:**公网真机拓扑**(VPS 信令+STUN / 公司 Windows(操作端,UI)/ 家 Linux(无头,root)/ 第三设备)——强于原计划的本机回环;手册 `docs/smoke-m3c.md`。**冒烟发现并当日修复 1 个产品 bug**:跨平台远端目录校验(Windows put 到 Linux `/home/…` 被本机规则拒、Linux 源端回拒 Windows download 落盘目录 `G:\…`)——34a2d9b 平台无关绝对形态预检,双端更新后推/拉全通。核心项结果:

- [x] CLI put/get:定向落盘、`--conflict ask` 问询(s/o/c 全生效;o 首测误报,复测覆盖确认)、skip/overwrite 直发、单/多文件(嵌套同名自动改名安全网未单测,引擎级已覆盖)(A1–A4)
- [x] CLI cp 四态:download/upload 糖、A→B 三真机中转、混双远端源正确拒答;本机↔本机拒答未单测(同校验路径已覆盖)(A5)
- [x] CLI rm/mv:单/多文件删除、跨设备混删正确拒、mv 跨设备指引 cp、同设备移动 ✓(重命名不做=V1 非目标);卷根与 `.mtdss` 拒(A10–A12 过);`--permanent`/回收不可用梯未构造(Windows 回环 SKIP,公网 Linux 桌面可补)(A6–A9、A12)
- [x] UI 冲突对话框:单条两键、批量四键、目录恒合并不弹框(设计);类型不匹配行注/Esc/预扫中止未单测(B1–B6 主体)
- [x] UI 粘贴与拖拽:Explorer→MTDSS、远端→另远端/本机/同设备、窗内拖到树、OS 拖入、「下载到本机…」选择器直落(C1–C8)
- [x] 拖出矩阵:桌面 ✓、普通资源管理器窗口 ✓(spike 结论窗口落点留 V1,真机实证);库/此电脑提示条未单测;拖拽源保持选中蓝底=Explorer 同款(设计);全程不交付数据(D1–D2)
- [x] 传输面板与控制:暂停/继续/取消、排队间隙暂停(先发起的暂停→后发先行→完成后继续)真机复现 E9 语义、状态栏速率与 N 计数(E1–E7、E9)
- [x] 托盘 tooltip 计数:状态栏 N 计数已验,托盘 tooltip 同 CountWatch 数据源(未单独目检)(E8)
- [x] 通知:Windows 系统通知 ✓;Linux 无头侧按设计仅日志(root/无 D-Bus 总线,实测符合「headless 只记日志」)(F1–F3;Linux 图形通知未测,留桌面环境补)
- [x] 删除与回收:UI Del 静默回收 ✓;离线文案/部分失败提示未单测(G1–G2)
- [x] 携带项回归:C:foo 拒 ✓、`.MTDSS` 滤除 ✓、登出→UI 引导态 ✓、树 spinner/加载态 ✓;登出拆卸 ctrl 与 devices focus 重拉未单测(H 主体)

**M3b 遗留补验情况**:D1(托盘登出→UI 引导态)✓ 本次过;**D2/D3(托盘唤起)根因已定位未收口**——daemon 解析序 debug 优先,debug ui.exe 依赖 vite dev server,独立场景须 release ui.exe 置于 mtdssd 同目录或 `MTDSS_UI_BIN`(即 M4 打包部署形态),随 M4 打包一并验证;D4(CLI 登出联动)沿 M3b 已验路径;**SDDL 跨用户 §E 仍未验,M4 打包前必须**(`docs/smoke-m3b.md` §E)。

**EOF 2s 竞速重估(Task 14 Step 1)**:维持 2s 宽限仍充分,不立项修。终局/拒绝两族路径在定向+策略+暂停新流下均未新增 EOF 依赖:①终局——收端仅在 checkpoint 臂(`if !paused_now`)发 `[0x03]`,暂停期间该臂整体禁用,EOF 舞步在结构上不可能始于暂停态;发端收到 Final 后无任何门直通 `ctl_tx.finish()`,EOF 立即到来。②拒绝——策略经 PLAN 帧落地(命中策略不发 REJECT),`reject_exit`/`reject_with_stop` 与字段零改动。③暂停中段——双侧共享 PauseGate 只停新块,keep-alive 10s(< idle 30s)保活连接,轮内不断链即不触舞步;断链/取消走 `conn.close`/任务 abort,`wait_peer_eof_grace` 的 read 立即出错返回(不等满 2s),且有界化保证最坏 2s 干净退出。④download 取消经 ctrl cancel_job 两侧同 abort(`run_send_with` 重试循环一并杀,不复活),无「收端已放行、发端还在重试」的错位窗口。结论:2s 宽限在新流下仍覆盖全部在途帧投递窗口,无实质竞速。

**M3c 后置项(deferred,记录依据)**:
- CSP 硬化——M4 打包前
- 设备重命名——需服务器端点
- 接收端并发多传输——同向串行,~50s Accept 窗口限制已记(第二传窗外失败可重试)
- 远端源嵌套冲突全粒度预扫——自动改名 `name (1)` 安全网兜底
- 跨会话断点续传——V2
- `mtdss send` 遗留 RPC 保留未迁移 jobs——e2e/nat-matrix 兼容面

## M4a 手动冒烟(2026-10-21 执行,通过)

冒烟环境:本机回环服务端 + 双 MTDSS_HOME(home_a 托盘/home_b 无头);托盘唤起/退出项在 **release 布局**(ui.exe 与 mtdssd.exe 同目录,daemon 解析序命中)执行——偿 M3b「release ui.exe 须与 mtdssd 同目录」的遗留前置;手册沿 `docs/smoke-m3b.md` §D/§E + Task 11 手工清单。**首验发现并当日修复 4 个缺陷(A–D,两波 fix wave,均复验过)**:A=最小化后托盘唤不回+双击无反应(72f2652:single-instance 补 unminimize+托盘接 TrayIconEvent/DoubleClick,左键单击改无操作、右键菜单不变);B=UI 登录表单无法指定 CA 证书(271772a:表单新增 CA 证书选择器);C=同设备粘贴同名不弹冲突框(06fbc17:根因 pastePlan 本机→本机误路由 local_copy 无策略面致预扫被跳过,改路由 device_copy 走统一预扫+对话框);D=托盘退出只退 daemon、ui.exe 孤儿残留(75681ca:daemon shutdown 首动作广播 `event.quit`+100ms flush,UI 收帧自闭;崩溃不经 shutdown 无帧=结构性只走优雅路径)。核心项结果:

- [x] 托盘唤起全项(release 布局):无窗「打开主窗口」新开、最小化后唤回+聚焦、ui.exe 单进程、双击托盘图标唤起(A 修后复验含双击);托盘登出转灰+主窗回引导态、「登录」唤起表单+CA 选择器登录转亮(D2/D3 收口,见「M3b 手动冒烟」段)
- [x] 托盘退出关双侧(D 终验):托盘「退出」→ daemon 优雅退出**且 UI 窗同闭**(`event.quit`;Ctrl-C 同路径 by design);**硬杀 mtdssd → UI 不退**,保 M3b 离线态+自愈拉起 daemon(崩溃边界成立;自愈快于离线横幅=M3a 既有设计,非缺陷,不立债)
- [x] 同设备粘贴冲突对话框(C 复验):文件级粘贴弹冲突对话框(单条两键/批量四键),跳过/覆盖/取消/剪切(move)全过;**目录对目录仍不弹框+自动改名=保留已披露 V1 限制**(`docs/smoke-m3c.md` §0 口径;M4a 只承诺文件级对齐,产品层后续统一)
- [x] UI 登录 CA 证书选择器(B,新增):「CA 证书(可选)」选 PEM 后 UI 直接登录自签/dev 服务器(此前须 CLI 绕行);× 清空/OS 选择器取消不破坏既有状态
- [x] CSP dev 实测(Task 10 落地面):CDP 旁听 dev 窗口 45s,0 条 CSP violation、0 error;改 `App.tsx` 触发 HMR 热更后复测仍 0 violation(devCsp 保 HMR 生效)
- [x] ConflictDialog 键盘焦点环:决策后 Tab/Shift+Tab 在对话框按钮间循环、焦点不逃出对话框;Esc/遮罩语义不变

**M4a 后置项(deferred/minor;台账在 `.superpowers` 下,git-ignored 不入库,要点列出)**:
- 同设备复制**无作业卡片**(与远端 device_copy 同口径;同步走 ~35s browse 超时口径,超时仅报错提示、daemon 侧实际继续)
- 目录对目录粘贴不弹框+自动改名(V1 已披露限制,见上)
- 托盘双击任意键(含中键)均唤起主窗(有意行为;左键单击无操作)
- 同版本随发语义:UI/CLI 对老 daemon 失 kind 捷径(确认升级/预扫捷径;设计已裁定,已知语义)

## 已知安全备注（Windows IPC ACL）

- [x] 管道 DACL 收紧已落地（M3b,`crates/ipc` `serve`）:经 interprocess 2.4.6
  的 `ListenerOptionsExt::security_descriptor` 挂显式 SDDL
  `D:P(A;;GA;;;{当前用户SID})(A;;GA;;;SY)(A;;GA;;;BA)`——仅当前用户 /
  SYSTEM / Administrators 拥有全部访问权,替代 `CreateNamedPipe` 默认 DACL
  对 Everyone/匿名只读的放行(此前同机他用户可被动接收事件广播:
  `event.devices` 设备名/在线态、`event.send_log` 计数;无文件路径、无凭据、
  不可发 RPC;凭据从不经 IPC 广播)。SID 查询/转换失败回退默认 DACL 并留痕,
  不因收紧失败破坏 IPC。
- [x] 跨用户人工验证(Task 11 冒烟,步骤见 `docs/smoke-m3b.md` §E):另建本地用户登录 Windows,连接
  mtdssd 管道应被拒(读不到事件、发 RPC 被拒);CI 无法覆盖,须桌面环境人工执行。
  **已于 M4a 补验(2026-10-21)**:等效自动化(临时用户+`Start-Process -Credential` 探针,30ms ACCESS_DENIED+不存在管道 TimeoutException 反向对照),E1–E4 全过——记录见「M3b 手动冒烟」段。
