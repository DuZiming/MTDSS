# MTDSS P4a 设计:收件私有暂存 + 完成发布

- 日期:2026-10-27
- 状态:实现完成(2026-10-28,四任务+评审链;终态随 PR 回填)
- 上游规范:[2026-10-25-mtdss-v2p3-android-design.md](2026-10-25-mtdss-v2p3-android-design.md)(P3 已收官 main@13a58cc;本设计是其 §5.2 接收桥的重新设计,取代「MediaStore pending 直写」形态)
- 票据:F3(MIUI 回收 pending 行→跨进程死亡续传降级整收)、F4(孤儿半成品)、“(1)” 改名——三案根除;F5(进度条)明确不在本刀,留 P4 双端进度统一专项

## 1. 意图与为什么

用户原则(P3 复述确立,源自 M2c 桌面地基):**传输中间态放隐藏目录、完成后清理、中断后重发可凭中间态断点续传**。P3 用 MediaStore IS_PENDING 充当「隐藏中间态」,真机(MIX 3/MIUI 12)证明它在三个面全破:force-stop 后 pending 行被平台回收(F3,属主查询可见性已排除干扰)→半成品失联整文件重收;裸预分配文件泄明文→孤儿(F4)与“(1)”改名。

安卓 scoped storage 不允许引擎直写公开 Download 内的隐藏子目录,「目标的隐藏目录」的正身 = **App 私有暂存区**(用户与平台皆不可见不可收)。完成时由 App **发布**进 MediaStore——发布即「从隐藏目录挪到正式位置」的安卓等价物。

## 2. 目标 / 非目标

| 目标 | 内容 |
|---|---|
| 真断点续传 | 跨进程死亡(MIUI 杀进程/重启手机)重发后从暂存位图续传,不再降级整收 |
| 中间态隐形 | `.part`+位图全在 `{filesDir}/mtdss/` 私有区;用户与文件管理器全程不可见 |
| 完成即清理 | 发布成功即删暂存件与记录;失败保留待重试 |
| 三案根除 | F3 pending 回收、F4 孤儿、“(1)” 改名一并消失(不存在公开区中间态) |
| 发布时序诚实 | 任务显示「完成」的充分条件=文件在收件箱(MediaStore)真实可见 |

| 非目标(明确不做) | 说明 |
|---|---|
| 进度条 | F5 留 P4「双端进度统一」专项(用户裁定 2026-10-27) |
| 桌面端改动 | FsDestIo 与桌面行为零变化;golden 桌面侧零漂移 |
| 发送侧 | request_source/512 护栏/两段式解析一概不动 |
| 存量迁移 | 直发未上架,无存量用户;旧 pending 残留(仅测试机)不做迁移,装新版自然清 |

## 3. 核心裁定(brainstorm 定稿)

1. **引擎侧**:安卓收件路由到 **FsDestIo(recv_dir={home}/inbox-staging)**——即桌面同款路径语义直接复用(私有区真实路径,引擎零障碍);`fs.request_dest`/fdpass 0x02 协议**整体退役**;`fs.query_dest`(0x04,终件在场查询)保留(发布后的 Skip 判定仍需问 MediaStore 真实行)。
2. **发布协议**:daemon finalize 成功后经 IPC 事件 `fs.publish {"request_id","rel","staging_path","size"}` 请求 App 发布;App 执行 MediaStore insert(带尾杠 RELATIVE_PATH)+流拷贝+揭开,经 fdpass 新 kind `0x05 publish_result {"request_id","ok","error?"}` 应答;daemon 收 ok 后删暂存并落任务终态。
3. **任务时序**:接收任务新增「保存中」态(finalize 完成、发布未确认);发布确认后才 `done`。发布失败→任务 failed「保存失败:…」但**暂存保留**,重新发起=仅重发布(不再走网络)。
4. **fdpass 通道**:0x01(源)、0x04(查询)两通道存续;0x02 通道退役(三实例纪律改二,文档同步)。
5. **BridgeDestIo 退役**:T6 的 fd 直写实现整体删除(含 inbox-records 布局——暂存区即 FsDestIo 的 `.mtdss` 记账,键空间天然隔离于桌面)。

## 4. 发布失败与边界

- 发布超时(30s,常量注入):任务转「保存失败」,暂存保留;重发路径识别「暂存件已完整(记录 done)」→直接重发布,零网络。
- 发布中 App 被杀:暂存与记录完好;重开发 App 后任务列表对 failed「保存失败」行重发→重发布。
- 磁盘满(拷贝中途):同「保存失败」,MediaStore 半行由 App 清理(insert 的 pending 行在拷贝失败时删除——发布自身的原子性沿用 IS_PENDING 包裹,发布内部的 pending 不再承载传输中间态,只包裹拷贝瞬间)。
- 暂存区损坏:沿 FsDestIo 既有语义(记录损坏→整文件重收)。

## 5. 测试与验收

- Rust e2e(inbox_staging_e2e,替代 inbox_bridge_e2e):①全链收→发布→清理→done;②传输中杀 daemon→重发→位图续传(跨进程续传首次有 e2e);③发布超时→保存失败→重发仅重发布;④终件在场的 Skip(query 0x04 命中);⑤0x02 通道退役后 0x01/0x04 无串扰。
- Kotlin JVM:发布决策(成功/失败/超时)、0x05 帧、任务「保存中/保存失败」态渲染。
- 真机(MIX 3):重演昨天全部案——force-stop 中断→重发真续传(位图跳块日志)、删终件重发、无“(1)”、无孤儿、短熄屏扛过。

## 6. 评审裁补(builder check 五问,2026-10-27 定)

1. **发布协调器归属**:round 完成语义=「**网络面收妥**」(全部落暂存、finalize 过哈希)——round 仍在此点正常结束;**发布协调器作为独立层**在 round 之后驱动任务走 保存中→(发布确认)→done / failed「保存失败」。download 驱动的终态合成增「发布已落定」条件;源端 10s 宽限只作用于网络相(终态化前),不侵入发布窗。**多文件发布并发**(每文件独立 30s);单文件发布失败=任务 failed「保存失败(N 项)」,其余文件照常发布保留(镜像 P2 部分缺失语义),重发走 republish 分支补齐。
2. **Skip 锚**:发布成功删**暂存 .part 件**但**保留记录并打 published 标记**(记录=Skip 锚,0x04=现实在场确认——双层,沿 F10 精神)。收件 DestIo=FsDestIo 薄包装:写/对账复用,唯 `final_state` 覆写为「记录门(done+published)→0x04 查询」——**绝不 stat 暂存路径**(staged-but-unpublished 不得 Skip,恰是本设计要杀的假完成类)。桌面对账零改动(覆写只挂收件实现)。
3. **Republish 判定树**:资格=**暂存记录 done,不限任务显示态**(failed「保存失败」/interrupted/cancelled 皆可——数据在本地,拒发布即无理;P2 的「done/cancelled 无动作」约束的是网络重发,本地发布是另一操作,经「重新发起」入口分流)。混合 rel 集:重发时**分区**——暂存完整者 republish、缺失者走网络,一个新 job(P2 翻新语义);分区保证 staged 件永不进接收集(引擎 Full 截断无从触及)。republish 分支免登录/免对端(纯本地)。
4. **发布撞名**:复用 MediaFace 行生命周期(F11 机器):预查落位行——真开失败(僵尸)→驱逐;真文件在场→**沿任务 TransferPolicy**(skip→弃暂存记 skipped;overwrite→驱逐+插入)。发布名与 0x04 键=记录的 final_rel(暂存内部 unique_target 改名天然携带)。
5. **0x05 通道与 id 空间**:0x04/0x05 共用**查询通道**+**统一 id 计数器**(双侧同一 resolver/同一 pendingAcks,单计数单通道无错路——Important A 教训的正面应用);实例纪律改二成立。

## 7. P4a-r2 修订:发布走 SAF 授权树(2026-10-28 晚,用户裁定 C)

F14b 实盘后用户裁定:点文件临时名方案**仍违反本设计 §1 原则**(中间态须在隐藏目录、任务成功后清理;残留件无 MediaStore 行=App 永不可清)。根因:MediaStore 路径写公开目录必然存在「公开区半成品瞬间」,平台无原子搬家。修订:**收件操作整体迁至 SAF**。

1. **授权**:首次收件发布前,`ACTION_OPEN_DOCUMENT_TREE`(EXTRA_INITIAL_URI=Download)一次性授权;用户可授 `Download/MTDSS`(最小权限)或 `Download`(App 在树内自管 MTDSS 子目录);持久化授权沿发送侧 RootsStore 模式。设置页显示授权状态与重授入口。
2. **收件箱根**:归一化为授权树内 `MTDSS` 目录(SAF createDirectory 自建;已存在则直接用)。rel 语义不变(§5.1 目录保真)。
3. **发布流(SAF 全权)**:私有暂存 →(经 DocumentsContract)在 `MTDSS/.mtdss/` **隐藏目录**建临时文件拷贝 → `renameDocument` 落正式名(返回名≠请求名=碰撞,沿 F14b 词面)→ 成功后删临时;活进程内任何失败 SAF 删临时。强杀残留**始终在 `.mtdss` 内**(隐藏目录,原则兑现),且下次发布可清理(SAF 对授权树有删除权——GC 能力回归 App)。
4. **在场查询(0x04)改走 SAF**:按 rel 在授权树内查子文档真实存在+尺寸——比 MediaStore 查询更真(无扫描器时延),F10-F12 的僵尸行家族(行与文件失联的一切形态)在接收面**整体退场**。
5. **撞名**:SAF 查同名子文档——真文件沿任务 TransferPolicy(skip 弃件/overwrite SAF 删后落位)。
6. **MediaStore 面**:发布后的文件由系统扫描器自然索引(Downloads UI 可见);App 不再持有/管理任何 MediaStore 行;rename 成功后主动 `MediaScannerConnection.scanFile` 触发索引(docId 推导路径,ExternalStorageProvider 形)。**InboxStore 的 MediaStore 实现(MediaStoreInboxStore/MediaFace)整体退役**。
7. **MIUI 已验语义沿用**:点开头名经 SAF 的保留性(ExternalStorageProvider 直写文件系统)为**本修订首要实机验证项**;若 MIUI 在 SAF 面同样改写点名,`.mtdss` 以 `_.mtdss` 形落但仍为独立目录——残留收纳于其内,原则「隐藏目录」在 MIUI 语义下降级为「专用目录」,GC(SAF 全权)仍然成立——此降级可接受并记录。
8. **Rust 面零改动**:fs.publish/0x05/查询 0x04 协议与语义原样;发布超时/相位/协调器/重发分区不动;App 应答面的实现换了地基,daemon 无感。

## 8. 后续移交

- F5 进度:暂存位图 App 可直读——P4 双端进度统一时一并立法。
- smoke-p3.md 30 项实盘勾验仍挂 P4 档期。
