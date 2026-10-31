# Task 8 报告:收件私有暂存 + 完成发布 + 文件 App 可见(v2-P5 M3,Review Focus #4)

**BASE 3103ab9 → HEAD c13232e;30 commits(含本报告);Windows check --workspace --tests 绿 + daemon lib 94 绿;WSL daemon 全目标绿(含新 inbox_layout_e2e 4 例 + 既有 inbox_staging_e2e 8 例)+ ios-bridge 3+4 绿;Mac Kit swift test 72/72 绿;模拟器实收 E2E 全过(md5 五件全对,杀停中间态收纳实证)。**

## 0. 裁定与取舍(简报问句逐条回答)

- **C ABI 不变(已选)**:`mtdss_ios_start(home)` 保持单参;**桥内派生**全部布局。Swift 侧只派生 home 传入,App 侧发布器经 Kit `EngineLayout`(与桥同一条规则)定位 fds.sock 与收件箱。
- **引擎参数化(最小面)**:`DaemonOpts` 增 `inbox_staging: Option<PathBuf>`(None = 默认 `{home}/inbox-staging`,桌面/Android **逐字节不变**;Some = iOS `{home}/.mtdss`)。`Daemon::start` 一次解析进 `DaemonState::inbox_staging()`,P4a 全部暂存面(`InboxDestIo`/`publish_backlog`/`settle_published`/`republish_partition`/`cleanup_inbox`)改收**暂存区根**直参——语义零改动,只是"哪里"可注入。发布目标不需要引擎参数:发布 = `fs.publish` 事件 + App 侧 move(与 Android MediaStore 同构,iOS 形态 = move 进 Documents)。
- **容器 Documents 派生规则(两侧同一条)**:home 以 `Library/<name>` 结尾 → 取**父目录的父** + `/Documents`(容器布局:Documents 与 Library 同级);平铺 home(DEBUG env)取父目录一级;病态入参退化 home 内 Documents(fail-closed 收口)。**Kit 测试抓出桥初版 bug**(父一级会得 `Library/Documents`)——已修,两侧各有钉面测试(bridge_test `container_documents_derivation` 3 形 + Kit `EngineLayoutTests` 闭环)。

## 1. 目标布局(全数落地,spec §3/§5.2)

| 项 | 路径 | 生产真机最坏字节数 |
|---|---|---|
| home(App 私有) | `{container}/Library/mtdss` | 76+14 = **90 B** |
| 暂存区 | `{home}/.mtdss` | — |
| 收件箱/发布根/recv_dir/授权根 | `{container}/Documents` | — |
| UDS | `{home}/daemon.sock` | 76+14+12 = **102 B < 104** ✓ |
| fdpass | `{home}/fds.sock` | 99 B ✓ |

sun_path 账本:40(`/var/mobile/Containers/Data/Application/`)+ 36(UUID)+ 14 + 12 = 102,余量 2 字节;Kit `testWorstCaseDeviceSocketPathsFitSunPath` 钉死(并验模拟器容器 ≈170B 必须短 home env,`testOversizedHomeIsRejectedByBudget`)。

**布局漂移守卫**(e2e):默认 `{home}/inbox-staging` 不得被创建(参数化必须生效);`.mtdss` 不得落进发布根;传输中发布根零中间态;杀停后残件只在暂存区。

## 2. 交付物

| 项 | 文件 |
|---|---|
| 引擎暂存根参数化 | `crates/daemon/src/{lib.rs,state.rs,inbox_dest.rs,flow.rs,jobs.rs,main.rs,ctrl.rs}`(DaemonOpts.inbox_staging → state → 5 消费点;13 处测试字面量 + cli 测试补 None) |
| 桥布局派生 | `crates/ios-bridge/src/lib.rs`(`container_documents()` + recv_dir=settings→Documents + authorized_roots=[Documents] + inbox_staging={home}/.mtdss;错误信息携 home+双 id 便于排障) |
| Kit 布局/通道/发布器 | `ios/package/Sources/MTDSSKit/Engine/EngineLayout.swift`、`Engine/FdPassChannel.swift`(线格式 codec + POSIX 客户端,每应答一连接)、`Inbox/InboxPublisher.swift`(fs.publish → move 暂存件进 Documents 本名/`(n)` 顺延 → 0x05;fs.query_dest → 收件箱真实行 → 0x04) |
| 事件契约 | `Ipc/Contract.swift`(fs.query_dest/fs.publish 两事件 + JSONValue.asUInt64) |
| App 装配 | `ios/app/MTDSS/Browse/InboxPublish.swift`(生产三件套接线,Ruling E:逻辑归 Kit)、`EngineHost.swift`(home = Library/mtdss 派生 + DEBUG env 整体覆盖)、`MTDSSApp.swift`(boot 时挂发布器订阅流) |
| 引擎 e2e(WSL) | `crates/daemon/tests/inbox_layout_e2e.rs`(4 例,见 §3) |
| Kit 单测 | `EngineLayoutTests`(7)+ `FdPassChannelTests`(6,含真实 AF_UNIX 回环)+ `InboxPublisherTests`(10,move/`(n)`/0x04 事实面/帧组装/事件解析) |
| 桥测试 | `bridge_test.rs`:④授权根面改容器形态 + ⑤recv_dir 派生断言 + `container_documents_derivation` 单测 |
| 截图 | `artifacts/mtdss-t8-app-inbox.png`(App 常用目录 = 收件箱单枚)、`artifacts/mtdss-t8-files-app.png`(文件 App MTDSS 区) |

## 3. TDD 轨迹与测试结果

1. **红**(f444313):`inbox_layout_e2e.rs` 四支(编译红:`inbox_staging` 字段不存在)→ 引擎参数化(e39705a)→ WSL 绿。
2. **红**(Kit):三类测试先行 → 实现落地(cce58af)→ Mac 绿。
3. **终态**:
   - **Windows**:`cargo check --workspace --tests` ✓;`cargo test -p mtdss-daemon --lib` = **94 passed, 0 failed**。
   - **WSL**:`cargo test -p mtdss-daemon` **全目标绿**(lib 89 + ctrl_e2e 17 + jobs 25 + inbox_staging_e2e 8 + 新 inbox_layout_e2e 4 + 其余全绿,0 failed);`cargo test -p ios-bridge` = **3+4 全绿**。
   - **Mac**:`swift test` = **72/72,0 失败**(49 既有 + 23 新)。
4. **新 e2e 四支**(都跑 iOS 注入布局 recv_dir/授权根 = Documents、暂存 = `{home}/.mtdss`):
   - `layout_full_chain_publishes_into_documents`:Hold 窗传输中暂存有终件 + 发布根零中间态;放行 App move 落位(逐字节一致);暂存空 + 记录 done+published;默认布局目录不出现;重发 0x04 在场 Skip(锚 = 发布根实行)。
   - `kill_mid_receive_residue_confined_then_resume_publishes`:64MiB 传中杀 B → 发布根零残留 + `.part` 在场 → 收件记录直写 + 重启 → reinitiate **位图续传** → 发布 → Documents 本名逐字节一致,暂存无残留。
   - `clear_reclaims_staging_and_documents_root_is_browsable`:failed「保存失败」暂存保留 → 收件记录 `transfer.clear` → 暂存记录+终件回收、发布根不触碰;authorized_roots=[Documents] → 远端 `list` 发布根放行、home 根拒。
   - `anonymous_resend_after_kill_resumes_and_publishes`:杀停后**无任务记录重发同文件(匿名来件)**→ 短推续传(推送 <90% 全量,断言钉住;实测远小)→ 恰一次发布 → done+published → 发布根逐字节一致。

## 4. 模拟器 E2E(产线信令,Windows dev daemon A + 模拟器;A = T6-Windows-A 44d97f96,home C:/mtdss-t6a,pid 63936,收尾已杀;用户产线 mtdssd 58416 未触碰)

sim:iPhone 17 Pro Max(319AAA16),MTDSS.app(Debug,引擎 libios_bridge.a 同源构建),`SIMCTL_CHILD_MTDSS_HOME=~/mtdss-t8/home`;smoke-login 真登录产线 → **device c8b70c09**;`status.recv_dir = /Users/kk/mtdss-t8/Documents`(桥派生运行时实证)。

| # | 流向 | 证据 |
|---|---|---|
| ① | A→sim inbox(t8-e2e.bin 2MiB) | transfer.wait **done**;`~/mtdss-t8/Documents/t8-e2e.bin` md5 **82351a87… = A 源**;暂存仅剩 done+published 记录、零终件 |
| ② | A→sim inbox(t8-kill.bin 48MiB) | done;md5 **35828586… = A 源** |
| ③ | **杀 App 中间态收纳**(t8-big2.bin 600MiB,`simctl terminate` 于 252/576 块) | 残件**只在** `~/mtdss-t8/home/.mtdss/`(`.part` 603,979,776B + 记录 done=false bitmap 252/576);`~/mtdss-t8/Documents` 无该文件;**容器 Documents(文件 App 可见面)整程空**(ls -A 空) |
| ④ | 杀停后重启 App + 重发(t8-big2) | 终态 done;`Documents/t8-big2.bin` md5 **c1c030dd… = A 源**;`.part` 已消费、记录 done+published |
| ⑤ | 补充全链(t8-big.bin/t8-big3.bin 600MiB ×2) | 均 done,md5 **da8f0581…/faa4bda3… = A 源** |

ls 全量输出与记录 JSON 逐字段(rel/final_rel/done/published/bitmap)见 §4 附录原样引用于本报告写作时的会话记录;截图两帧入 `artifacts/`(模拟器档 Files App 的 MTDSS 区为空 = 覆盖 home 形态下容器 Documents 恒空的诚实呈现;文件可见性以 `~/mtdss-t8/Documents` ls + md5 为准,真机形态两者同一路径)。

## 5. Concerns(诚实记录)

1. **sim 杀停重发的续传读数与 WSL 不一致(未决)**:sim 上杀停后重发 job 7 的 A 侧 `bytes_done` = 全量(600MB/600MB),而 WSL 同场景(engine e2e 第 4 支)短推续传(推送远小于全量)。两侧 Rust 代码同源(sim 引擎 = 4a3fb83,与 HEAD 仅差测试文件);差异在杀法(sim = SIGKILL,WSL = 优雅 shutdown)或 a 侧续传轮的计数口径。**数据安全面无差**:sim 侧重发后 md5 全对、发布面正确;`bytes_done` 只是呈现口径。已留 `anonymous_resend_after_kill_resumes_and_publishes` 钉住"必须短推"的引擎语义;sim 侧口径建议 T13(真机)复测时用真机日志核实一次。
2. **无窗模拟器 App 会自发退场**(后台约 1 分钟后进程消失;无崩溃报告,jetsam 类形态)——本任务三次"杀停"其中两次实际是它代劳。T9 传输列表长时间挂机测试时留意。
3. **容器 Documents 曾有 T7 旧布局遗留 `daemon.lock`**(旧版 home=Documents 形态产物),已手动清除;新布局产线只写发布终件进 Documents。
4. 产线账号设备表新增离线残项 "iOS 模拟器"(c8b70c09,每次全新 home 即新设备;与 T6/T7 同形态,无害)。
5. 暂存记录物理路径 = `{home}/.mtdss/.mtdss/<sha256(rel)>.json`(iOS 暂存根 `.mtdss` × intake 记账目录 `.mtdss` 嵌套)——纯外观;spec §5.2 的"App 私有 `.mtdss` 隐藏目录"由第一级满足。
6. `set_recv_dir` 仍禁(T2 红线未触碰);iOS 的 settings.json recv_dir 覆盖优先级保留(bin 同款)但无写入方。

## 6. 提交清单(BASE 3103ab9 → HEAD c13232e,30 commits;代码净 diff 以 0b33051 计)

主体:`f444313`(红)/ `e39705a`(引擎参数化+桥布局)/ `cce58af`(Kit 发布面+App 装配)/ `0b33051`(匿名重发续传钉面);修复与钉面见 `git log`(含 Kit 测试抓出的桥 Documents 派生 bug 修复 f9c5b74)。净 diff:**34 files,+2271/−94**。
