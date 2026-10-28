# P4a 冒烟测试手册(收件私有暂存+完成发布)

配套 `docs/superpowers/specs/2026-10-27-mtdss-p4a-inbox-staging-design.md` §5 的逐项
展开;覆盖 T1(暂存路由+发布协议+协调器)/T2(重发分区+republish)/T3(Kotlin
发布面+渲染)全部实机验收面。手册风格沿 `docs/smoke-p3.md`(前置/操作/预期
三行);P3 手册的基础面(登录/浏览/发送/更新,§A-§F 共 30 项)不重复。

> ## ⚠️ 全部实机项 PARKED(待设备)
>
> 下文 §A-§G 均标注 **PARKED(待设备)** = 代码/构建已交付(T1-T3),
> 判定待设备场执行;设备场由用户在场协作(装包/操作/MIUI 权限弹窗),
> 逐项打勾 + 记录现象。重点三案(spec §5):force-stop 续传跳块、删终件
> 重发、发布中杀 App→重发 republish。

## 0. 本刀验收面(能力边界增量)

| 能力 | 状态 |
|---|---|
| 收件改私有暂存(引擎 FsDestIo 复用,`{filesDir}/mtdss/inbox-staging`)→ 完成发布进 MediaStore | ✅ T1/T3 代码交付;实机判定 PARKED |
| 跨进程死亡真续传(强杀 App/重启 → 重新发起 → 位图跳块,不再降级整收) | ✅ T2 e2e 首钉;实机判定 PARKED(§B headline) |
| 任务「保存中」(publishing)态与「保存失败」词面渲染 | ✅ T3 JVM 钉;实机判定 PARKED(§A2/§D3) |
| 三案根除:F3 pending 回收 / F4 孤儿 / “(1)” 改名(公开区零中间态) | ✅ 设计根除;实机判定 PARKED(§E) |
| 在场双层判定(记录 done+published → 0x04 问 MediaStore:在场 Skip / 缺件网络重收) | ✅ e2e 钉;实机判定 PARKED(§C) |
| 发布失败:暂存保留 → 重新发起 = republish-only 零网络(免登录/免对端) | ✅ e2e 钉;实机判定 PARKED(§D) |
| 旧 v2-P3 `inbox-records` 残留启动清扫(无迁移) | ✅ T1 交付;实机判定 PARKED(§G) |
| 进度条(F5) | ❌ 非本刀(spec §2 非目标,P4「双端进度统一」专项) |
| 桌面端 | ❌ 零改动(golden 逐字节零漂移,T4 全量比对核验) |

## 1. 环境与准备

### 1.1 环境矩阵

| 环境 | 覆盖测试项 | 准备要点 |
|---|---|---|
| MIX 3(Android 10 / API 29 / arm64) | §A-§G | P4a debug APK;文件管理器(系统自带);存储权限在位(§D3 要反向操作它) |
| Windows PC(同账号对端) | 全部(发送端) | daemon 以 `--log-file` 起便于回查;备好小文件(≈10MB)与大文件(≥1GB)各一 |

### 1.2 装机与观测口径

- 装机:`MSYS_NO_PATHCONV=1 adb install -r android/app/build/outputs/apk/debug/app-debug.apk`。
- 日志:`MSYS_NO_PATHCONV=1 adb logcat -s MtdssEngine mtdssd`。
- 私有暂存区(run-as;用户与文件管理器全程不可见):
  - 暂存件与记录:`MSYS_NO_PATHCONV=1 adb shell run-as com.mtdss.android ls -la files/mtdss/inbox-staging files/mtdss/inbox-staging/.mtdss`
  - 记录内容(位图/`done`/`published` 标记):`… run-as com.mtdss.android cat files/mtdss/inbox-staging/.mtdss/<sha256(rel)>.json`(文件名先 `ls` 取)。
- md5 对账:PC `certutil -hashfile <文件> MD5`;设备 `MSYS_NO_PATHCONV=1 adb shell md5sum /sdcard/Download/MTDSS/<文件>`。
- 公开区快照:`MSYS_NO_PATHCONV=1 adb shell ls -laR /sdcard/Download/MTDSS`(§E 传前传后各一)。

## 2. 测试项清单

### A. 全链收件:暂存→保存中→done(T1 e2e①对应;PARKED 待设备)

- [ ] **A1 小文件全链**(PARKED 待设备)
  - 前置:双端登录在线;公开区快照在位(§E1 复用)。
  - 操作:PC 发 ≈10MB 小文件到 MIX 3 → 任务行走完全程;done 后文件管理器
    看 `Downloads/MTDSS/`。
  - 预期:任务翻 done;文件在场且 md5 与源一致;「保存中」徽章一闪而过
    (小文件发布窗毫秒级,不见属正常);run-as 查暂存区:`.part`/暂存终件
    已删,`.mtdss` 记录留(带 `published:true`)。
- [ ] **A2 大文件「保存中」可见窗 + md5 对账**(PARKED 待设备)
  - 前置:A1 通过;≥1GB 大文件。
  - 操作:PC 发大文件;网络进度走满后盯任务行(尾段窗口)。
  - 预期:网络收妥后任务呈**「保存中」**(publishing,非终态徽章)——此即
    「网络收妥≠完成」的诚实时序;发布完成(拷贝进 MediaStore)翻 done;
    `Downloads/MTDSS/` 文件 md5 与源逐字节一致;期间文件管理器全程**不
    可见**半成品(私有暂存,公开区零中间态)。

### B. 跨进程续传 headline(T2 e2e①③对应;PARKED 待设备)

- [ ] **B1 强杀→位图跳块续传**(PARKED 待设备,headline)
  - 前置:A2 通过;≥1GB 大文件传输进行中;另备一段录屏/记录。
  - 操作:①传输中 run-as 查暂存区:`.part` 在场、`.mtdss` 记录位图部分
    置位(中间态确在私有区);②`MSYS_NO_PATHCONV=1 adb shell am force-stop
    com.mtdss.android`(强杀 App+引擎,跨进程死亡);③重开 App → 任务行
    「中断」→ 点「重新发起」;④盯任务行进度。
  - 预期:重发起跑**进度从断点字节起跳**(接收进度初值 = 已收部分,引擎
    Resume 语义),数秒内走完剩余(对照整收需分钟级)= 位图跳块;PC 侧
    新发送任务 bytes_done 终值 **<** 文件大小(只补缺块);随后「保存中」
    → done;md5 与源一致;run-as 查暂存:`.part`/暂存终件删、记录
    `published:true`。
- [ ] **B2 短熄屏扛过**(PARKED 待设备)
  - 前置:B1 通过;传输进行中。
  - 操作:按电源键熄屏数秒 → 亮屏回 App。
  - 预期:任务不断(spec §5「短熄屏扛过」),暂存照常推进、发布照常;
    若系统确杀(记录现象,非预期),走 B1 续传路径自愈不判失败。
- [ ] **B3 混合分区:部分续传+部分 republish**(PARKED 待设备)
  - 前置:多文件任务(≥3 件,含大文件)传输中途强杀 App(部分文件已整收
    进暂存、部分半程)。
  - 操作:重开 App → 「重新发起」。
  - 预期:一个新任务行收口——已整收件 republish(零网络)、半程件位图续
    传;全部「保存中」→ done;各文件 md5 逐一相符(T2 e2e③ 的实机形)。

### C. 在场双层:Skip 与删件重发(T1 e2e④对应;PARKED 待设备)

- [ ] **C1 在场 Skip**(PARKED 待设备)
  - 前置:A1 的文件已 done,**不删**。
  - 操作:PC 对同一文件再次发送。
  - 预期:秒级 done;文件管理器无 “(1)” 副本、原文件字节不动(md5 同旧);
    设备侧该接收任务零网络字节(bytes_done=0,0x04 在场查询命中 → 引擎
    Skip);PC 侧发送卡照常计完成(files_done=1,Skip 计完成口径)。
- [ ] **C2 删终件重发(query 0x04→网络→发布)**(PARKED 待设备)
  - 前置:C1 通过。
  - 操作:设备**文件管理器**删除 `Downloads/MTDSS/<文件>`(经管理器删,
    MediaStore 行同步回收;勿用 `adb shell rm`,残行会搅 0x04 判定)→
    PC 再发同文件。
  - 预期:0x04 答「不在场」→ 走网络重收(任务行真实进度)→「保存中」→
    done;文件回到 `Downloads/MTDSS/`,md5 与源一致,无 “(1)”。

### D. 发布失败与 republish(T1 e2e②③ / T2 e2e②对应;PARKED 待设备)

> 真机无 0x05 丢帧/30s 超时的注入面;发布失败注入的可行变体 = 传中杀
> App(暂存保留 → 重发 republish-only)。超时分支由 e2e 钉死
> (`MTDSS_TEST_PUBLISH_TIMEOUT_MS`),实机不追。

- [ ] **D1 保存中强杀 → 暂存保留 → republish-only 零网络**(PARKED 待设备)
  - 前置:大文件传输至「保存中」窗(A2 尾段)时 `am force-stop`。
  - 操作:重开 App → 任务行呈「中断」(publishing 非终态,重启映射)→
    先 run-as 查暂存:暂存终件**在场**、记录 `done:true`(未 published)
    → 点「重新发起」。
  - 预期:republish-only——数秒内「保存中」→ done,**零网络字节**(PC 侧
    daemon 日志无新传输;对照 B1 的续传有字节);run-as 查暂存:暂存终件
    删、记录 `published:true`;文件管理器文件在场。
- [ ] **D2 republish 免登录/免对端**(PARKED 待设备)
  - 前置:另造一条「保存中」强杀的中断行(重复 D1 前半,先**不**点重新
    发起);PC 关机或断网(对端不可达更铁)。
  - 操作:设备开飞行模式 → 中断行「重新发起」。
  - 预期:纯本地发布照常完成(免登录/免对端,spec §6.3)——「保存中」→
    done;关飞行模式后状态自然回同步。
- [ ] **D3 「保存失败」词面渲染**(PARKED 待设备;窗口窄,不苛求必现)
  - 前置:多文件大任务(加宽发布窗);设置里能看到 MTDSS 存储权限。
  - 操作:「保存中」窗内撤销存储权限(设置→应用→MTDSS→权限)。
  - 预期:发布失败 → 任务 failed,状态列呈**「保存失败」**、行内带原因
    词面(daemon「保存失败(N 项): …」前缀,spec §3.3);暂存保留(run-as
    可查)→ 恢复权限 → 「重新发起」= republish-only 补齐(§D1 同路)。
    注入未命中(窗口错过)不判设备失败:渲染契约已由 JVM 钉(T3
    TasksReducer),记录现象即可。

### E. 无 “(1)” 无孤儿(公开区洁净;PARKED 待设备)

- [ ] **E1 全链路径快照对账**(PARKED 待设备)
  - 操作:传前 `ls -laR /sdcard/Download/MTDSS` 快照 → 完成 A1/A2 →
    传后再快照;传输中途刷一次文件管理器。
  - 预期:传后快照 = 传前 + 恰好新增终件(名字/字节数与源一致),**无
    “(1)” 后缀件、无零字节残件**;传输中文件管理器任何时刻不可见中间态
    (F3/F4 根除面)。
- [ ] **E2 强杀/中断路径快照对账**(PARKED 待设备)
  - 操作:B1/D1 强杀后、重发完成前,公开区再快照。
  - 预期:与传前快照逐项一致(公开区零中间态 = 孤儿根除;中间态全在
    run-as 私有区)。

### F. 同落位真文件撞名缺省驱逐(T3 评审移交单列;PARKED 待设备)

- [ ] **F1 撞名缺省驱逐**(PARKED 待设备)
  - 前置:A1 的文件已 done;PC 侧**修改该文件内容**(字节不同,记新
    md5)。
  - 操作:PC 再发同名同落位。
  - 预期:done 后 `Downloads/MTDSS/<文件>` 内容 = **新版**(md5 = 新版;
    App 端 policyFor 缺省 overwrite=落最新);无 “(1)” 副本;旧版被驱逐
    (MediaFace F11 机器)。

### G. 旧残留清扫(T1 对应;PARKED 待设备,可选)

- [ ] **G1 旧 inbox-records 启动清扫**(PARKED 待设备)
  - 前置:测试机曾装 v2-P3 构建且 `files/mtdss/inbox-records` 有残留
    (无残留则记「无可验,防串台 e2e 已钉」)。
  - 操作:升装 P4a APK,首启。
  - 预期:run-as `ls files/mtdss` 无 `inbox-records`;logcat 见「已清扫
    v2-P3 旧收件桥记录布局」;登录/任务列表正常(无迁移,spec §2 非目标)。

### 记录格式

每项:`编号 | 通过/失败 | 现象一句话 | (失败时)logcat/截图路径`。日志:
`adb logcat -s MtdssEngine mtdssd`、PC 侧 daemon `--log-file`、任务列表与
文件管理器截图、run-as 暂存区输出。全部通过后回填本清单勾选 + spec 状态行
(controller 收官时)。

## 3. 故障排查速查(P4a 增量)

| 现象 | 先查 |
|---|---|
| 卡「保存中」超 30s 不翻 done | logcat mtdssd:`fs.publish` 是否发出/0x05 是否应答;存储权限被撤(§D3);存储空间满 |
| 任务「保存失败」 | 行内原因词面;run-as 查暂存应保留(重发 = republish-only);权限/空间二连查 |
| 重发整收不跳块(进度从 0 起步) | run-as 查记录位图与 `.part` 是否在场(强杀是否真杀干净:`am force-stop` 后 `pidof mtdssd`);记录是否 done(整收=对账 Full) |
| done 后文件管理器看不到文件 | MIUI MediaStore 缓存刷新(重进文件管理器/重启);`ls /sdcard/Download/MTDSS` 在而管理器不在 = 刷新问题非发布失败 |
| C2 删件重发不重收(秒 done 但文件不在) | 是否经文件管理器删(残行搅 0x04);`adb shell content query --uri content://media/external/file --where "relative_path LIKE 'Download/MTDSS%'"` 查残行(MediaProvider 归一化恒带尾杠,等值查询必 0 行假「无残行」;LIKE 前缀连子目录一并查得) |
| 重发报「保存失败(N 项)」但其余文件好了 | 镜像 P2 部分缺失语义(spec §6.1):失败件暂存保留,再「重新发起」= republish-only 补齐 |
| 装新版后旧任务消失/报中断 | 旧 pending 记录不迁移(spec §2 非目标):非终态加载即 interrupted,重新发起走新链路 |
