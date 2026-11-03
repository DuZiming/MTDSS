# T13b 修复波 A 报告(smoke 真机首轮 #1/#4/#5/#7)

状态:**四修全交付,全量绿** / 基线 76b072c → HEAD 0b76f0c(6 commits)
执行日期:2026-11-01~11-02。执行面:Windows(cargo check/suites)+ WSL(daemon 全量,e2e 红绿)+ Mac mini(swift test + sim 实机截图)。

## 1. Commits(顺序)

| # | commit | 修 |
|---|---|---|
| 1 | `21a8ea5` fix(ios): 首页/常用目录只显收件箱,信任目录退居设置页(Ruling N) | #1 |
| 2 | `277aa81` fix(engine,ios): 收件箱未登录可看——本机保留别名 self 短路不要求会话 | #4 |
| 3 | `b3e5ac3` fix(ios): 设备页点按即时反馈+loading 态钉面 | #5 |
| 4 | `d80f5c2` feat(engine): 守卫对端列根合成虚拟授权根列表 | #7 |
| 5 | `af65893` fix(ios): loading 时序测字符串转义修复(heredoc 落盘损坏) | #5 测试 |
| 6 | `0b76f0c` test(daemon): 别名 self 未登录 delete 面补钉(自审) | #4 测试 |

## 2. 逐修对账

### 修#1(Ruling N)首页只显收件箱
- `AuthorizedRoot.roots(recvDir:)` 去 `trusted` 参数,收件箱单枚;FavoritesView 去 TrustedRootsStore 依赖;TrustedRootsStore 文档改「展示/管理面只在设置页」。
- spec §3「授权目录集」行 + §4.1 注(信任目录档撤销)按钦定文案改写;smoke-p5.md B2 预期同步(首页不出现该目录)。
- Kit 测试改 `testAuthorizedRootsInboxOnly`(收件箱单枚/nil/空串)。

### 修#4 收件箱未登录可看
- **实证**:daemon api list 的本机短路比对 `state.self_device_id()`,未登录为 None → 本地浏览原本同样要登录(任务假设的「本地不要求 session」不成立,已修)。
- 修法:list/stat/delete 三臂增保留别名 `"self"`(真实 device_id = 服务端 SHA-256 十六进制 32 位,无撞面;已登录真实 id 短路不变;move/copy/orchestrate 留真实 id 面)。iOS 推入页登录门拆除:`deviceId = 真实 id ?? "self"`(`BrowseViewModel.localBrowseDeviceId` + `Contract.selfDevice`),占位文案改「引擎未就绪」。
- 测试:daemon_basic 别名 list/stat/**delete** 未登录可达 + 非别名仍「未登录」拒;Kit 三测(参数解析/别名帧装载/远端帧原样)。
- **sim 实证**:未登录 sim 收件箱浏览全链装载(截图 `artifacts/mtdss-t13b-inbox-loggedout.png`,6 项真实内容经 device="self" 往返)。

### 修#5 设备页即时反馈+loading
- 代码面核查:点行→`navPath.append` 同步推入、BrowseView listArea 在飞 ProgressView **均已在位**(T7);真机观感缺口 = `.plain` 行零按压反馈。补 `PressableRowStyle`(按压 opacity 0.6,文件 App 惯例)于设备行/常用目录行;离线禁用门不动。
- Kit 新增 loading 翻转时序测(伪 daemon 400ms 迟到应答:在飞 loading=true+列表空→应答收口;reload 同口径)。
- sim 截图(`artifacts/mtdss-t13b-remote-loading-push.png`):推入即时成立——设备标签仍高亮而远端浏览页已推入在飞;终态帧 `-error.png`(punch 不可达时 错误+重试 呈现)。ProgressView 居中位被通知授权弹窗遮挡(sim TCC 无法预授,真机无此弹窗面)。

### 修#7(结构性)守卫对端虚拟根列表
- **实证红**(WSL,修前 b3e5ac3+测试文件):守卫对端列根 `""` → `INVALID_PATH 路径非法`(即真机「远端路径非法」)。
- 修法:授权形态 List 空路径不再按集外拒——`RootGuard::roots_listing()` 把授权根自身列成目录条目(name=末段真实目录名、path=规范化根可下钻、children 按 T5b 计、metadata 失败兜底 (0,0));根条目路径 stat 直达(守卫放行根自身);集内下钻/集外拒绝语义不变;**桌面默认(None)走既有卷列表逐字节不变**(ctrl 单测钉 unix `["/"]`)。core fs `child_count` 转 pub 供合成条目沿用 T5b。
- 测试:ctrl 单测重写(集外/逃逸拒 + 合成根恰为 [docs, trusted] 含 children + 根 stat + 未配置卷列表不变);WSL e2e `guarded_peer_root_listing_shows_authorized_roots`(A 未配置列根=卷列表 / B 守卫列根=恰 [Documents] children=2 / 根 stat / 根内下钻 / home 根拒)。

## 3. 验证矩阵(全绿)

| 面 | 结果 |
|---|---|
| Windows `cargo check --workspace` | 绿(0 warning) |
| Windows daemon suites(lib 96 / daemon_basic 7 / contract_golden 1 / tray_logic 14,`MTDSS_UPDATE_URL` 已 unset) | 全绿 |
| **WSL daemon 全量**(d80f5c2,~/mtdss-ci) | **lib 91 + 集成全目标绿零失败**(ctrl_e2e 17/daemon_e2e 25/inbox_layout_e2e 5[+新 1]/jobs 21/relay 13/…);新 e2e 红先(guard 拒)+绿后 |
| Mac `swift test`(af65893) | **130/130 零失败** |
| sim 截图 | 首页收件箱单枚(未登录)/未登录收件箱装载/远端浏览即时推入+在飞终态三帧 |

## 4. Concerns(按重要性)

1. **⚠️ 本会话误伤产线 daemon 已修复,留一项用户动作**:E2E 编排时经命名管道驱动「本机 daemon」登录本地测试信令,误中**用户产线实例**(`%LOCALAPPDATA%\MTDSS\mtdssd.exe`,home `~/.mtdss`)——产线 `session.json` 被覆写为指向本地测试服。已完整修复:经产品自身 `login.logout` 清除(身份 seed 未动,device_id c87f79e0… 不变;**生产服务器 devices 表从未被写**——登录写的是本地测试库),产线 mtdssd(1.1.0)已重启运行、处于**已登出**态;另 `taskkill` 曾连带杀掉产线进程(已随重启恢复)。**待用户:打开 MTDSS 重新登录一次账号即可**。dev 残留:登录驱动脚本误判成因 = Windows 下同名管道二义(本会话已改按管道名精确指认)。
2. **未删的临时目录**:`C:\mtdss-t13b-a`(dev 对端 daemon home:测试 daemon home/一个测试文件/日志;安全检查拦 rm,留用户手删)。
3. **登录链路验证注记**:本会话一度疑似「产线登录坏」——实为编排脚本 fifo 回填验证码损坏(RPC 直驱同链路即成功,产线 request-code/verify 实测正常)。产品代码零嫌疑,无票。
4. **修#5 残余观感**:远端首拉 RpcGuard 上限 30s,而 T7 实测冷握手 25-35s——真机冷握手若 >30s,loading 后落「请求超时+重试」(重试乘已建连接通常即成)。本次未动超时口径(非本波范围);若真机复测仍见,建议票:首拉超时与握手预算对齐或 punch 后台续跑。
5. **sim 残留态**:通知授权弹窗遮 loading 中心(simctl privacy 预授被拒,TCC 直写未生效;真机无此面);sim 引擎现处已登出态(本地测试服已撤);`~/.mtdss-ios-sim` 下次 `build.sh sim` 自动重置。`.DS_Store` 在收件箱可见 = 该 sim 早期 T10 冒烟持久化了「显示隐藏项」开关,非本波回归(代码默认关,持久键在本会话前已被 DEBUG 钩子写真——该钩子「直写 defaults 不经 didSet」的注释与实况不符,污染持久开关,建议小票修钩子)。
6. **spec 联动已改**:§3/§4.1(Ruling N);smoke-p5.md B2。progress.md 的 R1 记录(控制器笔迹)未动。
7. 设备页离线行 `guard device.online` 静默吞点按(离线行本身 disabled,双保险)维持原样;真机 #5 的「连点无反馈」若在**离线行**上复现,即此守卫,属预期行为。
