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
- [ ] 右键菜单四项：打开主窗口（置灰）、登录/登出（动态文案，置灰）、设置 ▸ 开机自启（勾选态）、退出
- [ ] 退出优雅：托盘「退出」→ 图标消失、`daemon.lock` 文件已删除、进程收干净（此后 `mtdss` 命令报 daemon 缺失并可重新拉起）
- [ ] 开机自启开关真实生效：勾选后 HKCU `Software\Microsoft\Windows\CurrentVersion\Run` 出现 `MTDSS` = 带引号的 mtdssd.exe 全路径；重启资源管理器/重登录后 mtdssd 自启；取消勾选后该值删除（注：受管机器若 EDR 拦截 Run 键写入，RPC 会业务失败——属预期降级）

## M3a 手动冒烟（收尾终检）

M3a 全部任务落地后的整体人工冒烟清单，发布前逐项勾选归档：

- [ ] 托盘亮/灰随登录态：`mtdss login` 后亮色，`mtdss logout` 后灰色
- [ ] 托盘右键四项：打开主窗口、登录/登出（动态文案，按登录态置灰）、设置 ▸ 开机自启（勾选态）、退出
- [ ] 托盘退出优雅：点「退出」后图标消失、`daemon.lock` 已清除、`mtdss status` 报 daemon 缺失且可重新拉起
- [ ] 开机自启开关真实生效：HKCU Run 键写入/删除并重启后自启（注：受管机 EDR 拦写属预期降级，需在常规 Windows 桌面验证）
- [ ] CLI 自动拉起 daemon：冷启环境（无 daemon 在跑）直接 `mtdss status`，应自动拉起 mtdssd 并返回状态
- [ ] UI 壳交互（`npm run tauri dev`）：连接 daemon、登录/登出联动状态、设备列表刷新
- [ ] UI 冷启：直接启动主窗口，应自动拉起 daemon 并在数秒内出现设备列表
