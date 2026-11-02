# SDD ledger — plan: docs/superpowers/plans/2026-10-31-mtdss-v2p5-ios.md

Spec: docs/superpowers/specs/2026-10-31-mtdss-v2p5-ios-design.md(本 worktree 提交 2bda20f)
Worktree: .claude/worktrees/worktree-mtdss-v2p5-ios,分支 worktree-worktree-mtdss-v2p5-ios(EnterWorktree 自动前缀,与仓内惯例 worktree-* 命名等效)
基线:cargo check --workspace 后台跑(任务 buuteh64k),结果未回。

## 预检冲突扫描(表)

| 对/任务 | 产出 vs 消费 | 结论 |
|---|---|---|
| T2↔T8 | T2 产 C ABI+cfg;T8 改 home 布局(暂存/收件箱路径) | 兼容,T8 依赖 T2,顺序已定 |
| T3↔T5 | T3 产 MtdssRepository(方法按 Contract 分组);T5 消费 `.browse` 门面 | 命名粒度松——Ruling A |
| T2↔T1 | T2 Step4 的 Mac mini 编译验证依赖 T1 隧道 | Ruling B:先派 T2(Windows/WSL 面),Mac 验证步由控制者在 T1 完成后亲自跑 |
| T6↔T11 | T6 产 SendService.send(items:to:);T11 ShareConsumer 消费 | 一致 |
| T10↔T2 | T10 书签解析 → 注入授权集 | **原为缺口**——已修计划:T2 增 authorized_roots 守卫(提交 c48942b 前,见 git log),Ruling C |
| T11↔T13 | T11 真机首验需设备签名装载;TestFlight 产线在 T13 | Ruling D:T11 真机验证用 Mac mini 开发签名直装(设备 UDID 注册+插线或无线),不等 TestFlight |
| T4 EngineHost 位置 | 计划文内"或放 app 侧,二选一" | Ruling E:定死放 MTDSSKit(mock 可测),计划文字已自决 |
| T3 golden 资源接法 | "符号链接或拷贝脚本"二选一 | Ruling F:优先资源引用/符号链接;fallback 拷贝脚本必须带一致性校验测试 |
| T12↔T3 | CI swift test 需 macOS 宿主可跑(UDS/POSIX) | T3 platforms 声明 macOS 13,一致 |
| 全局约束 vs T2 | 桌面行为不变由 cargo check workspace + 既有测试套守护 | T2 测试步含此验证 |

## Rulings

- **Ruling A**:T3 的 Repository 门面方法名按 Contract 实际 RPC 名落地(list_remote 族);T5/T6/T7 派发时携带 T3 报告中的实际方法名,勿按计划示意名硬编。——避免命名粒度不匹配返工;代价:无。
- **Ruling B**:T2 派发即行(不等待 T1),实现+WSL 测试完成即报 DONE;Mac mini 侧 `cargo build --target aarch64-apple-ios` 验证由控制者待 T1 隧道通后亲自执行并记入本账——T2 完成判定含该步。——并行省时;代价:T2 完结时点后移到 T1 后。
- **Ruling C**(已修计划,commit "authorized-roots guard"):daemon cfg 增 authorized_roots,ctrl 浏览/发送读取路径守卫(规范化+前缀判定,拒集外与 `..` 逃逸),桌面不配置=行为不变;T2 实现+测试,T10 注入。——否则对端可翻整个沙箱(隐私洞);代价:桌面 cfg 面加一个可选字段。
- **Ruling D**:T11 真机首验走 Mac mini 开发签名直装(先注册两台 iPhone 的 UDID),不依赖 TestFlight 产线。——T11↔T13 解耦;代价:用户需把 iPhone 接到 Mac mini 一次。
- **Ruling E**:EngineHost 放 MTDSSKit(计划文已注明理由)。
- **Ruling F**:golden 资源优先引用仓库 crates/ipc/contract;SwiftPM 不便则拷贝脚本+一致性校验测试双保险。

## T13b 真机整测(smoke-p5.md;2026-11-01 起)

**R1(iPhone 16PM,批次1 登录)**:A1/A3 ✅。战果 8 条:
- **Ruling N(用户钦定,推翻"首页=授权目录列表"合成)**:首页/常用目录**只显示收件箱**;信任目录不出现在常用目录——只是"其他设备可浏览进来"白名单+发送授权;收件箱=默认目录(收发皆可,天然授权)。spec §3 改,T10"信任目录出现在首页"行为移除。
- #2 默认保存目录:Ruling N 模型消解(收件箱即默认);首启引导=UX 票。
- #4 收件箱未登录可看:合理——T5 登录门拆除(验证 daemon 本地 list 无需 session)。
- #5 设备页点击无反馈(冷握手 25-35s 无 loading):UX bug——即时推入+loading 态。
- #6 跨网设备打开失败「中继互通失败」:**已结案**(2026-11-01 深夜)——双根因:①unit 缺 --relay-addr(用户改+重启,套接字 127.0.0.1→0.0.0.0)②**阿里云安全组未放行 UDP 8788**(tcpdump 0 包铁证,用户补规则后跨公网通);F4 跨网中继实质验证通过。教训:此前"都加了"未实测,P4b 服务端三件套实际只做了 relay enable。
- #7 其他设备浏览 iPhone「远端路径非法」:结构性缺口——对端列根 path="" 被授权根守卫拒;引擎需在 guard 配置时把根列表合成虚拟授权目录清单。
- #8 分享扩展选设备无反应:真 bug 待诊(真机 Console 日志)。
修复波 A: complete (76b072c..29b8e59,review 四修全 CORRECT;2 Minor 并入 R2 波)。

R2 波: complete (29b8e59..cf8cd49,review APPROVE 三项全 CORRECT 零新破坏;评审员独立复跑;手势真机手感留下一轮)。构建 20261102.1 交用户 GUI 终端跑(SSH 钥匙串门未解=已知坑)。

**R3(2026-11-02,20261102.1 装机批次2)**:#6/#7 ✅(跨网互访+PC 看手机信任目录全通=波A#7+中继修复实证)。新 8 条:
- **Ruling O(用户裁定,改 §4.2 语义)**:选择模式右上蓝圈确认=**仅退出选择**,不再触发任何动作弹层;操作一律从底部按钮发起。0 选点确认同样=退出。
- #1 行点击热区=整行(现在只有文字/箭头可点);长按选择同样要全行(#5 同源)。
- #3/#6 动作零反馈:下载/发送发起后要 toast「已开始 N 项·见传输列表」+完成提示含**跳过 N 项**(同名跳过=引擎 Skip 语义,答用户重复机制疑问)。
- #4 选设备弹层加高(medium→更高档/自定义),顶部文字/取消钮离圆角太近要修 padding。
- **#8 树形展开交互(大件,用户裁定)**:蓝色箭头=**行内展开**子级(缩进、可逐级再展),行点击=进入目录整页(两种并存);历史导航语义保持(行点击侧)。
- #8(分享扩展)仍无反应:20261102.1 未动扩展代码(预期无变化);真机 Console 日志仍欠(手机暂接不了 Mac)。
- 处置:R3 波派发(1/2/3/4/5/6/8 + spec §4.2/§4.1/弹层同步改);扩展日志继续等手机能连 Mac。

R4 波: complete (3a04688..6f2f3a8,review APPROVE 5/5 零C/I;3 Minor 缓办)。
**R5(2026-11-02,20261102.3 装机批次5)**:12 条——
- R5a bug 系:②设备名改名不生效(疑 iOS login.verify 未带 AccountStore 名/或用户未重登——查代码实证);④下拉刷新报「无法加载目录·连接中断」(仅首页根正常——疑 refreshable 任务取消与 RpcGuard/IpcClient 竞态,sim DEBUG 可复现);⑤标题设备名|目录名加分隔留白;⑥搜索进下级要清空(只搜当前层=接受面);⑩退出登录确认改居中 alert(现 action sheet 在底);⑫显示隐藏开关不生效(疑 wiring/树投影漏滤)。
- R5b 功能系:①首页树选择模式(根行不可选,下级可选,···选择+长按);③设备列表显示本机(本机标注);⑦行左滑操作(本地=发送/删除,远端=下载/转发,上下文式);⑧选择模式左下加垃圾桶;⑨删除一律确认弹窗。
- ⑪相册权限+自动信任:**用户裁定方案 A**(2026-11-02)——主 App 增「照片图库」特殊来源:首用弹权限;首页/设置出现入口;按时间/相册浏览+多选发送(PHPicker/PHAsset 数据面);信任目录机制不动,照片库=并列特殊来源。排 R5c(R5b 后)。
R5a: complete (5dfa578..8911b5a,review APPROVE 全五 CORRECT;#4 转圈疑虑专查排除[Task.value 等待保真];#2 零改动调查三环实证=改名需重登属设计面;2 Minor 缓办:取消窗口路径 title 先进/Devices 无 seq 守卫[先在])。

R5b: complete (8911b5a..774aa5e,review PASS 零C/I;3 Minor 缓办:spec §4.2 旧句残留/空态文案不可达/本机行投影窗;发现:daemon devices 恒含本机=服务端全列透传,去滤零合成)。

R5c: complete (774aa5e..r5c-docs,报告 task-13b-r5c-report.md;Kit 172/172 + app 构建绿 + 五帧截图 + sim→Windows 照片 E2E md5 全对)。方案 A 全交付:Kit PhotoLibraryService 协议+PhotoLibraryViewModel(权限状态机/选择 Ruling O/发送=T11 管线 导出→暂存收件箱→SendService→终态 done 清暂存)+App PHPhotoLibraryService 绑定+权限门/相册/网格/选择 UI+首页第二固定行+设置权限段;真机面(滑动/长按手感、真机照片库)留真机轮,见报告 §Concerns。
R5b 波: complete (8911b5a..r5b-docs,报告 task-13b-r5b-report.md;Kit 155/155 + sim 构建绿 + 五帧截图)。#1 首页选择=BrowseViewModel 复用零新状态(顶栏/底栏随 VM 态在 FavoritesRootList 收口,根行非 entry 无圈=结构面 Kit 钉);#3 调查实证 daemon devices 恒含本机(server /devices 全账号)→去滤即得,本机行「平台 · 本机」禁点,弹层仍滤己;#7 swipeActions 需 List 而列表是 ScrollView+LazyVStack → T9 SwipeableRow 通用化迁 Common(动作规格出集/展开态点行只收),Kit BrowseSwipeAction 纯函数+deletePaths 拆分;#8 远端底栏 下载/转发/删除(deleteSelected 按 vm.deviceId=对端,Kit 钉帧 device=对端);#9 全 App 删除路径居中 alert 统一(选择/滑删/信任目录移除,dialog 归零)。真机面(滑动吸附手感/展开态点行收口/首页长按进选择)留真机复测,见报告 §Concerns。
R5a 波: complete (5dfa578..8e47116,报告 task-13b-r5a-report.md;Kit 151/151 + sim 构建绿)。#2 链路实证零改动(下次登录生效=设计面,server/daemon/iOS 四环全绿);#4 根因=refreshable 任务被 SwiftUI 取消→VM 兜底文案伪装「连接中断」并清列表(两层修:VM 良性取消收口+三面 refreshable 非结构化 Task 承载=与「···」同语义;真机触发器留复测);#5 标题「设备名 | 目录名」留白;#6 open/back/forward 清搜索(reload 保留);#10 退出确认居中 alert;#12 根因=远端浏览页完全没接 showHidden(补双面接线;Kit 投影无缺口,双层级钉测)。真机复测口径见报告 §4。
**事故 #2(已透明)**:实现者 fifo 二义误登录产线实例→session 覆写+产线 daemon 连杀;已复原(服务器零污染/daemon 1.1.0 已拉起);**用户动作:Windows MTDSS 重登一次**。教训加重:多实例同机必须先验管道指纹。
波 A 缓办:RpcGuard 30s vs 冷握手 25-35s 残余(复现则票);sim defaults DEBUG 钩子直写小票;C:\mtdss-t13b-a 留用户手删。
排程:评审过 → R2 右滑退回小修 → build 20261102.1 + 上传 TestFlight → 用户装机复测(R1 四修+继续 B-E 批次)。
- **R2 增补(用户)**:浏览页支持**右滑(边缘)退回上一级**(与左上返回箭头同路);入修复单(波 A 收尾后追加或随波合入)。

## 未清票池(T12/T13 派发携带)

真机就绪(2026-11-01):开发者模式开+配对过 Mac mini——iPhone 16 Pro Max(iPhone17,2)connected,CoreDevice id D3F33FBE-D993-587C-A755-8B98DCC24857;UDID 注册随 T13 设备签名时从 Xcode 取。

- **T12 携带**:ios-bridge mtdss_ios_ipc_path clippy not_unsafe_ptr_arg_deref ×2(T2 基线遗留,本地 clippy 1.99 触发)——CI clippy 门要绿必须修
- T13 携带:真机分享面板验证(T11 Ruling M)+ 通知横幅/左滑/scenePhase 真机取证 + isIdleTimerDisabled 熄屏验证 + sim bytes_done 读数复测 + T10-minor4 迁移路径分叉 + bridge 源重发拒(ios ReinitiateRouter 票)
- 后续轮票:Android clear-done UI 对齐;daemon_e2e restart_marks_interrupted 预算放宽(flake 家族);list 慢共享懒计数;IPC self 面过 guard 收紧;改名即时生效 server 面;build.sh 预建 Documents
- 纪律:proto 增量=消息包裹/缺省值手法(protoc 3.12 红线);杀 dev daemon 按可执行路径匹配

## Task 状态

基线:cargo check --workspace 绿(exit 0,worktree 首编)。
Task 1: complete (docs/dev-mac-mini.md f570039;全链路验证:隧道/工具链/protoc/iOS 26.5 平台包/T2 交叉编译全过;两 remote 已同步)。
Task 2: complete (commits aee84ae..82fd01d, review clean 4 minor 缓办,Mac 交叉编译 BUILD_EXIT=0 libios_bridge.a 60MB 产出)。
Task 3: complete (commits 82fd01d..1bb39bc,review clean;5 Minor 缓办:actor 内阻塞 write/connect、Task-per-line 顺序论题、事件缓冲丢旧 vs Kotlin 丢新、close 测试未钉错误码、Goldens 注释措辞;**Ruling G**:brief「坏帧断连」与 Rust 真源冲突,裁为跳帧不断连(实现即此,带测试)。⚠️ 采信:swift test 13/13 Mac 绿(证据链)。携带:pointer=UDS 路径 sun_path 104 上限,iOS 容器路径 ≈103 贴边(T4/T10 观察项)。

事件记录:2026-10-31 深夜 API 限速第一轮(周/月上限)杀掉 T4 评审员半途与 T4 实现者遗留后台(工作已交付无损);2026-11-01 用户解限恢复。2026-11-02 晚第二轮(5 小时窗)杀掉 R5c 实现者半途(网格取消问题排查中);用户解限后恢复续跑。

Task 4: complete (commits f570039..19c7f2e,review Approved 零C/I;3 Minor 缓办:pollHello deadline 近似、sim udid 硬编码探测、标签题缩写[T5 已携带修复])。⚠️ 采信:Mac 侧执行证据链;真机路径贴边断言留 T13 真机验。

Task 5: complete (commits 19c7f2e..9d8bcf5,review Approved;3 Minor 缓办:canBack 死参数、visibleEntries 每渲染 3×全量排序(规模小无感)、可达性 label(T13 打磨))。

Task 6: complete (commits 9d8bcf5..573727a,review Approved 0C/I;5 Minor 缓办:more 圆点禁用灰显[T7 顺带]、未选圈白底暗色刺眼(现阶段全亮色一致)、transferActive 前台重对账[T9 顺带]、report 计数笔误、delete 双端缺 golden[**T5b 票池+1**])。

Task 7: complete (commits 573727a..3103ab9,review Approved 0C/I;4 Minor 缓办:30s 超时本地/远端不分档、回标签不重拉滞留在线态、sorted 稳定性注释、未知平台文案 Nit)。

Task 8: complete (commits 3103ab9..7b9d114,review Approved Spec 8/8;3 Minor 缓办:jobs.rs 陈旧注释、FdPass 104 双写常数、退化入参两侧分歧[生产不可达])。

Task 9: complete (commits 7b9d114..d06d191,review Approved;2 Minor 缓办:测试类头注释陈旧、onChange 废弃单参形式)。

Task 10: complete (commits d06d191..2d20295,review Approved 全✅;4 Minor 缓办带票:重授权按全结构去重[同路径可重复]、restartEngine start 失败会话内不恢复、prune 不释放 access、备份迁移路径分叉[T11/T13 验证带];报告计数笔误 102/17)。

Task 10b: complete (commits 2d20295..1bdf097 + 批准回填 2 提交;review Approved——产线安全[handle 路由互斥无遮蔽/备份在/三服务活]、12+ 行主张实证、图标全验证;1 Minor:信令路由披露句已随批准补入)。**隐私政策已正式批准:生效日期 2026-11-01,线上/仓库同步,TestFlight 硬门槛关闭**(联系邮箱=13160659024@163.com,用户定)。

Task 5b: complete (commits a56d69f..9574513+1519642,review Approved 四件全✅ 零C/I;评审员本机实跑复验三面[core fs 24/24/daemon 单测/Kotlin 258];3 Minor 全已带票)。

Task 11: complete (commits 1519642..1376d16 含修复轮;review Approved+2 Important→修复轮1 双 ADDRESSED[取消感知 CallbackAwait+MainActor 归位,126/126];3 新 Minor 缓办:回调先到微窗值损/测试名半钉/enqueue 主线程同步 I/O)。

Task 12: complete (commits 1376d16..05148df,review Approved;4 Minor 全文档级[7 处 unsafe 非 6/行数 284 非 277/37→39 项/smoke A1A4 引用了不存在的 UI 文案「连接引擎中」T13 执行时改述];run 41280614030 六 job 绿实证)。

Task 13(分段执行):T13a **complete**——签名链三关全过:①team ID=D4L57M3M67(altool --list-providers @env 绕钥匙串 SSH 不可见问题);②证书吊销重发=用户 Xcode GUI 一次性修(GUI 会话钥匙串与 SSH 互不可见的 macOS 坑两遇:set-key/partition 待票或写入 runbook);③ASC App 记录=用户建(套装 ID 随签名注册后出现;首次"上传完成"实为无记录失败,补传 `altool --upload-app -f .ipa` **No errors uploading**,Delivery 4863636b,1.1.0(20261101.1));build.sh upload 档与 device-sign 档全链可用;runbook docs/testflight-runbook.md 在库。T13b 真机整测:用户装 TestFlight 双机后开跑(smoke-p5.md 39 项)。
T12 新票:nat-matrix 信令 SQLITE_BUSY 偶发(main 近 8 跑 5 红含 docs-only,transfer_v2 时序 3 例同族)→ 独立票(signaling db busy_timeout/重试)。
T10b 关键事实(给用户审的摘要):验证码=HTTPS 应用内回显(服务器无 SMTP,SHA-256 哈希存 10 分钟);E2E 主张真实(直连 QUIC TLS1.3 互钉 SPKI/中继只转发密文/信令 sealed-box);中继元数据=IP/端口/大小/月流量;在线状态纯内存;删除=admin CLI 级联。占位:privacy@mtdss.top 邮箱未开通(用户确认或换);生效日期待批准。
**事故记录(已向用户透明)**:T10 收尾按进程名杀 dev daemon 误重启用户产线 mtdssd 一次(自愈 ~1s,会话完好,现 pid 11752);教训=以后按可执行路径匹配杀。
T10 票池候选:IPC self 面不过 guard(收紧);改名即时生效需 server 面;build.sh 不预建 Documents。
T10 缓办:macOS 13→14 地板(Observation);iOS 26 无 withSecurityScope→plain bookmark+startAccessing 持有;fileImporter 真点选 T11/T13。
**Ruling K**:引擎 transfer.clear 拒清 done 与 spec「清除已完成」冲突 → **T5b 追加:引擎侧放开 done 清除**(daemon 行为松绑,proto 不动);过渡期 UI=done 行无动作、菜单清非完成态。T5b 票池四件:list 子项数、unix 收端哨兵回退、delete golden、**clear-done 放开**。
T9 缓办/携带:通知横幅视觉+左滑手势+scenePhase 真机取证 T13;bridge 源重发引擎拒→T11 票候选(iOS 版 ReinitiateRouter 或接受拒发横幅);dev home 两处残留记录。
T8 缓办/携带:sim 重发 bytes_done 读数差(T13 真机复测);无窗 sim 后台 ~1min 自退场(T9/T13 留意);产线多一台「iOS 模拟器」离线残项(无害)。
**Ruling J**:T5b 采用**引擎侧哨兵统一**(unix 收端无 App 应答回退 recv_dir)而非契约加 peer recv_dir——惠及全部发送方、零协议变更、零客户端返工;v1 期间 iOS 维持拒发文案。T5b 票池三件:list 子项数、unix 收端哨兵回退、delete golden。
T7 缓办/携带:最近时间无字段→仅在线/离线(不编造);远端浏览移动对端=v1 不可(roots guard,与安卓同);超时 30s 仅远端;smoke 脚本正斜杠。
**Ruling I**:「inbox 哨兵对 unix 桌面对端超时」→ 客户端侧解决裁归 **T7**(SendService 按对端平台:移动=哨兵/桌面=devices 携带的绝对 recv_dir,+测试);引擎侧哨兵统一并入 **T5b**。代价若错:对 Linux/macOS 用户发送失败的 UX 缺口。
T6 缓办/携带:isIdleTimerDisabled 真机验证 T13;远端动作 UI 挂接 T7(ActionKind 点已留);T6-Windows-A/T6-WSL-B 离线残项(无害);DEBUG 钩子报备。
**Ruling H**:spec §4.1「日期 - N 项」与「协议零改动」约束正面冲突(list 契约无子项数)→ 裁:**T7 后插 T5b 专项**(list 增可选 children 计数,proto+daemon+golden REGEN+Kotlin 断言+Swift 四面一次做,v1 交付前完成);期间目录副文纯日期。代价若错:一次契约增量返工。
T5 缓办/携带:模拟器交互态=VM 测试覆盖,真机交互 T6 顺手过;未登录占位(T10);Documents 真位 T13;RpcGuard.withTimeout 新公共面(T6+ 复用);AuthorizedRoots 建模已备 T10 注入。
T4 关键发现(携带):①sim 容器路径>170B 超 sun_path 104→DEBUG MTDSS_HOME 短路径绕开,真机 ≈100B 贴边(**T8/T10 盯**);②引擎 slice 分 sim/device(aarch64-apple-ios-sim / aarch64-apple-ios)不可混链;③xcodegen *.a 只落 -F 须配 LIBRARY_SEARCH_PATHS;④macOS nc 无 -N。
T4 缓办:home=Documents 过渡布局(session.json/daemon.sock 对文件 App 可见→T8 收整);hello 心跳监督只做雏形(完整版归 UI 任务);build.sh device 仅编译(T13);MTDSSShare 空壳(T12)。

T3 缓办 Minor(终审triage):
1. 无 auto-heal 重连环(安卓有;进程内 UDS 场景弱需求,懒重连已备)
2. 被丢帧的调用永久 pending(同 Kotlin 语义)→ **T4/T5 派发携带**:view model 层 send 加超时包裹

T2 缓办 Minor(终审triage用):
1. ios-bridge mtdss_ios_ipc_path 错误码 -1/-2 语义混同(null 与非UTF-8合一,-2 实际不可达)
2. extern"C" 内 engine_slot().lock().unwrap()——毒锁会 abort(实际不可达,防御性)
3. **set_recv_dir 在 guard 配置下未拒绝**——iOS UI 若调它会把收件箱挪出授权集(可用性陷阱非安全洞)→ **T4 派发必须携带**:guard 启用时拒绝 set_recv_dir 或联动扩展 roots
4. roots.rs Windows 分支注释说 ASCII fold 实为 Unicode casefold(注释级)

T2 ⚠️ Cannot-verify 裁定:①WSL 测试证据链(实现者报告+评审者Windows子集复跑)→采信;②Mac 交叉编译→控制者执行中(工具链安装后台 brr646a5f);③TOCTOU symlink 残留→维持 T10 边界裁定,实现者已文档化。

T1 进展(控制者执行):Mac mini 物理安装完成(用户)→VPS 公钥配置✓→隧道 2222 上听✓→Windows `ssh mac` 全链路通(arm64/macOS 26.3/Xcode/cargo/autossh✓,brew 绝对路径可用)→xcodegen+rust iOS target 安装后台中(brr646a5f)→分支推 origin✓;Mac 直连 GitHub 超时→改隧道内推:Mac bare 中转 ~/mtdss-src.git(remote macmini)+工作检出 ~/mtdss-ios(同步配方:push macmini→fetch origin→reset --hard)✓。T1 工具链补齐:xcodegen 2.46✓、rust aarch64-apple-ios✓;xcode-select 指向 CLT 的坑以 `DEVELOPER_DIR` 绕开(~/.mtdss-env.sh 持久,无需 sudo;永久修需用户 `sudo xcode-select -s /Applications/Xcode.app/Contents/Developer`,记入 T1 文档待办);Xcode 26.6 验证✓;模拟器 runtime 0 → 后台下载中。**Mac 后台任务**(~/t2-ios-build.log→~/ios-platform.log 串行):①T2 交叉编译 libios_bridge.a ②downloadPlatform iOS。

Task 3 dispatched @BASE 82fd01d(实现者 sonnet;曾一次非用户意图中断,恢复后续完)——状态见上方 Task 3 行。
