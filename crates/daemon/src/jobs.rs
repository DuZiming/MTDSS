//! 传输作业注册表 + upload/download/local_copy/remote_copy 驱动 + 速度
//! EMA + 事件节流(M3c Task 6/7/8)。
//!
//! 生命周期不变量(取消/暂停的语义钉死,改动前必读):
//!
//! - **取消 = 置 watch + abort 驱动任务**。[`Jobs::cancel`] 在同一临界区里
//!   查终态 → 发取消 watch(流内 select 感知)→ 终态落定 `cancelled` →
//!   取走 JoinHandle 并 abort——连 run_send_with 的重试循环一并杀,**绝不
//!   复活**(JoinHandle 取走后注册表不再持有任务面)。abort 与流内 select
//!   是双保险:abort 在任务挂起点立即生效,select 让取消在退避睡眠/暂停
//!   门等待中也能以干净错误收束。终态粘滞:done/failed/cancelled 一旦
//!   落定,后续 cancel/finish/翻转全部无操作(cancel 与完成竞速时先到者
//!   胜,后到者只落空,不回滚)。
//!
//! - **暂停 = 双侧共享 PauseGate**(任务注册表持一份,流进
//!   SendOptions.pause)。门在发送引擎的每波前/每条块流开启前挂起(在途
//!   写不受影响,接收侧靠 QUIC 流控背压);重试循环轮间也持同一门——
//!   **暂停期间不烧重连预算、不退避、不重打洞**;若网络失败发生时门处于
//!   暂停态(连接死于用户挂起期间,如接收端 idle 超时),该失败按暂停
//!   归因,attempt 不增(预算 re-arm),门上等恢复后按原预算重试。
//!   pause + 断链 + resume 由此收敛:恢复后既有重试循环重新打洞,接收方
//!   按 .mtdss 记录对账断点续传。可见状态随之翻转(running↔paused,事件
//!   即发),但驱动起步时若用户已暂停(queued 期 pause),状态保持
//!   paused——门会在流内生效,不会出现「状态 running 实际挂死」的假象。
//!
//! - **事件契约**:`event.transfer {id,kind,state,files_done,files_total,
//!   bytes_done,bytes_total,speed_bps,error?}`——状态翻转即发;进度节流
//!   ≥400ms(首条进度即发,保证快传也有最早反馈;终态帧恒即发,不受节流
//!   约束)。transfer.wait 长轮询由 Notify 唤醒(enable-先于-查 的登记序
//!   防漏唤醒)。
//!
//! - **持久层(v2-P2 Task 4)**:三类网络作业(upload/download/remote_copy)
//!   建卡即全字段落盘 `{home}/tasks.json`(经
//!   [`crate::taskstore::TaskStore`];编排回显/匿名 incoming/local_copy 是
//!   被动或本机面不持久化——记录只在发起端,spec §2)。状态翻转即时落盘,
//!   进度 ≥5s 节流且 FileDone 恒写(窗口内崩溃至多回退一个检查点,续传由
//!   对账兜底)。**内存注册表不驱逐终态(M4a 上限退役:持久层即历史面)**,
//!   唯一例外(终审修复#2):clear 成功删记录 / reinitiate 摘旧翻新时,该
//!   id 的终态 live 条目同步逐出——记录已不在,live 行失去消费面(行上按
//!   钮全报「任务不存在」)。transfer.list 由持久层打底 + live 快照按 id
//!   覆盖合并;重启后非终态由 load 标 interrupted,注册表不自动复活作业。
//!
//! - **清除(v2-P2 Task 7,spec §5.2「各清各的」)**:`transfer.clear` 按记
//!   录类型回收残留面——download 本地清目标目录 `.mtdss`(恒可执行,不依
//!   赖对端);upload/remote_copy(含 cancelled 终态)对端 `cleanup_outbox`
//!   回执 Ok 才删本地记录(离线失败保留可重试);done 拒清(完成行无操作
//!   入口,对端 done 记录是后续 SKIP 素材,spec 明文不动);live 态
//!   (queued/running/paused)拒清(先取消或等终态,门在空 rels 短路之前);
//!   rels 为空直接删记录(无可清面)。删除成功时同步把该 id 的终态 live
//!   条目逐出注册表(终审修复#2:行随记录走,不再幽灵至重启;live 非终
//!   态条目不经此路径)。
//!
//! - **重新发起(v2-P2 Task 6/8,spec §5.1)**:failed/interrupted 记录翻新
//!   重发(store.remove + 原参数 + 新 id),一次调用一次驱动。摘旧成功即
//!   同步逐出终态 live 行(同清除口径,终审修复#2);摘旧后重发起动失败
//!   (全源消失/未登录等)原记录回插——记录不自动删除(spec 裁定 6),
//!   不留「记录蒸发、对端残留无清除入口」的死面。upload 走本
//!   地 partial 预检(T6);**download/remote_copy = 重发编排**:orchestrate
//!   携 `old_rels = rec.rels` 给源端,源端以同一 partial 机制预检,dropped
//!   由**源端**在推流前统一清接收端残留(spec §5.1,与 upload 清理同一机
//!   制);缺失项混入 missing → 混合后果 failed(在的文件照常交付)。
//!   rec.rels 空(尚未经 job_query 回填)= 退化重发:old_rels 空 → 源端
//!   普通首发预检,行为正确只是残留清理面窄。**幂等门**:store.get 返回
//!   克隆,并发 reinitiate 可能双双过校验门——`remove` 的命中返回值是唯一
//!   原子裁决,未命中即已被并发重发摘走,按存在性报错(不造第二个并行
//!   驱动)。cancelled 的记录一律拒(仅 failed/interrupted,同 T6 门)。
//!   **收件箱 download 例外(P4a T2,spec §6.3)**:target_dir=收件箱哨兵
//!   的记录先做**重发分区**(`inbox_dest::republish_partition`:暂存记录
//!   done 未发布 → staged;缺失 → missing;已发布 → 无动作面)——
//!   republish-only(缺集空)时 **cancelled 亦放行**(republish 资格=暂存
//!   记录 done,不限显示态;P2「cancelled 拒」约束的是网络重发,本地发布
//!   经「重新发起」入口分流);**同一新 job** 承载两相([`Jobs::start_
//!   download_republish`]):republish 相(纯本地重发 fs.publish,免登录/
//!   免对端,**先于网络相落定**——发布打 published 后网络相同名件经 0x04
//!   在场查询天然 Skip,staged 件永不进接收集,Full 截断/"(1)" 改名无从
//!   触及)+ 网络相(缺失集走既有 download 编排)。翻新记录携带 rels 全集
//!   (分区语义,后续重发可再分区)。
//!
//! - **cut 收尾**:cut=true 的 upload 在驱动成功后本地 `fs::delete_paths
//!   (force=false)` 删源(回收优先,尽力而为:失败仅留痕,不翻终态);
//!   删源先于 done 落定——wait 返回 done 时删源已完成,磁盘事实与状态
//!   一致(cancel 若落在本窗口,终态由先到者定)。混合后果路径(partial
//!   预检有 missing,终态落 failed)同样删源:删源挂在驱动 Ok 之后、终态
//!   翻转之前,与终态是 done 还是 failed 无关——在的文件交付完即回收。
//!
//! - **并发面**:作业不经遗留 send 的互斥(code 2 只属遗留 send RPC);
//!   多作业并发合法。V1 已知限制:同一接收端的入件仍按常驻接收轮串行。
//!
//! - **RoundHandle/bind_incoming**:resident 接收轮与作业的双向句柄
//!   (进度入注册表 + 作业侧取消/暂停直达该轮)。Task 7 接线:resident
//!   loop 在 ConnectRequest 处装配三件套并 `bind_incoming`(仅 download
//!   作业可作绑定目标——跨设备普通作业的 id 撞号不构成绑定,防误路由);
//!   cancel/pause 直达绑定的轮,进度回调随轮进 RecvCtl。终审修复补两
//!   面绑定纪律:绑定即把作业级暂停门现值**种子**进轮门(暂停可能落在
//!   轮装配前——orchestrate/打洞/重试间隙,作业级门无消费方,不抄即
//!   静默丢失);**终态作业拒绝绑定**并驱动该轮取消 watch(取消后仍在
//!   途的来件数据不得落地)。
//!
//! - **download(Task 7)**:操作端注册 download 作业 → ctrl `orchestrate`
//!   发给源设备(to_device=自己)→ 源端起 upload 驱动(回显同一 job_id,
//!   [`Jobs::accept_orchestration`],外部 id 已存在即拒)→ ConnectRequest
//!   带 job_id 回来 → resident loop 绑定轮路由进度/取消。轮成功记落定
//!   事实(round_ok),**终局由驱动按源端作业态合流收口**(Task 8:混合
//!   后果的 failed 终因只在源端,轮先落 done 会被终态粘滞吞掉映射;源端
//!   done + 轮落定 → done,源端 failed/missing → failed);轮失败作业保
//!   持 running——源端驱动自行重试,重来的 ConnectRequest 重绑续流,终
//!   局失败由 download 驱动周期 `job_query` 探测源端作业态收口。取消 =
//!   本地照常(cancel watch + abort + 轮取消)+ 独立哨任务在 cancelled
//!   终态后 best-effort `cancel_job` 通知源端停重试(哨不经驱动任务,
//!   abort 杀不掉)。
//!   cut 收尾 = 本地 done 后对源端 ctrl `delete`(回收优先,尽力而为)。
//!   计数面:数据在远端,预扫不可为——files/bytes 总数恒 0(未知),
//!   RecvProgress 逐步填 done/bytes。查询持续不可达超
//!   [`SOURCE_POLL_FAIL_BOUND`] 亦按失败收口(对端真下线不悬挂)。
//!   **rels 回填(Task 8,spec §4.1)**:源端回显 upload 驱动的预扫 rel 全
//!   集随 JobQueryResponse.rels 回带,操作端轮询处非空即写回 store
//!   ([`JobCore::backfill_rels`])——重发 diff 的「全集」原料;回填前的
//!   空集是 spec 明文的退化可接受面(行为正确,残留清理面窄)。
//!
//! - **local_copy(Task 7;M4b 移交债①策略化)**:本机复制(spawn_blocking
//!   跑 `fs::copy_paths_with`,cut=false)或移动快路(`fs::move_entries_
//!   with` 逐顶层条目 rename,cut=true;文件覆盖语义、目录撞名逐对失败),
//!   冲突策略(skip/overwrite)与传输面同口径经 RPC 下发进驱动——同设备
//!   复制不再有「35s RPC 同步等待」的旧脸面,作业卡/取消/暂停全量可用。
//!   纯本地操作不要求登录;fs 层无逐字节进度,作业卡以预扫计数呈现,完成
//!   时一次性补齐 done/bytes;策略 skip 命中数落 `JobSnap.skipped`(各
//!   kind 缺省 0),事件与快照同带。
//!
//! - **remote_copy(Task 8)**:操作端 X 编排**两台远端设备间**的直传
//!   (from 源 → to 目标;数据不经 X 落地,X home 无传输数据)。预扫(from
//!   端逐源 stat × to 端目标目录 list;目录缺失 = 无冲突,列表截断按顶层
//!   可见集比对 + 警示;命中同名仅留痕,策略以调用方下发表为准)→
//!   orchestrate 发给 **from 设备**(其起回显 job_id 的 upload 驱动,
//!   ConnectRequest 定向 to)→ 本端卡片 1.5s 轮询 from 端 job_query 至终态
//!   并即时映射——done 无宽限窗(X 无本机数据面,from 发完 = to 已收口,
//!   与 download 的收端时差语义不同);failed/cancelled/missing 同 download
//!   口径,查询持续不可达超 [`SOURCE_POLL_FAIL_BOUND`](60s)亦按失败收口
//!   ——对端真下线不悬挂(单次瞬时失败不计,任一成功查询归零,取消不受
//!   此界约束)。cut 收尾 = done 后对 from 端 ctrl delete(回收优先,尽力
//!   而为);
//!   cancel = 本地终态 + 独立哨 cancel_job 发 from(哨不受驱动 abort 影响,
//!   同 download)。暂停面缺位(本机无数据流,门无消费方):set_paused 对
//!   remote_copy 直接拒,防「状态 paused 实际在传」假象。id 命名空间沿
//!   download 口径(`<本机 device_id>:<序号>`,回显键与 from 端自增面/其他
//!   操作端永不撞号)。计数面:数据在远端两端之间,预扫不可为——files/
//!   bytes 总数恒 0(未知)。已知 V1 限制:冲突策略顶层粒度,嵌套同名交
//!   接收端引擎安全网。重发与 rels 回填同 download 口径(见「重新发起」
//!   与 download 节;X 的 rec.rels 经轮询 from 端回带写回)。
//!
//! - **匿名 incoming 卡(Task 7)**:ConnectRequest 不带 job_id(legacy
//!   send / 对端无任务面)→ resident loop 注册 kind="incoming" 的匿名卡
//!   并绑定轮——每笔传输都有可见卡片。轮失败即落 failed(源端重试会起新
//!   卡),轮成功落 done。

use crate::ctrl::CtrlErr;
use crate::state::DaemonState;
use crate::taskstore::{TaskRec, TaskStore};
use mtdss_core::protocol::manifest::SourceFile;
use mtdss_core::transport::intake::TransferPolicy;
use mtdss_core::transport::transfer::{PauseGate, RecvProgress, SendProgress};
use mtdss_ipc::EventFrame;
use serde_json::json;
use std::collections::{HashMap, HashSet};
use std::path::{Path, PathBuf};
use std::sync::atomic::{AtomicU64, Ordering};
use std::sync::{Arc, Mutex, MutexGuard};
use std::time::{Duration, Instant};
use tokio::sync::{watch, Notify};
// fd 桥面(v2-P3 T4 起):fdpass 通道与 fd 类型仅 unix;Windows 编译单元
// 整体缺席(SourceResolver 走占位实现,见下方)
#[cfg(unix)]
use mtdss_ipc::fdpass::{FdMsg, FdMsgHandler, FdReply, KIND_ACK, KIND_DEST_QUERY, KIND_PUBLISH_RESULT, KIND_SOURCE_FD};
#[cfg(unix)]
use serde_json::Value;
#[cfg(unix)]
use std::future::Future;
#[cfg(unix)]
use std::os::fd::OwnedFd;
#[cfg(unix)]
use std::pin::Pin;
#[cfg(unix)]
use tokio::sync::oneshot;
use tokio::task::JoinHandle;
use tokio::time::MissedTickBehavior;

/// 事件方法名:作业状态翻转(立即)与节流进度(≥400ms)。
pub(crate) const EV_TRANSFER: &str = "event.transfer";
/// 进度事件最小间隔(节流):进度帧两两间隔不得小于它;状态翻转帧不受限。
/// jobs 的 transfer 进度与 update.rs 的下载进度共用同一口径。
pub(crate) const PROGRESS_EVENT_MIN_GAP: Duration = Duration::from_millis(400);
/// 速度 EMA 采样窗口(毫秒)。
const SPEED_WINDOW_MS: u64 = 5_000;
/// 速度采样保留上限(窗口过滤之外的无界硬帽,防长跑作业样本无界增长)。
const SPEED_SAMPLE_CAP: usize = 256;
/// 策略上限(全局约束):skip+overwrite 合计条数。
pub(crate) const POLICY_MAX_ENTRIES: usize = 2048;
/// 策略上限(全局约束):单条字节数。
pub(crate) const POLICY_MAX_ENTRY_BYTES: usize = 4096;
/// bridge 上行条目数上限(护栏,v2-P3 T4 评审 Important#1):当前实现为
/// 「驱动源表整表持有逐文件 fd」(懒逐文件请求,表存活至 hash+send
/// 收尾,见 [`SourceResolver`] 文档)——条目数即峰值句柄数,而 Android
/// 进程 fd 软限 1024,扣除基线 socket/文件句柄与并发 try_clone 副本
/// 余量后取 512。超限在 [`Jobs::start_upload_bridge`] 入口即拒(可读
/// 错误指向分批发送),防 EMFILE 在 resolve 处暴露成「源不可用: 等待
/// App 应答超时」的误诊(重试必然再败)。这是 spec §5.2「惰性按需
/// fd……免大目录句柄耗尽」意图的入口级护栏;真·窗口化(逐文件释放/
/// 重试轮内重解析)开跟进票:T9/T11 实机大目录验证后 P4 收敛。
pub(crate) const MAX_BRIDGE_ITEMS: usize = 512;
/// 源端作业态查询持续失败的上界(download/remote_copy 两驱动共用,评审
/// Important:轮询缝查询失败原为无限重试——A 中途下线会让操作端卡片永远
/// running,只有用户取消能救)。连续查询不可达(打洞失败/对端掉线)超过
/// 它 → 本端卡片失败收口,文案可行动(可重试)。单次瞬时失败不烧计数:
/// 任一成功查询即归零;取消走 waiter 臂/任务 abort,即时胜出不受此界约束。
/// 60s ≈ 若干次打洞尝试的自然上界,把「偶发断链」与「真下线」分开。
const SOURCE_POLL_FAIL_BOUND: Duration = Duration::from_secs(60);
/// 事件方法名:bridge 上行逐文件请求源 fd(v2-P3 T4;参数面钉死
/// `{"request_id":u64,"ref":str}`;登记先于广播、request_id 单调、
/// oneshot 有界等待——收件侧 0x04/0x05(DestQueries)沿本模式)。
#[cfg(unix)]
pub(crate) const EV_REQUEST_SOURCE: &str = "fs.request_source";
/// resolve 的应答等待上限(默认 10s):App 端打开 SAF fd(可能触达用户
/// 可见的授权面)的人机时延上界;超窗即「源不可用」收口,不悬挂。
#[cfg(unix)]
pub(crate) const REQUEST_SOURCE_TIMEOUT: Duration = Duration::from_secs(10);
/// 超时常量的测试注入缝(MTDSS_TEST_* 家族口径,构造期一次读定):
/// `MTDSS_TEST_REQUEST_SOURCE_TIMEOUT_MS` = 毫秒值(e2e 注入小值,不真等
/// 10s)。非法值回默认。
#[cfg(unix)]
fn request_source_timeout() -> Duration {
    std::env::var("MTDSS_TEST_REQUEST_SOURCE_TIMEOUT_MS")
        .ok()
        .and_then(|v| v.parse::<u64>().ok())
        .map_or(REQUEST_SOURCE_TIMEOUT, Duration::from_millis)
}
/// 事件方法名:收件箱终件在场查询(F7 修复;参数面钉死
/// `{"request_id":u64,"rel":str,"size":u64}`)。记录面只证明「曾收完」,在场与否由 App 查
/// MediaStore 真实行回答(fdpass `0x04 dest_query`);**P4a 起 id 计数器与
/// 0x05 发布应答共用**(spec §6.5:单计数单通道无错路)。事件面 cfg 无关
/// (查询不触 fd,Windows host 单测与正式面同一路径)。
pub(crate) const EV_QUERY_DEST: &str = "fs.query_dest";
/// query_dest 的应答等待上限(默认 3s):对账路径的在场查询无用户交互面
/// (纯 MediaStore 行查询),须显著快于发布的 30s IO 窗。
pub(crate) const QUERY_DEST_TIMEOUT: Duration = Duration::from_secs(3);
/// 超时常量的测试注入缝(MTDSS_TEST_* 家族口径):
/// `MTDSS_TEST_QUERY_DEST_TIMEOUT_MS` = 毫秒值。
fn query_dest_timeout() -> Duration {
    std::env::var("MTDSS_TEST_QUERY_DEST_TIMEOUT_MS")
        .ok()
        .and_then(|v| v.parse::<u64>().ok())
        .map_or(QUERY_DEST_TIMEOUT, Duration::from_millis)
}
/// 事件方法名:收件发布请求(P4a,spec §3.2;参数面钉死
/// `{"request_id":u64,"rel":str,"staging_path":str,"size":u64}`——rel =
/// 记录 final_rel,发布名与 0x04 键同源,§6.4)。daemon finalize 成功后经
/// 发布协调器广播,App 执行 MediaStore 发布后经 fdpass `0x05
/// publish_result` 回话。
pub(crate) const EV_PUBLISH: &str = "fs.publish";
/// 发布应答等待上限的**基值**:App 侧 insert+流拷贝+揭开 的 IO 上界
/// (大文件拷贝,远长于 0x04 的 3s 纯查询窗);超窗即「保存失败」收口,
/// 暂存保留待重试(§4)。正式超时按尺寸在基值上加算
/// ([`publish_timeout_for_size`],终审 I-1)。
pub(crate) const PUBLISH_TIMEOUT: Duration = Duration::from_secs(30);
/// 发布超时的尺寸附加基数:每 10MB 加 1s(终审 I-1 缓解)。30s 固定墙钟
/// 对多 GB 拷贝必超时——文件实际在场、0x05 迟到被拒收,republish 缺省
/// overwrite 重拷再超时的永久「保存失败」环。MIX 3 级媒体拷贝实测吞吐
/// ~100-200MB/s,每 10MB 1s 预留 ≥10 倍余量(1GB≈+100s≈130s,4GB≈430s);
/// 小文件仍走 30s 基值口径。
const PUBLISH_TIMEOUT_PER_10MB: u64 = 10 * 1024 * 1024;

/// 发布应答等待上限的尺寸感知基线(纯函数,单测钉下界):
/// `PUBLISH_TIMEOUT + size/10MB` 秒。
fn publish_timeout_for_size(size: u64) -> Duration {
    PUBLISH_TIMEOUT + Duration::from_secs(size / PUBLISH_TIMEOUT_PER_10MB)
}

/// 发布超时的测试注入缝(同 [`query_dest_timeout`] 口径):
/// `MTDSS_TEST_PUBLISH_TIMEOUT_MS` = 毫秒值,语义保持**整体覆盖**(含
/// 尺寸附加项:注入即恒定,不随 size 放大);None = 无注入,逐文件按
/// 尺寸感知基线计算。构造期一次读定([`DestQueries`] 持有)。
fn publish_timeout_override() -> Option<Duration> {
    std::env::var("MTDSS_TEST_PUBLISH_TIMEOUT_MS")
        .ok()
        .and_then(|v| v.parse::<u64>().ok())
        .map(Duration::from_millis)
}
/// 持久层进度回写最小间隔(节流,spec §4.1):进度两两落盘间隔不得小于它;
/// 状态翻转与 FileDone 不受限(即时落盘)。窗口内崩溃至多回退到上个检查点
/// 的进度,续传由对账兜底,无数据损坏。与 400ms 事件节流分开:落盘成本
/// (临时件+fsync+rename)高一个量级,5s 足够中断面新鲜。
const STORE_WRITE_MIN_GAP: Duration = Duration::from_secs(5);

/// 是否终态(粘滞:落定后任何再翻转无操作)。
fn is_terminal_state(s: &str) -> bool {
    matches!(s, "done" | "failed" | "cancelled")
}

/// download dir 的收件箱哨兵放行(v2-P3 T6):仅 android(收件箱是
/// android 端的目的地形态);桌面恒 false——`#[cfg]` 两型同一签名,
/// 调用点无 cfg 分叉,桌面判定路径逐字节不变。
#[cfg(target_os = "android")]
fn inbox_dir_ok(dir: &str) -> bool {
    dir == mtdss_core::transport::intake::INBOX_SENTINEL
}

#[cfg(not(target_os = "android"))]
fn inbox_dir_ok(_dir: &str) -> bool {
    false
}

/// 毒锁恢复取锁(M4a-2):std Mutex 在持锁 panic 后永久 PoisonError,旧
/// `.lock().unwrap()` 会把一次 panic 放大成该锁所有后续使用方全部 panic
/// (守护进程连锁崩)。此处 Err(poisoned) → `into_inner()` 取回数据照常
/// 读写——毒化只说明持锁段被打断,数据结构本身未坏(本模块临界区均为
/// 毫秒级简单字段改写)。jobs.rs 定义,flow.rs(SendTracker)复用。
pub(crate) fn lock_ok<T>(m: &Mutex<T>) -> MutexGuard<'_, T> {
    match m.lock() {
        Ok(g) => g,
        Err(poisoned) => poisoned.into_inner(),
    }
}

/// id 排序键(snapshot 与 transfer.list 持久+live 合并后的统一排序共用):
/// 裸数字 id 即数字本身;命名空间 id(`<dev>:<seq>`)取末段冒号后的序号
/// ——两型同一排序面。解析失败的畸形 id 排末尾(防御,不 panic)。
pub(crate) fn job_seq(id: &str) -> u64 {
    id.rsplit(':')
        .next()
        .unwrap_or(id)
        .parse::<u64>()
        .unwrap_or(u64::MAX)
}

/// 当前时刻的 epoch 毫秒(任务记录时间戳;时钟异常回退取 0,尽力而为)。
fn epoch_ms() -> u64 {
    std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .map(|d| d.as_millis() as u64)
        .unwrap_or(0)
}

/// 显示名 = 首项的末段名(本地/远端两型路径统一按双分隔符取末段,Windows
/// 与 Unix 形态远端路径同权;尾随分隔符等无末段输入末段为空串——无害,
/// UI 对空名回退显示 id)。UI 任务名列的单文件形态;多文件/目录由 UI
/// 组合「首项名 · N 项」(spec §7.2)。
fn display_name(first: &str) -> String {
    first
        .rsplit(['/', '\\'])
        .next()
        .unwrap_or(first)
        .to_string()
}

/// 策略上限校验(全局约束:合计 ≤2048 条、单条 ≤4096 字节);upload/
/// download 起作业与 ctrl copy/move RPC(api.rs)共用同一校验面。
pub(crate) fn check_policy_limits(policy: &TransferPolicy) -> Result<(), String> {
    if policy.skip.len() + policy.overwrite.len() > POLICY_MAX_ENTRIES {
        return Err(format!(
            "策略超限:skip+overwrite 合计 {} 条 > 上限 {POLICY_MAX_ENTRIES}",
            policy.skip.len() + policy.overwrite.len()
        ));
    }
    for e in policy.skip.iter().chain(policy.overwrite.iter()) {
        if e.len() > POLICY_MAX_ENTRY_BYTES {
            return Err(format!(
                "策略条目超长: {} 字节 > 上限 {POLICY_MAX_ENTRY_BYTES}",
                e.len()
            ));
        }
    }
    Ok(())
}

// ---------- 清除(transfer.clear,spec §5.2) ----------

/// 清除错误(transfer.clear,spec §5.2):离线可重试,其余业务失败。
#[derive(Debug)]
pub(crate) enum ClearErr {
    /// 对端离线(upload/remote_copy 的残留清理无法送达):任务保留可重试。
    PeerOffline(String),
    /// 其余失败(不存在/done 拒清/对端错误/本地面失败):任务同样保留。
    Other(String),
}

impl ClearErr {
    /// RPC 码位映射(收口处,api 层照此翻 RpcError):离线 3(与 CtrlErr
    /// 同用「设备离线」既有码位),其余 1。
    pub(crate) fn rpc(&self) -> (i64, String) {
        const RPC_PEER_OFFLINE: i64 = 3;
        match self {
            ClearErr::PeerOffline(m) => (RPC_PEER_OFFLINE, m.clone()),
            ClearErr::Other(m) => (1, m.clone()),
        }
    }
}

// ---------- 注册表 ----------

/// 非终态作业计数面(Task 13,托盘 tooltip 数据源):原子计数 + watch 广播。
/// 建队 +1(JobCore::new)、终态落定 -1(transition 终态分支;终态粘滞保证
/// 不重复扣)。watch 供托盘线程同步轮询(与 login watch 同一样板)。
struct CountWatch {
    tx: tokio::sync::watch::Sender<usize>,
    n: std::sync::atomic::AtomicUsize,
}

impl CountWatch {
    fn new() -> Self {
        Self {
            tx: watch::channel(0).0,
            n: std::sync::atomic::AtomicUsize::new(0),
        }
    }

    /// 增减计数并广播(+1 建队 / -1 终态;无接收者时静默丢弃)。增减与
    /// 建队/终态严格配对,不会下溢。
    fn bump(&self, d: isize) {
        let n = if d >= 0 {
            self.n.fetch_add(d as usize, Ordering::Relaxed) + d as usize
        } else {
            self.n.fetch_sub((-d) as usize, Ordering::Relaxed) - (-d) as usize
        };
        let _ = self.tx.send(n);
    }
}

/// 传输作业注册表:id 单调分配(u64 计数,字符串对外),map 持全部作业
/// ——内存面不驱逐终态(M4a 上限退役,持久层即历史面,见模块文档持久层
/// 节;唯一例外 [`Jobs::evict_terminal`]:清除/重发摘记录时同步摘终态条
/// 目);历史行的可见性由 transfer.list 的持久+live 合并承载。
/// 键空间(Task 7 修复):upload/local_copy/incoming 用裸数字自增 id;
/// **download 用 `<本机 device_id>:<序号>` 命名空间**——下载 id 会经
/// orchestrate 回显进源端注册表并复用为 bind 键,嵌操作端 id 后与源端
/// 自增面/其他操作端永不撞号(评审 Critical#1)。
pub(crate) struct Jobs {
    next_id: AtomicU64,
    map: Mutex<HashMap<String, JobHandle>>,
    /// 非终态作业计数 watch(Task 13):托盘 tooltip「N 个传输中」。
    count: Arc<CountWatch>,
    /// bridge 源解析器(v2-P3 T4):upload 驱动(逐文件 request_source)
    /// 与 fdpass 服务端(0x01 交付)两方共用。
    sources: Arc<SourceResolver>,
    /// 收件目的地查询/发布解析器(P4a):InboxDestIo 的 final_state 在场
    /// 查询(0x04)、发布协调器(0x05)与 fdpass 服务端三方共用——统一
    /// id 计数器 + 查询通道(spec §6.5;v2-P3 的 DestResolver/0x02 通道已
    /// 随收件 fd 桥退役)。
    queries: Arc<DestQueries>,
}

/// 注册表条目:核心状态(快照/事件/唤醒)+ 控制面(暂停门/取消 watch/
/// 驱动任务)+ 接收轮槽位(Task 7)。
struct JobHandle {
    core: Arc<JobCore>,
    pause: Arc<PauseGate>,
    cancel_tx: watch::Sender<bool>,
    /// 驱动任务句柄:cancel 取走并 abort(取走即注册表不再持任务面,
    /// 不复活);自然结束后留在槽内(abort 对已完成任务是 no-op)。
    task: Mutex<Option<JoinHandle<()>>>,
    /// resident 接收轮双向句柄槽位(Task 7 接线;cancel/pause 已直达)。
    round: Mutex<Option<RoundHandle>>,
}

/// resident 接收轮与作业的双向句柄(Task 7 download 侧接线;签名钉死)。
pub(crate) struct RoundHandle {
    /// 该轮的取消开关(resident loop 在轮内 select 它)。
    pub cancel: watch::Sender<bool>,
    /// 该轮的暂停门(接收主循环挂起,发送端被 QUIC 流控背压)。
    pub pause: Arc<PauseGate>,
    /// 进度回调(RecvProgress → 注册表):resident loop 装配进 RecvCtl,
    /// 注册表经 [`Jobs::track_recv`] 聚合。
    pub progress: Arc<dyn Fn(&RecvProgress<'_>) + Send + Sync>,
}

/// `bind_incoming` 结果(终审修复):三分支让 resident loop 区分「未命中
/// 降级匿名卡」与「终态拒绝弃轮」——旧 bool 面把两者压成同一值,取消后
/// 在途的来件会被降级匿名卡照常收下(已取消下载的数据落地)。
#[derive(Debug, PartialEq, Eq)]
pub(crate) enum BindOutcome {
    /// 占槽成功;作业级暂停门现值已抄进轮门(轮装配前的暂停不丢)。
    Bound,
    /// 作业存在、种类相符但已终态:该轮取消 watch 已被驱动,调用方弃轮。
    RefusedTerminal,
    /// 无此作业或种类不符(降级匿名卡)。
    Miss,
}

/// 作业快照(transfer.list 的数据源;字段钉死,Task 7/8/11/12 消费)。
pub(crate) struct JobSnap {
    pub id: String,
    pub kind: &'static str,
    /// 显示名(首项末段名;UI 任务名列,spec §7.2)。
    pub name: String,
    pub state: String,
    pub files_done: u64,
    pub files_total: u64,
    pub bytes_done: u64,
    pub bytes_total: u64,
    pub speed_bps: u64,
    /// 策略 skip 命中数(M4b 移交债①,local_copy 驱动回填;各 kind 缺省
    /// 0——传输面 skip 文件按 FileDone 计入 done,不重复计数)。
    pub skipped: u64,
    /// 中继兜底标记(v2-P1c T5):传输走了服务端中继即 true(兜底生效恰
    /// 一次回填;直连恒 false)。
    pub relay: bool,
    pub src: String,
    pub dst: String,
    pub error: Option<String>,
}

/// 驱动起步前的作业静态面(构造期一次定形:预扫计数与展示字段)。
struct JobMeta {
    files_total: u64,
    bytes_total: u64,
    /// rel → 字节(FileDone 无总量信息时补齐 done=total,让 skip 文件也
    /// 计满字节;来源 = 预扫 local_tree_rels)。
    rel_sizes: HashMap<String, u64>,
    /// 预扫 rel 全集(v2-P2 Task 4:建卡落盘的重发原料,重发时 diff
    /// 「消失的文件」;download/remote_copy 预扫不可为,建卡为空,待
    /// Task 8 job_query 回填)。
    rels: Vec<String>,
    /// 显示名(首项末段名)。
    name: String,
    src: String,
    dst: String,
}

/// 发布相位(P4a,spec §6.1):收件箱轮 Ok 落定后、发布协调器驱动的
/// 「保存中→发布确认→终态」状态面。与 `round_ok` 同锁改写
/// ([`JobCore::mark_round_publishing`]:两读之间不出现「轮已落定而发布
/// 未开窗」的假 NotApplicable 窗)。
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum PublishPhase {
    /// 未涉及发布(桌面轮/收件箱轮无待发布件):终态合成不受发布门约束,
    /// 既有行为零变化。
    NotApplicable,
    /// 协调器在途(任务态 `publishing` = 保存中):done 合成等待发布落定,
    /// download 的 10s 宽限只作用于网络相、不得侵入本窗(§6.1)。
    Pending,
    /// 已落定:全发布成功(true,done 合成放行)/有失败(false——协调器
    /// 已直接落 failed「保存失败(N 项)」,卡片终态粘滞)。
    Settled(bool),
}

/// 单作业可变状态(锁内持改;进度回调/控制面/驱动收尾三方共用此锁,
/// 所有操作毫秒级,无长阻塞)。
struct JobMut {
    state: String,
    /// 已完整落盘的 rel(发送侧 FileDone;skip 文件接收端也回 FileDone)。
    done: HashSet<String>,
    /// rel → (done, total):done 跨轮取最大值(与 SendTracker 同口径)。
    file_bytes: HashMap<String, (u64, u64)>,
    /// 速度采样 (ms since started, 累计字节)。
    samples: Vec<(u64, u64)>,
    /// 策略 skip 命中数(M4b 移交债①;local_copy 驱动终局前一次性回填)。
    skipped: u64,
    /// 中继兜底标记(v2-P1c T5;兜底生效即置位,直连恒 false)。
    relay: bool,
    started: Instant,
    last_progress_emit: Option<Instant>,
    /// 持久层进度回写的上次时刻(≥5s 节流,spec §4.1;FileDone 与状态
    /// 翻转不受此窗约束)。
    last_store_write: Option<Instant>,
    /// 下载绑定轮的 Ok 落定标记(Task 8):轮成功不再直接落 done——混合
    /// 后果(源端 partial 预检 missing)的终因只在源端作业态里,先落
    /// done 会被终态粘滞吞掉 failed 映射。轮只记落定事实,终局由
    /// download 驱动按源端态合流收口。
    round_ok: bool,
    /// 发布相位(P4a,收件箱轮专属;见 [`PublishPhase`])。
    publish: PublishPhase,
    error: Option<String>,
}

/// 单作业核心:静态标识 + 可变状态 + 终态唤醒 + 事件出口。
struct JobCore {
    id: String,
    kind: &'static str,
    meta: JobMeta,
    m: Mutex<JobMut>,
    /// 终态长轮询唤醒(transfer.wait);翻转通知用 notify_waiters。
    notify: Notify,
    /// 事件广播出口(与 daemon 全事件面同一 channel)。
    events: tokio::sync::broadcast::Sender<EventFrame>,
    /// 非终态计数面(Task 13):建队 +1(new)/终态 -1(transition),与
    /// 注册表共享同一 CountWatch。
    count: Arc<CountWatch>,
    /// 任务记录持久层句柄(v2-P2 Task 4):Some = 建卡/翻转/进度即时或
    /// 节流镜像进 store(三类网络作业的发起端);None = 不持久化(编排
    /// 回显/匿名 incoming/local_copy——记录只在发起端,spec §2)。
    store: Option<Arc<TaskStore>>,
}

impl JobCore {
    /// 建队(queued):计数 +1(托盘「N 个传输中」面);不在此发事件——
    /// 调用方在建队原子段内发首帧。建卡落盘不在 new 内:调用方(三个
    /// 网络建卡点)持全字段参数面,建队原子段后自行 upsert 全字段。
    fn new(
        id: String,
        kind: &'static str,
        meta: JobMeta,
        events: tokio::sync::broadcast::Sender<EventFrame>,
        count: Arc<CountWatch>,
        store: Option<Arc<TaskStore>>,
    ) -> Self {
        count.bump(1);
        Self {
            id,
            kind,
            meta,
            m: Mutex::new(JobMut {
                state: "queued".into(),
                done: HashSet::new(),
                file_bytes: HashMap::new(),
                samples: Vec::new(),
                skipped: 0,
                relay: false,
                started: Instant::now(),
                last_progress_emit: None,
                last_store_write: None,
                round_ok: false,
                publish: PublishPhase::NotApplicable,
                error: None,
            }),
            notify: Notify::new(),
            events,
            count,
            store,
        }
    }

    /// 锁内快照投影。
    fn snap_locked(&self, m: &JobMut) -> JobSnap {
        let bytes_done: u64 = m.file_bytes.values().map(|(d, _)| *d).sum();
        JobSnap {
            id: self.id.clone(),
            kind: self.kind,
            name: self.meta.name.clone(),
            state: m.state.clone(),
            files_done: m.done.len() as u64,
            files_total: self.meta.files_total,
            bytes_done,
            bytes_total: self.meta.bytes_total,
            speed_bps: crate::ops::ema_speed(&m.samples, SPEED_WINDOW_MS),
            skipped: m.skipped,
            relay: m.relay,
            src: self.meta.src.clone(),
            dst: self.meta.dst.clone(),
            error: m.error.clone(),
        }
    }

    /// 事件帧(进度与翻转共用同一字段面;error 为 null 或字符串)。
    fn emit_locked(&self, m: &JobMut) {
        let snap = self.snap_locked(m);
        let _ = self.events.send(EventFrame {
            method: EV_TRANSFER.into(),
            params: json!({
                "id": snap.id,
                "kind": snap.kind,
                "state": snap.state,
                "files_done": snap.files_done,
                "files_total": snap.files_total,
                "bytes_done": snap.bytes_done,
                "bytes_total": snap.bytes_total,
                "speed_bps": snap.speed_bps,
                "skipped": snap.skipped,
                "relay": snap.relay,
                "error": snap.error,
            }),
        });
    }

    /// 状态翻转。终态粘滞(已终态 → 无操作);非终态同态翻转幂等无事件。
    /// 终态落定:置状态 + 事件即发 + 持久层即时落盘 + 计数 -1(托盘
    /// 「N 个传输中」面)+ notify_waiters(唤醒全部 wait 长轮询)。非终态
    /// 翻转同样即时落盘(spec §4.1 状态翻转即持久,不进进度节流窗)。
    fn transition(&self, new_state: &str, error: Option<String>) {
        let mut m = lock_ok(&self.m);
        if is_terminal_state(&m.state) {
            return; // 终态粘滞:cancel/完成竞速,先到者胜
        }
        if is_terminal_state(new_state) {
            m.state = new_state.to_string();
            m.error = error;
            self.emit_locked(&m);
            self.store_sync(&m); // 终态即时落盘(state/error/finished_ms)
            drop(m);
            self.count.bump(-1); // 终态粘滞保证只扣一次
            self.notify.notify_waiters();
            return;
        }
        if m.state == new_state {
            return;
        }
        m.state = new_state.to_string();
        self.emit_locked(&m);
        self.store_sync(&m); // running/paused 翻转即时落盘
    }

    /// 作业面镜像进持久层(store None = 不持久化:incoming/local_copy/
    /// 编排回显的被动建卡,spec §2 记录只在发起端)。读改写:建卡点已落
    /// 全字段,此处只翻状态/进度/终因面;记录缺席(tasks.json 损坏恢复后
    /// 从空开始)时以快照面补一张最小卡——参数面(重发原料)已不可考,
    /// 留空,面板可见即可。
    fn store_sync(&self, m: &JobMut) {
        let Some(store) = self.store.as_ref() else {
            return;
        };
        let snap = self.snap_locked(m);
        let mut rec = store.get(&self.id).unwrap_or_else(|| TaskRec {
            id: self.id.clone(),
            kind: self.kind.into(),
            name: self.meta.name.clone(),
            ..Default::default()
        });
        rec.state = snap.state;
        rec.error = snap.error;
        rec.files_done = snap.files_done;
        rec.files_total = snap.files_total;
        rec.bytes_done = snap.bytes_done;
        rec.bytes_total = snap.bytes_total;
        if is_terminal_state(&rec.state) {
            rec.finished_ms.get_or_insert_with(epoch_ms);
        }
        store.upsert(&rec);
    }

    /// 驱动起步翻转 running;用户在 queued 期已暂停 → 保持 paused(门在
    /// 流内生效,不制造「状态 running 实际挂死」的假象;resume 后由
    /// transition("running") 对齐)。
    fn begin_running(&self, gate_paused: bool) {
        if gate_paused {
            let m = lock_ok(&self.m);
            if m.state == "paused" {
                return; // 保持暂停态(非终态,无需事件)
            }
        }
        self.transition("running", None);
    }

    /// 暂停/恢复的可见状态翻转(门已由调用方先行翻转)。
    fn transition_paused(&self, paused: bool) {
        self.transition(if paused { "paused" } else { "running" }, None);
    }

    /// 终态落定(done/failed;cancel 走注册表的 mark_cancelled 同一通道)。
    fn finish(&self, state: &str, error: Option<String>) {
        self.transition(state, error);
    }

    /// 发送进度回调(SendProgress → 聚合计数 + 速度采样 + 节流事件)。
    /// 在发送引擎上下文同步调用:只做毫秒级锁内聚合与非阻塞广播。
    fn track_send(&self, p: &SendProgress<'_>) {
        match p {
            SendProgress::FileDone { rel } => self.track_common(Some(rel), None),
            SendProgress::Bytes { rel, done, total } => {
                self.track_common(None, Some((rel, *done, *total)))
            }
        }
    }

    /// 接收进度回调(RecvProgress → 同一聚合通道,Task 7 download/匿名
    /// 来件面):与 SendProgress 同形(FileDone{rel}/Bytes{rel,done,total}),
    /// 注册表侧单一聚合实现,两侧驱动各一行适配——不重复记账数学。
    fn track_recv(&self, p: &RecvProgress<'_>) {
        match p {
            RecvProgress::FileDone { rel } => self.track_common(Some(rel), None),
            RecvProgress::Bytes { rel, done, total } => {
                self.track_common(None, Some((rel, *done, *total)))
            }
        }
    }

    /// 进度聚合共用体:done 集合 + per-rel 字节(done 跨轮取最大,与
    /// SendTracker 同口径)+ 速度采样(单调时刻 + 累计字节,窗口/容量双重
    /// 裁剪)+ 节流事件(首条即发,其后 ≥400ms;终态帧不在此路径)。
    fn track_common(&self, file_done: Option<&str>, bytes: Option<(&str, u64, u64)>) {
        let mut m = lock_ok(&self.m);
        if is_terminal_state(&m.state) {
            return; // 驱动收尾/取消后迟到的回调:丢弃
        }
        match (file_done, bytes) {
            (Some(rel), _) => {
                m.done.insert(rel.to_string());
                // FileDone 不带总量:用预扫 size 补齐(skip 文件无 Bytes 回调,
                // 否则 bytes_done 永远缺它一份)
                let total = m
                    .file_bytes
                    .get(rel)
                    .map(|(_, t)| *t)
                    .or_else(|| self.meta.rel_sizes.get(rel).copied())
                    .unwrap_or(0);
                m.file_bytes.insert(rel.to_string(), (total, total));
            }
            (None, Some((rel, done, total))) => {
                let e = m.file_bytes.entry(rel.to_string()).or_insert((0, total));
                if done > e.0 {
                    e.0 = done;
                }
                e.1 = total;
            }
            (None, None) => return,
        }
        // 速度采样:单调时刻 + 累计字节;窗口/容量双重裁剪
        let ms = m.started.elapsed().as_millis() as u64;
        let bytes_now: u64 = m.file_bytes.values().map(|(d, _)| *d).sum();
        m.samples.push((ms, bytes_now));
        let last_ms = m.samples[m.samples.len() - 1].0;
        m.samples.retain(|(t, _)| last_ms.saturating_sub(*t) <= SPEED_WINDOW_MS);
        if m.samples.len() > SPEED_SAMPLE_CAP {
            let drop_n = m.samples.len() - SPEED_SAMPLE_CAP;
            m.samples.drain(..drop_n);
        }
        // 节流:首条即发(快传也有最早反馈),其后 ≥400ms;终态帧不在此路径
        let due = m
            .last_progress_emit
            .map(|t| t.elapsed() >= PROGRESS_EVENT_MIN_GAP)
            .unwrap_or(true);
        if due {
            m.last_progress_emit = Some(Instant::now());
            self.emit_locked(&m);
        }
        // 持久层进度回写(spec §4.1「≥5s 或 FileDone」):首条即写,其后
        // 窗口内跳过;FileDone 恒写(文件粒度是中断面最后已知值的最小有
        // 意义单位)。状态翻转的落盘走 transition,此处只镜像进度面。
        let store_due = file_done.is_some()
            || m.last_store_write
                .map(|t| t.elapsed() >= STORE_WRITE_MIN_GAP)
                .unwrap_or(true);
        if store_due {
            m.last_store_write = Some(Instant::now());
            self.store_sync(&m);
        }
    }

    /// 本地复制完成面:全部文件 rel 一次性记满 done/bytes(复用 track 通道
    /// 的计数口径——fs 层无逐字节进度,作业卡以预扫计数呈现终值)。不在此
    /// 发事件:调用方随后 finish,终态帧自带终值。
    fn mark_all_done(&self) {
        let mut m = lock_ok(&self.m);
        if is_terminal_state(&m.state) {
            return;
        }
        for (rel, size) in &self.meta.rel_sizes {
            m.done.insert(rel.clone());
            m.file_bytes.insert(rel.clone(), (*size, *size));
        }
    }

    /// 策略 skip 命中数回填(M4b 移交债①):local_copy 驱动在终局前一次
    /// 性写入,终态帧(与 transfer.list 快照)自带终值。已终态(取消竞速
    /// 先到)不回写——粘滞语义下迟到账目无消费者。
    fn record_skipped(&self, n: u64) {
        let mut m = lock_ok(&self.m);
        if is_terminal_state(&m.state) {
            return;
        }
        m.skipped = n;
    }

    /// 中继兜底标记(v2-P1c T5):置位 + 事件即发(卡片/事件面 relay=true,
    /// 终态帧与快照自带)。已终态(兜底生效与收尾的竞速尾)或已置位
    /// (幂等)不重发——迟到标记无消费者。
    fn mark_relay(&self) {
        let mut m = lock_ok(&self.m);
        if is_terminal_state(&m.state) || m.relay {
            return;
        }
        m.relay = true;
        self.emit_locked(&m);
    }

    /// rels 回填(v2-P2 Task 8,spec §4.1):download/remote_copy 的操作端
    /// 驱动在 job_query 轮询拿到源端回带的 rel 集(源端 upload 驱动从预扫
    /// 全集回填)非空时写回 store,作重发 diff 的「全集」原料。只改 rels
    /// 一字段的读改写:与进度回写(store_sync)同一把 store 锁串行,交错
    /// 窗至多回退一个进度检查点/一个回填(均无害)。记录缺席(tasks.json
    /// 损坏恢复后)静默忽略;store=None(被动面)不持久化,同空转。
    fn backfill_rels(&self, rels: &[String]) {
        let Some(store) = self.store.as_ref() else {
            return;
        };
        let Some(mut rec) = store.get(&self.id) else {
            return;
        };
        rec.rels = rels.to_vec();
        store.upsert(&rec);
    }

    /// 当前快照。
    fn snap(&self) -> JobSnap {
        let m = lock_ok(&self.m);
        self.snap_locked(&m)
    }

    fn is_terminal(&self) -> bool {
        is_terminal_state(&lock_ok(&self.m).state)
    }

    /// 下载绑定轮的 Ok 落定标记(Task 8):置位即返回,不改可见状态——
    /// 终局由 download 驱动按源端作业态合流收口(见
    /// [`Jobs::round_complete`] 与 `download_wait_terminal`)。已终态
    /// (取消竞速,轮死前迟到的落定)幂等忽略。
    fn mark_round_ok(&self) {
        let mut m = lock_ok(&self.m);
        if is_terminal_state(&m.state) {
            return;
        }
        m.round_ok = true;
    }

    /// 轮 Ok 落定标记的读取(download 驱动收口判定用)。
    fn round_ok(&self) -> bool {
        lock_ok(&self.m).round_ok
    }

    /// 轮 Ok 落定 + 发布开窗(P4a,收件箱轮专属,常驻循环经
    /// [`Jobs::begin_publish`] 调用):`round_ok` 与 `publish=Pending` **同
    /// 锁原子落定**——download 驱动两侧分开读时不出现「轮已落定而发布
    /// 未开窗」的假 NotApplicable 窗(该窗会把未发布的文件合成 done,
    /// §2 发布时序诚实即死于此);随后翻可见态 `publishing`(保存中,§3.3
    /// ——非终态,重启沿既有语义映射 interrupted)。false = 已终态(取消
    /// 竞速),调用方不得再起协调器。
    fn mark_round_publishing(&self) -> bool {
        let mut m = lock_ok(&self.m);
        if is_terminal_state(&m.state) {
            return false;
        }
        m.round_ok = true;
        m.publish = PublishPhase::Pending;
        drop(m);
        self.transition("publishing", None);
        true
    }

    /// 发布落定(协调器收尾):Settled(ok)。已终态(取消/失败竞速先到)
    /// 幂等忽略——落定事实无消费者。
    fn settle_publish(&self, ok: bool) {
        let mut m = lock_ok(&self.m);
        if is_terminal_state(&m.state) {
            return;
        }
        m.publish = PublishPhase::Settled(ok);
    }

    /// 发布相位读取(download 驱动 done 合成的发布门,§6.1)。
    fn publish_phase(&self) -> PublishPhase {
        lock_ok(&self.m).publish
    }

    /// 网络相起跑的轮落定面重置(P4a T2 fix,评审 Important#1;混合重发
    /// 驱动 [`inbox_republish_driver`] 的相位切换点调用):相位一
    /// (republish)经 [`JobCore::mark_round_publishing`] 置过
    /// `round_ok=true`、settle 置过 `publish=Settled(true)`——两值描述的
    /// 是**相位一**的落定事实,若原样带进相位二,`download_wait_terminal`
    /// 的 done 合成门会被陈旧值短路:源端作业翻 done 与 B 侧轮落定
    /// (begin_publish→mark_round_publishing 重开 Pending 窗)之间有常态
    /// 序窗(~ms 级),窗内轮询见「round_ok 真 + Settled(true)」即合成
    /// done——而真轮随后落定时 [`Jobs::begin_publish`] 的
    /// mark_round_publishing 因终态粘滞返回 false,**缺件的发布协调器永
    /// 不 spawn**,文件滞留暂存(done 拒重发、clear 拒 done——滞留至未来
    /// 同内容重发)。重置后 done 合成重新要求**本相**的轮落定;publish
    /// 相位不必动——其消费门在 round_ok 之后,真轮落定必经
    /// mark_round_publishing 重开 Pending 窗,NotApplicable/Settled(true)
    /// 只会在「本相真值」语境下被读到。已终态(取消竞速)幂等忽略——驱
    /// 动将即返,重置无消费方。首发 download 免疫:round_ok 起步 false,
    /// 本重置是纯混合重发相位切换面的收口。
    fn reset_round_gate_for_network(&self) {
        let mut m = lock_ok(&self.m);
        if is_terminal_state(&m.state) {
            return;
        }
        m.round_ok = false;
    }
}

impl Jobs {
    /// 构造并自持久层**播种 id 计数器**(v2-P2 Task 4 评审 Important):
    /// jobs 内存面每次会话从 1 重数,而 tasks.json 跨会话持有同命名空间
    /// 的记录——不播种,新会话的第 N 个作业会按 id 静默覆盖上一会话的
    /// 第 N 条持久历史(upsert 按 id 覆盖语义是 Task 6 翻新所依,故在
    /// 计数器侧根治,不在 store 侧加存在性检查)。种子 = 持久记录 id 的
    /// job_seq 最大值 + 1(空 store 取 1,保住既有「新 home 从 1 起数」
    /// 的断言面)。本端 store 只含本机铸造的 id(记录只在发起端,spec
    /// §2),全量 max 即「本机 id 前缀」的 max。
    pub(crate) fn new(store: &TaskStore, events: tokio::sync::broadcast::Sender<EventFrame>) -> Self {
        let seed = store
            .list()
            .iter()
            .map(|r| job_seq(&r.id).saturating_add(1))
            .max()
            .unwrap_or(1);
        Self {
            next_id: AtomicU64::new(seed),
            map: Mutex::new(HashMap::new()),
            count: Arc::new(CountWatch::new()),
            sources: Arc::new(SourceResolver::new(events.clone())),
            queries: Arc::new(DestQueries::new(events)),
        }
    }

    /// bridge 源解析器句柄(upload 驱动逐文件 request_source;fdpass 服务
    /// 端 0x01 交付;Daemon::start 的 fdpass 装配与驱动两侧共用)。
    pub(crate) fn source_resolver(&self) -> &Arc<SourceResolver> {
        &self.sources
    }

    /// 收件目的地查询/发布解析器句柄(P4a):收件箱轮(InboxDestIo 的
    /// final_state 在场查询)、发布协调器(fs.publish / 0x05 应答)与
    /// fdpass 服务端(0x04/0x05 交付)三方共用,全进程单实例。
    pub(crate) fn dest_queries(&self) -> &Arc<DestQueries> {
        &self.queries
    }

    /// 非终态作业计数 watch 接收端(Task 13):托盘线程同步轮询「N 个
    /// 传输中」,与 login watch 同一样板。消费者(托盘)与单测各自存活:
    /// 托盘仅 Windows,单测双平台——any(windows, test) 门住,Linux 正式
    /// 构建不编译(否则 dead_code)。
    #[cfg(any(windows, test))]
    pub(crate) fn count_rx(&self) -> watch::Receiver<usize> {
        self.count.tx.subscribe()
    }

    /// 非终态作业数(M4b Task 10):原子读 CountWatch 计数。paused 计入
    /// ——暂停不是终态(D12:无独立 stalled 态,stalled 即 running);
    /// 更新器空闲门(Task 11)据此在有传输(含已暂停)时拒绝 apply。
    // Task 11 的消费点(update.rs 空闲门)仅在 Windows 装配;非 Windows
    // 侧唯一消费者是本模块单测,按平台保留 allow 防 ubuntu CI dead_code。
    #[cfg_attr(not(windows), allow(dead_code))]
    pub(crate) fn active_count(&self) -> usize {
        self.count.n.load(Ordering::Relaxed)
    }

    /// 注册表插入路径(常态无驱逐:持久层即历史面,内存面全保留;唯一的
    /// 摘除出口见 [`Jobs::evict_terminal`])。upload/download/remote_copy/
    /// local_copy/incoming 各建队点与测试装配共用。
    fn insert_job(&self, id: String, h: JobHandle) {
        lock_ok(&self.map).insert(id, h);
    }

    /// 终态作业条目的注册表摘除(终审修复#2):清除成功删记录 / 重发摘旧
    /// 翻新时调用——持久记录已不在,其终态 live 行失去消费面(此前幽灵至
    /// 重启,行上按钮还全报「任务不存在」)。安全面:仅摘终态条目——驱动
    /// 已收尾(计数已在终态翻转时扣减,摘除不再动计数),done/failed 的
    /// JoinHandle 是自然完成面、cancelled 的 abort 已在 cancel 时发出,丢
    /// 弃句柄无复活面;wait 长轮询 waiter 手持 Arc<JobCore> 不受影响。终
    /// 态检查是防误用的防御面:live 态条目本就不该走到这里(clear 有 live
    /// 拒清门,reinitiate 仅 failed/interrupted 可达),真走进来即不动。
    fn evict_terminal(&self, id: &str) {
        let mut map = lock_ok(&self.map);
        if map.get(id).is_some_and(|h| h.core.is_terminal()) {
            map.remove(id);
        }
    }

    /// 起一个 upload 作业(自动 id;download 编排的回显入口走
    /// [`Jobs::accept_orchestration`] → [`Jobs::start_upload_internal`])。
    ///
    /// 校验(超限/非法即 Err → RPC code 1):files 非空;dir 合法(Task 1
    /// 绝对形态口径,不接受空 = 本 RPC 显式携带定向目录,与遗留 send 的
    /// 可缺省语义分开);策略上限(全局约束:合计 ≤2048 条、单条 ≤4096
    /// 字节)。预扫为空(输入缺失/全符号链接)同样 Err——作业化传输有
    /// 同步反馈面,不把注定失败的编排放进异步驱动。
    pub(crate) async fn start_upload(
        &self,
        state: &Arc<DaemonState>,
        files: Vec<PathBuf>,
        to: &str,
        dir: &str,
        policy: TransferPolicy,
        cut_src: bool,
    ) -> Result<String, String> {
        self.start_upload_internal(
            state,
            None,
            UploadSource::Paths(files),
            to,
            dir,
            policy,
            cut_src,
            Some(state.tasks()),
            &[],
        )
        .await
    }

    /// 起一个 bridge 形态的 upload 作业(transfer.start `source:"bridge"`,
    /// v2-P3 T4):与路径形态共用驱动与终态面,差别仅在源表——预扫无盘面
    /// (items 自带 {ref,rel,size,mtime} 元数据:计数/rels/rel_sizes 直构
    /// 自声明值),驱动起步经 [`SourceResolver`] 逐文件 request_source
    /// 解析 fd(懒解析:transfer.start 只建卡,不触 fd)。taskstore
    /// src_paths 落 `bridge://{ref}` 串——P2 重发/清除语义原样承袭:
    /// bridge 引用不可当路径预扫,reinitiate 按「源已不存在」优雅失败
    /// (记录保留,spec 裁定 6)。cut 恒 false 由 Android UI 保证(spec
    /// §5.2 裁定④);引擎侧 bridge 形态的 cut 收尾为 no-op(不 panic)。
    ///
    /// 校验(Err → RPC code 1):**条目数 ≤ [`MAX_BRIDGE_ITEMS`]**
    /// (入口即拒,先于其余校验——防 fd 峰值击穿 Android 软限,见常量
    /// 文档);items 非空;条目 ref 非空、rel 合法
    /// (`validate_rel_path`:拒 `.mtdss` 首段/反斜杠/盘符,与清单校验
    /// 同口径)且不重复;dir 合法;策略上限;已登录。size/mtime 为 App
    /// 声明值,仅进计数/记录面(清单 size 以 fd 的 fstat 为准,声明值
    /// 偏差不破传输、只影响进度显示)。
    pub(crate) async fn start_upload_bridge(
        &self,
        state: &Arc<DaemonState>,
        items: Vec<BridgeItem>,
        to: &str,
        dir: &str,
        policy: TransferPolicy,
        cut_src: bool,
    ) -> Result<String, String> {
        // 条目数护栏(评审 Important#1,先于一切校验立即拒——超限单必
        // 然 EMFILE,后续校验/建卡全是无效功;依据见 MAX_BRIDGE_ITEMS)
        if items.len() > MAX_BRIDGE_ITEMS {
            return Err(format!(
                "桥接条目过多({} 项,上限 {MAX_BRIDGE_ITEMS}):请分批发送",
                items.len()
            ));
        }
        self.start_upload_internal(
            state,
            None,
            UploadSource::Bridge(items),
            to,
            dir,
            policy,
            cut_src,
            Some(state.tasks()),
            &[],
        )
        .await
    }

    /// start_upload/start_upload_bridge 的内部本体:`explicit_id` 为
    /// Some 时采用外部作业 id(download 编排的回显,Task 7),已存在即
    /// Err(操作端重试换新 id,不撞);None 走单调计数分配。`store` 为发起端持久层句柄(建卡即
    /// 落盘,v2-P2 Task 4);编排回显([`Jobs::accept_orchestration`],被动
    /// 收件面)传 None——记录只在发起端,spec §2。`old_rels` 非空 = 重发
    /// 部分预检模式(v2-P2 Task 6 [`Jobs::reinitiate`]):顶层已消失的输入
    /// 收进 missing 不再整单 Err,在的文件照常驱动,消失项按 old_rels 差
    /// 集推流前尽力清理对端残留;空 = 普通首发,既有口径不动。
    #[allow(clippy::too_many_arguments)] // 内部本体吃下 start_upload/start_upload_bridge 与编排回显两侧的参数面
    async fn start_upload_internal(
        &self,
        state: &Arc<DaemonState>,
        explicit_id: Option<String>,
        source: UploadSource,
        to: &str,
        dir: &str,
        policy: TransferPolicy,
        cut_src: bool,
        store: Option<Arc<TaskStore>>,
        old_rels: &[String],
    ) -> Result<String, String> {
        // 空表检查先行(与泛化前同序:files 缺失先于 dir 校验报错)
        match &source {
            UploadSource::Paths(files) if files.is_empty() => {
                return Err("files 不能为空".into());
            }
            UploadSource::Bridge(items) if items.is_empty() => {
                return Err("items 不能为空".into());
            }
            _ => {}
        }
        // 授权根守卫(v2-P5 T2,spec §3「授权目录集」的发送源正身):send
        // 读取的本机源路径必须落在授权集内(iOS 形态注入:收件箱 + 信任
        // 目录);未配置(桌面默认)直通,行为不变。编排回显与重发都汇入
        // 本体,一并收口。先于 dir/策略/登录检查 fail-fast;Bridge 源是
        // ref 句柄(无本机路径面),不涉守卫。
        if let UploadSource::Paths(files) = &source {
            for f in files {
                state.guard().check(f)?;
            }
        }
        // dir 属于**接收设备**(upload 目标/编排回显 target_dir),平台未知
        // ——平台无关绝对形态预检(公网跨平台:Windows↔Linux 互通),真
        // 合法性由接收端落地时按其自身平台判定(非法 warn+回退默认目录)。
        if !crate::flow::remote_absolute_form(dir) {
            return Err(format!("dir 非法(须为绝对路径,Windows 或 Unix 形态): {dir}"));
        }
        check_policy_limits(&policy)?;
        if state.session().is_none() {
            return Err("未登录,请先 login.verify".into());
        }
        // 源形态分支(v2-P3 T4):预扫 + 建卡元数据 + src_paths 序列化。
        // - Paths:全树 rel 预扫(只读元数据;整树元数据遍历是阻塞调用,
        //   落线程池,直跑占死 async worker,与 ctrl 面同一收口纪律)。重发
        //   路径(old_rels 非空)同池顺带做部分预检:顶层已消失的输入收进
        //   missing(存在性 stat 也是阻塞面,同池做),驱动面只带在的文件;
        //   全缺失 → present 空,沿「无可传内容」既有规则拒绝发起(口径同
        //   空目录)。dropped = old_rels − 新预扫 rels:推流前让对端清
        //   `.mtdss` 残留(spec §5.3/§5.1;尽力而为,目录 rel 幂等跳过)。
        // - Bridge:无盘面——计数/rels/rel_sizes 直构自条目声明值;ref/rel
        //   合法性与 rel 重复在入口收口(路径形态的同类检查分散在
        //   collect_items 与清单校验,bridge 无树可走,建卡前即拒);
        //   missing/dropped 恒空(bridge 无重发编排入口——bridge 记录的
        //   reinitiate 走 Paths 臂的「源已不存在」优雅失败)。
        // 驱动面源表与建卡面各取所需:驱动带 present(部分预检滤后)或
        // 原条目;src_paths 建卡取原始全列表(翻新原料不缩水)。
        let (meta, driver_source, src_paths, missing, dropped): (
            JobMeta,
            UploadSource,
            Vec<String>,
            Vec<String>,
            Vec<String>,
        ) = match source {
            UploadSource::Paths(files) => {
                type PrescanOut = (Vec<PathBuf>, Vec<(String, bool, u64)>, Vec<String>);
                let prescan_files = files.clone();
                let partial_mode = !old_rels.is_empty();
                let (present_files, rels, missing): PrescanOut =
                    tokio::task::spawn_blocking(move || {
                        let (present, missing) = if partial_mode {
                            let mut present = Vec::with_capacity(prescan_files.len());
                            let mut missing = Vec::new();
                            for f in prescan_files {
                                match std::fs::symlink_metadata(&f) {
                                    Ok(_) => present.push(f),
                                    Err(_) => missing.push(f.to_string_lossy().into_owned()),
                                }
                            }
                            (present, missing)
                        } else {
                            (prescan_files, Vec::new())
                        };
                        let rels = crate::ops::local_tree_rels(&present);
                        (present, rels, missing)
                    })
                    .await
                    .map_err(|e| format!("扫描任务失败: {e}"))?;
                let mut rel_sizes = HashMap::new();
                let mut files_total = 0u64;
                let mut bytes_total = 0u64;
                for (rel, is_dir, size) in &rels {
                    if !is_dir {
                        files_total += 1;
                        bytes_total += *size;
                        rel_sizes.insert(rel.clone(), *size);
                    }
                }
                if files_total == 0 {
                    return Err("无可传内容(输入缺失、空目录或全为符号链接)".into());
                }
                let src = files
                    .iter()
                    .map(|p| p.to_string_lossy().into_owned())
                    .collect::<Vec<_>>()
                    .join(", ");
                let meta = JobMeta {
                    files_total,
                    bytes_total,
                    rel_sizes,
                    // 预扫 rel 全集(含目录项:重发 diff 的「全集」口径,清理面
                    // 对目录 rel 天然幂等跳过)
                    rels: rels.iter().map(|(rel, _, _)| rel.clone()).collect(),
                    name: display_name(&files[0].to_string_lossy()),
                    src,
                    dst: format!("{to}:{dir}"),
                };
                let dropped: Vec<String> = if old_rels.is_empty() {
                    Vec::new()
                } else {
                    let fresh: HashSet<&str> = rels.iter().map(|(r, _, _)| r.as_str()).collect();
                    old_rels
                        .iter()
                        .filter(|r| !fresh.contains(&r.as_str()))
                        .cloned()
                        .collect()
                };
                (
                    meta,
                    // 驱动面只带在的文件(部分预检筛除已消失项)
                    UploadSource::Paths(present_files),
                    files
                        .iter()
                        .map(|p| p.to_string_lossy().into_owned())
                        .collect(),
                    missing,
                    dropped,
                )
            }
            UploadSource::Bridge(items) => {
                let mut seen = HashSet::with_capacity(items.len());
                for it in &items {
                    if it.r#ref.is_empty() {
                        return Err(format!("条目 ref 不能为空: {}", it.rel));
                    }
                    if !mtdss_core::protocol::manifest::validate_rel_path(&it.rel) {
                        return Err(format!("条目 rel 非法: {}", it.rel));
                    }
                    if !seen.insert(it.rel.clone()) {
                        return Err(format!("重复 rel: {}", it.rel));
                    }
                }
                let mut rel_sizes = HashMap::with_capacity(items.len());
                let mut bytes_total = 0u64;
                for it in &items {
                    bytes_total += it.size;
                    rel_sizes.insert(it.rel.clone(), it.size);
                }
                let meta = JobMeta {
                    files_total: items.len() as u64,
                    bytes_total,
                    rel_sizes,
                    // rel 全集 = 条目声明面(清除/重发 diff 的原料,同路径形态)
                    rels: items.iter().map(|i| i.rel.clone()).collect(),
                    name: display_name(&items[0].rel),
                    src: items
                        .iter()
                        .map(|i| format!("bridge://{}", i.r#ref))
                        .collect::<Vec<_>>()
                        .join(", "),
                    dst: format!("{to}:{dir}"),
                };
                // src_paths 序列化:bridge 条目 → `bridge://{ref}` 串(P2
                // 重发/清除语义原样承袭;rel 独立在 rec.rels 面)
                let src_paths = items
                    .iter()
                    .map(|i| format!("bridge://{}", i.r#ref))
                    .collect();
                (
                    meta,
                    UploadSource::Bridge(items),
                    src_paths,
                    Vec::new(),
                    Vec::new(),
                )
            }
        };

        let id = match explicit_id {
            Some(id) => {
                if lock_ok(&self.map).contains_key(&id) {
                    return Err(format!("任务 id 已存在,拒绝编排回显: {id}"));
                }
                // 计数器前推(评审 Critical#1a):外部 id 若为纯数字且 ≥ 自增
                // 面计数器,立即把计数器推到它之上——否则后续自增 mint 会
                // 造出同号作业,注册表插入静默覆盖回显条目(驱动 JoinHandle
                // 丢失、作业孤儿化)。命名空间 id(现役操作端形态)不含纯数
                // 字,天然不撞;此守卫兜住旧版/异常的纯数字回显。
                if let Ok(n) = id.parse::<u64>() {
                    self.next_id.fetch_max(n.saturating_add(1), Ordering::Relaxed);
                }
                id
            }
            None => self.next_id.fetch_add(1, Ordering::Relaxed).to_string(),
        };
        let (cancel_tx, cancel_rx) = watch::channel(false);
        let pause = PauseGate::new();
        let core = Arc::new(JobCore::new(
            id.clone(),
            "upload",
            meta,
            state.events(),
            self.count.clone(),
            store.clone(),
        ));
        // 建队原子段:首帧事件(queued)在插入注册表前发出——job_id 尚未
        // 返回给调用方,不存在「外部已见事件却查无此业」的窗口。
        {
            let m = lock_ok(&core.m);
            core.emit_locked(&m);
        }
        // 建卡即落盘(v2-P2 Task 4,spec §4.1):queued 全字段入 store。
        // 先于驱动起步,窗口内崩溃至多回退为 interrupted(无损);编排
        // 回显(被动收件)store=None 不持久化——记录只在发起端。
        if let Some(store) = store.as_ref() {
            store.upsert(&TaskRec {
                id: id.clone(),
                kind: "upload".into(),
                name: core.meta.name.clone(),
                from_dev: state.self_device_id().unwrap_or_default(),
                to_dev: to.to_string(),
                src_paths,
                target_dir: dir.to_string(),
                policy_skip: policy.skip.clone(),
                policy_overwrite: policy.overwrite.clone(),
                cut: cut_src,
                rels: core.meta.rels.clone(),
                files_total: core.meta.files_total,
                bytes_total: core.meta.bytes_total,
                state: "queued".into(),
                created_ms: epoch_ms(),
                ..Default::default()
            });
        }
        // spawn 至插入之间无 await 点:spawn 不会立即执行,插入先于驱动
        // 首步,transfer.cancel 不存在找不到条目的竞窗。驱动包监督网(supervise_
        // driver):panic 意外退出 → failed 收口,JoinHandle 语义不变。块内
        // shadow:借用形参(&state/&to/&dir)先定形为 owned,core/pause 另
        // 克隆给驱动任务,块外原绑定留给 JobHandle。
        let driver = {
            let state = state.clone();
            let core = core.clone();
            let pause = pause.clone();
            let to = to.to_string();
            let dir = dir.to_string();
            tokio::spawn(supervise_driver(
                core.clone(),
                async move {
                    upload_driver(
                        state,
                        core,
                        UploadJob {
                            // 驱动面源表 = present(部分预检滤后)或 bridge 原条目;
                            // src_paths 建卡取原参数全列表(翻新原料不缩水)
                            source: driver_source,
                            to,
                            dir,
                            policy,
                            cut_src,
                            missing,
                            dropped,
                        },
                        pause,
                        cancel_rx,
                    )
                    .await
                },
            ))
        };
        self.insert_job(
            id.clone(),
            JobHandle {
                core,
                pause,
                cancel_tx,
                task: Mutex::new(Some(driver)),
                round: Mutex::new(None),
            },
        );
        Ok(id)
    }

    /// 接受跨设备编排(ctrl `orchestrate` 响应分支,Task 7):在**源端**
    /// 起一个 upload 作业,回显操作端预先注册的 job_id(ConnectRequest 随
    /// 之携带,操作端 resident loop 据 id 绑定路由)。现役操作端 id 带
    /// 操作端命名空间(`opDev:seq`,见 `start_download`),与源端自身自增
    /// 键空间天然隔离;旧版纯数字 id 仍按原样回显(存在即拒 + 计数器前
    /// 推兜底,见 `start_upload_internal`)。校验:job_id 非空(空 id 无法
    /// 回绑)、to_device 非空、id 未被占用;其余校验(路径存在性/目录形
    /// 态/策略上限/登录)沿用 start_upload 的同步反馈面,失败文案经
    /// OrchestrateResponse.detail 回给操作端落 failed。`old_rels` 非空 =
    /// 重发编排(v2-P2 Task 8,reinitiate download/remote_copy 的源端半边):
    /// partial 预检模式(顶层已消失项收进 missing,dropped 由本端驱动推流
    /// 前统一清接收端残留);空 = 普通首发。
    #[allow(clippy::too_many_arguments)] // 编排回显面参数齐平 OrchestrateRequest 契约形态(重发侧多 old_rels 一位)
    pub(crate) async fn accept_orchestration(
        &self,
        state: &Arc<DaemonState>,
        job_id: &str,
        to_device: &str,
        paths: &[String],
        target_dir: &str,
        policy: TransferPolicy,
        old_rels: &[String],
    ) -> Result<(), String> {
        if job_id.is_empty() {
            return Err("orchestrate 缺 job_id(无法回绑)".into());
        }
        if to_device.is_empty() {
            return Err("orchestrate 缺 to_device".into());
        }
        let files: Vec<PathBuf> = paths.iter().map(PathBuf::from).collect();
        // cut 恒 false:download/remote_copy 的剪切收尾在操作端(成功后对
        // 源端 ctrl delete),源端 upload 驱动不动源。store=None:编排回显
        // 是**被动收件面**(spec §2 记录只在发起端)——源端不建持久记录。
        self.start_upload_internal(
            state,
            Some(job_id.to_string()),
            UploadSource::Paths(files),
            to_device,
            target_dir,
            policy,
            false,
            None,
            old_rels,
        )
        .await?;
        Ok(())
    }

    /// 重新发起(重发,v2-P2 Task 6/8 + P4a T2 收件箱重发分区):持久记录
    /// 校验 → 摘旧记录 → 以原参数加新 job_id 翻新建卡重发。**不自动复活**:
    /// 一次调用一次驱动;对端离线照普通作业退避口径(可注入预算),耗尽标
    /// 失败。
    ///
    /// 校验:记录存在;kind ∈ {upload,download,remote_copy};state ∈
    /// {failed,interrupted}(已完成不开,running/paused/queued 拒——单测
    /// `reinitiate_rejects_done_and_running` 钉死)。**P4a T2 收件箱放行
    /// 面**(spec §6.3 裁定 3):download + target_dir=收件箱哨兵的记录先
    /// 预扫暂存分区([`crate::inbox_dest::republish_partition`]),分区为
    /// **republish-only(缺失集空、staged 非空)时 cancelled 亦放行**——
    /// republish 资格=暂存记录 done,**不限任务显示态**(数据在本地,拒发
    /// 布即无理;P2 的「cancelled 拒重发」约束的是**网络重发**,本地发布
    /// 经「重新发起」入口分流,语义注释钉 spec §6.3);混合/纯网络分区仍
    /// 沿 P2 门(cancelled 拒——网络相不得因发布分流而复活)。
    ///
    /// **幂等门(评审 carry)**:`store.get` 返回克隆,并发 reinitiate 可能
    /// 双双过校验门 → `remove` 的命中返回值是唯一原子裁决:未命中 = 已被
    /// 并发重发摘走,按存在性报错,不造第二个并行驱动(单测
    /// `reinitiate_concurrent_calls_mint_exactly_one_job` 钉死)。
    ///
    /// 翻新换 id:旧记录 `store.remove` 后新 id 由自增面铸造——注册表无驱
    /// 逐,同号再插入会静默覆盖旧条目(驱动 JoinHandle 丢失);计数器播种
    /// /回显前推保证新 id 高于全部历史 id(单测
    /// `reinitiate_upload_survives_stale_peer_job_id` 钉死)。记录面以新 id
    /// 重新建卡(原 src_paths 等参数原样携带,upload 的 rels 换新预扫集;
    /// download/remote_copy 建卡 rels 空,待重发轮 job_query 回填——
    /// **收件箱重发例外:rels 原样携带**,分区语义见
    /// [`Jobs::start_download_republish`])。
    ///
    /// 记录面两守(终审修复):摘旧成功即同步逐出该 id 的终态 live 行(行
    /// 随记录走,不再幽灵至重启,见 [`Jobs::evict_terminal`]);起动失败(全
    /// 源消失「无可传内容」/未登录等)原记录回插——记录不自动删除(spec
    /// 裁定 6;单测 `reinitiate_all_sources_missing_keeps_record` 钉死)。
    ///
    /// 四类分臂:
    /// - **upload**(T6):本地 partial 预检,`old_rels = rec.rels`,已消失
    ///   项收进 missing、dropped 推流前尽力清对端残留。
    /// - **download 非收件箱 / remote_copy**(T8):重发编排——orchestrate
    ///   携 `old_rels = rec.rels` 给源端,源端以同一 partial 机制预检,
    ///   dropped 由**源端**在推流前统一清接收端残留(spec §5.1);rec.rels
    ///   空(尚未回填)= 退化重发,行为正确只是残留清理面窄。
    /// - **download 收件箱**(P4a T2,spec §6.3):**重发分区**——预扫暂存
    ///   记录:done 未发布件 → staged(纯本地 republish 分支,免登录/免
    ///   对端,直接驱动协调器重发 fs.publish);缺失件 → missing(走既有
    ///   网络分支);done 已发布件 → 无动作面(网络相经 0x04 在场查询
    ///   Skip 维持)。**同一新 job** 翻新承载两相
    ///   ([`Jobs::start_download_republish`]);分区保证 staged 件永不进
    ///   接收集——引擎 Full 截断与 unique_target 改名 "(1)" 均无从触及。
    pub(crate) async fn reinitiate(
        &self,
        state: &Arc<DaemonState>,
        id: &str,
    ) -> Result<String, String> {
        let rec = state
            .tasks()
            .get(id)
            .ok_or_else(|| format!("任务不存在,无法重新发起: {id}"))?;
        if !matches!(rec.kind.as_str(), "upload" | "download" | "remote_copy") {
            return Err(format!("任务种类不可重新发起: {}", rec.kind));
        }
        // 收件箱 download 记录的重发分区(P4a T2,spec §6.3):预扫私有暂存
        // 记录(阻塞 fs 面,落线程池)——done 未发布且终件在场 → staged;
        // 无记录/未完成/终件缺失 → missing;已发布 → 无动作面。分区先于
        // 资格门:republish-only 的 cancelled 放行需要分区结果(见函数文
        // 档);非收件箱记录零开销直过(分区两集恒空)。
        let inbox = rec.kind == "download"
            && rec.target_dir == mtdss_core::transport::intake::INBOX_SENTINEL;
        let (staged, missing): (Vec<crate::inbox_dest::PublishItem>, Vec<String>) = if inbox {
            let home = state.home();
            let rels = rec.rels.clone();
            tokio::task::spawn_blocking(move || {
                crate::inbox_dest::republish_partition(&home, &rels)
            })
            .await
            .map_err(|e| format!("重发分区扫描任务收尾失败: {e}"))?
        } else {
            (Vec::new(), Vec::new())
        };
        // 缺失集空且 staged 非空 = 纯本地重发布(网络相不起,免登录/免对端)
        let republish_only = inbox && !staged.is_empty() && missing.is_empty();
        if !matches!(rec.state.as_str(), "failed" | "interrupted")
            && !(republish_only && rec.state == "cancelled")
        {
            return Err(format!(
                "任务不可重新发起(仅失败/中断可重发,当前 {}): {id}",
                rec.state
            ));
        }
        // 幂等门:remove 命中=false = 并发重发已摘走记录,存在性报错(见
        // 上方文档;get→remove 之间的窗口由 TaskStore 锁内原子裁决收口)
        if !state.tasks().remove(id) {
            return Err(format!("任务不存在,无法重新发起: {id}"));
        }
        // 摘旧成功 → 终态 live 行同步逐出(终审修复#2:行随记录走,不再
        // 幽灵至重启;驱动已终态,安全面见 evict_terminal)
        self.evict_terminal(id);
        let policy = TransferPolicy {
            skip: rec.policy_skip.clone(),
            overwrite: rec.policy_overwrite.clone(),
        };
        // 四类分臂起动重发;起动失败(全源消失「无可传内容」/未登录等)→
        // 原记录回插(终审修复#1:记录不自动删除,spec 裁定 6——否则记录
        // 无声蒸发,对端 .mtdss 残留也失去清除入口),保持 failed/interrupted
        // 等用户重试。
        let started = match rec.kind.as_str() {
            // bridge 形态的记录(src_paths 为 `bridge://{ref}` 串)同样落
            // 此臂:引用不可当路径预扫 → partial 预检全 missing →
            // 「无可传内容」Err,原记录回插保留——P2 重发语义原样承袭
            // (优雅失败,不 panic;实机语义见计划 T10「分享件重发=源已
            // 不存在」)
            "upload" => {
                let files: Vec<PathBuf> = rec.src_paths.iter().map(PathBuf::from).collect();
                self.start_upload_internal(
                    state,
                    None,
                    UploadSource::Paths(files),
                    &rec.to_dev,
                    &rec.target_dir,
                    policy,
                    rec.cut,
                    Some(state.tasks()),
                    &rec.rels,
                )
                .await
            }
            // 收件箱 download(P4a T2):重发分区翻新——同一新 job 承载
            // republish 相(staged 集纯本地重发布,先于网络相落定)+ 网络
            // 相(缺失集走既有编排);dir 沿记录哨兵原值(重放信任原判,
            // 见 starter 文档)。
            "download" if inbox => {
                self.start_download_republish(state, &rec, staged, republish_only)
            }
            // download/remote_copy 重发 = 翻新 + 原参数 + old_rels=rec.rels
            // 重发编排:start_download/start_remote_copy 同步建卡并 spawn
            // 驱动,驱动 orchestrate 携旧 rel 集,源端 partial 预检 + dropped
            // 统一清接收端残留(spec §5.1)。
            "download" => self.start_download(
                state,
                &rec.from_dev,
                rec.src_paths.clone(),
                &rec.target_dir,
                policy,
                rec.cut,
                &rec.rels,
            ),
            "remote_copy" => self.start_remote_copy(
                state,
                &rec.from_dev,
                rec.src_paths.clone(),
                &rec.to_dev,
                &rec.target_dir,
                policy,
                rec.cut,
                &rec.rels,
            ),
            _ => unreachable!("上方 matches 已收口种类"),
        };
        match started {
            Ok(new_id) => Ok(new_id),
            Err(e) => {
                state.tasks().upsert(&rec);
                Err(e)
            }
        }
    }

    /// 清除一条任务记录并回收其残留面(transfer.clear,spec §5.2「各清各
    /// 的」;api 层码位映射见 [`ClearErr::rpc`]):
    ///
    /// - **记录存在性在 store 判定**:不在 store 的 id(不存在/已被并发
    ///   清除或翻新)清除即报不存在。
    /// - **done 拒清**(spec §5.2「完成 | 无操作入口…不动」的 daemon 侧落
    ///   法):不删任何东西——对端 done 记录是后续 SKIP 素材,清了会让同
    ///   内容再发失去对账锚点。
    /// - **live 态拒清**(queued/running/paused,评审 Important):进行中
    ///   清除会搅在途传输面,且驱动回写会重建记录(清除效果不持久)——
    ///   请先取消或等待终态;门在空 rels 短路之前(download 当期 rels 恒
    ///   空,位置错了会被绕过)。
    /// - **rels 为空** → 直接删记录(无可清面;download 的 rels 待 Task 8
    ///   job_query 回填,当期建卡为空,清除走此退化路径)。
    /// - **download** → 本地残留清理(目标目录 `.mtdss` 记录与 `.part`;
    ///   收件箱记录 P4a 起清私有暂存区 `{home}/inbox-staging`——记录 +
    ///   `.part` + 未发布暂存终件,经 `inbox_dest::cleanup_inbox`;
    ///   恒可执行,不依赖对端在线)→ 删记录。
    /// - **upload/remote_copy(含 cancelled 终态)** → 对端
    ///   `cleanup_outbox(dir, rels)` 回执 Ok 才删本地记录;对端离线 →
    ///   [`ClearErr::PeerOffline`],任务保留可重试。
    ///
    /// 删除成功的每条路径,该 id 的终态 live 条目同步逐出注册表(终审修复
    /// #2:行随记录走,不再幽灵至重启;见 [`Jobs::evict_terminal`] 的安全
    /// 面)。live 态作业被拒清门挡住,不经任何删除路径;失败/保留路径不清
    /// live 面(行留着,按钮可重试清除)。
    pub(crate) async fn clear(&self, state: &Arc<DaemonState>, id: &str) -> Result<(), ClearErr> {
        let rec = state
            .tasks()
            .get(id)
            .ok_or_else(|| ClearErr::Other(format!("任务不存在,无法清除: {id}")))?;
        if rec.state == "done" {
            return Err(ClearErr::Other(format!("已完成任务无可清除面: {id}")));
        }
        if matches!(rec.state.as_str(), "queued" | "running" | "paused" | "publishing") {
            // live 态拒清(spec §5.2 可用性矩阵:进行中只有暂停·取消):进行
            // 中清除会搅在途传输面(upload 臂在对端仍接收时删其 `.mtdss`/
            // `.part`,publishing 臂在协调器发布中途删暂存件与记录),且
            // store.remove 后驱动进度/终态回写经 store_sync 重建记录——Ok
            // 而清除效果不持久。位置在空 rels 短路**之前**:download 当期
            // rels 恒空,门放短路之后会被绕过;Task 8 回填 rels 后运行中
            // download 的清除会真删在途接收文件,危害只增不减。
            return Err(ClearErr::Other(format!(
                "任务进行中,请先取消或等待结束: {id}"
            )));
        }
        if rec.rels.is_empty() {
            // 无可清面(含 download 待回填的退化路径):直接删记录,不触网络
            state.tasks().remove(id);
            self.evict_terminal(id);
            return Ok(());
        }
        match rec.kind.as_str() {
            "download" => {
                // 本地残留清理(阻塞 fs 面,落线程池,与 ctrl 面同一收口
                // 纪律):普通记录清目标目录 `.mtdss`;收件箱记录(P4a,
                // target_dir == INBOX_SENTINEL)的残留面在私有暂存区
                // `{home}/inbox-staging(记录 + `.part` + 已落定未发布的
                // 暂存终件)——经 inbox_dest 的 cleanup_inbox 一并回收
                // (含 InboxDestIo 的构造面)。恒可执行,不依赖对端在线。
                let rels = rec.rels.clone();
                let cleaned = if rec.target_dir
                    == mtdss_core::transport::intake::INBOX_SENTINEL
                {
                    let home = state.home();
                    let queries = self.dest_queries().clone();
                    tokio::task::spawn_blocking(move || {
                        crate::inbox_dest::cleanup_inbox(&home, &rels, &queries)
                    })
                    .await
                    .map_err(|e| ClearErr::Other(format!("清理任务收尾失败: {e}")))?
                } else {
                    let dir = PathBuf::from(&rec.target_dir);
                    tokio::task::spawn_blocking(move || {
                        mtdss_core::transport::intake::cleanup_rels(
                            &mtdss_core::transport::intake::FsDestIo::new(&dir),
                            &rels,
                        )
                    })
                    .await
                    .map_err(|e| ClearErr::Other(format!("清理任务收尾失败: {e}")))?
                };
                tracing::debug!(job = %id, cleaned, "download 清除:本地残留已清");
                state.tasks().remove(id);
                self.evict_terminal(id);
                Ok(())
            }
            // upload/remote_copy 同臂:残留在接收端,对端清完回执 Ok 才删
            // 本地记录;离线可重试,其余对端/本地失败同样保留(仅 Ok 删)。
            "upload" | "remote_copy" => {
                match state
                    .ctrl()
                    .cleanup_outbox(state, &rec.to_dev, &rec.target_dir, &rec.rels)
                    .await
                {
                    Ok(cleaned) => {
                        tracing::debug!(job = %id, cleaned, "清除:对端残留已清");
                        state.tasks().remove(id);
                        self.evict_terminal(id);
                        Ok(())
                    }
                    Err(CtrlErr::PeerOffline(m)) => Err(ClearErr::PeerOffline(m)),
                    Err(e) => {
                        let (_, msg) = e.rpc();
                        Err(ClearErr::Other(msg))
                    }
                }
            }
            other => Err(ClearErr::Other(format!(
                "任务种类不可清除(无残留面语义): {other}"
            ))),
        }
    }

    /// 起一个 download 作业(Task 7):注册 queued 卡 → spawn 驱动(orchestrate
    /// → 轮进度经绑定路由 → job_query 周期探测源端终局)→ spawn 取消通知哨
    /// (cancelled 终态后 best-effort 通知源端停驱动重试;独立任务不受驱动
    /// abort 影响)。`old_rels` 非空 = 重发编排(v2-P2 Task 8):orchestrate
    /// 携旧 rel 集,源端 partial 预检 + dropped 由源端统一清本端残留。
    ///
    /// 校验:device 不得是本机(本机复制请用 local_copy,api 层已拒,此处
    /// 兜底);paths 非空;dir 合法(本机落盘目录,Task 1 绝对形态口径);
    /// 策略上限;须已登录(orchestrate 经 ctrl 打洞需要会话)。计数面:数
    /// 据在远端,预扫不可为——files/bytes 总数恒 0(未知),RecvProgress
    /// 逐步填 done/bytes。
    #[allow(clippy::too_many_arguments)] // 构造面吃下首发与重发编排两侧的参数面(重发侧多 old_rels 一位)
    pub(crate) fn start_download(
        &self,
        state: &Arc<DaemonState>,
        device: &str,
        paths: Vec<String>,
        dir: &str,
        policy: TransferPolicy,
        cut: bool,
        old_rels: &[String],
    ) -> Result<String, String> {
        if state.self_device_id().as_deref() == Some(device) {
            return Err("下载源设备不能是本机(本机复制请用 local_copy)".into());
        }
        if paths.is_empty() {
            return Err("paths 不能为空".into());
        }
        // v2-P3 T6 收件箱哨兵:android 端 download 的 dir 允许 INBOX_SENTINEL
        // (目的地 = 收件箱,ConnectRequest 定向哨兵,接收端 resident loop
        // 哨兵路由接 InboxDestIo——P4a 私有暂存 {home}/inbox-staging);桌面
        // (Windows/Linux host)不接受,「dir 非法」既有文案即防串台守卫
        // (契约 golden transfer.start.download-inbox 钉死该形态)。桌面
        // 判定路径零变化(inbox_dir_ok 恒 false)。
        if !crate::flow::absolute_form(dir) && !inbox_dir_ok(dir) {
            return Err(format!("dir 非法(须为绝对路径,Task 1 口径): {dir}"));
        }
        check_policy_limits(&policy)?;
        if state.session().is_none() {
            return Err("未登录,请先 login.verify".into());
        }
        let src = format!("{device}:{}", paths.join(", "));
        let meta = JobMeta {
            files_total: 0,
            bytes_total: 0,
            rel_sizes: HashMap::new(),
            // 数据在远端,预扫不可为:空集待 Task 8 job_query 回填
            rels: Vec::new(),
            name: display_name(&paths[0]),
            src,
            dst: dir.to_string(),
        };

        // id 命名空间(Task 7 修复:评审 Critical#1):`<本机 device_id>:<序号>`。
        // 下载 id 会经 orchestrate 回显进**源端**注册表,并与操作端 resident
        // loop 的 bind 键复用——裸数字 id 与源端自身作业计数同键空间:
        // ① 源端既有任意作业(一笔 legacy send 即可)即撞号回绝编排;
        // ② 两台操作端并发同名序号互相污染。嵌入操作端 id 后,回显键只可能
        // 命中「本机接受过的该操作端编排」,自增面(纯数字)永不与之相撞。
        let id = match state.self_device_id() {
            Some(dev) => format!("{}:{}", dev, self.next_id.fetch_add(1, Ordering::Relaxed)),
            None => return Err("未登录,请先 login.verify".into()),
        };
        // 取消面:驱动内不 select 取消 watch(registry.cancel 的 abort 直接
        // 杀任务,orchestrate/轮询的每个 await 点即时生效),rx 仅留通道
        // 完整性;真正的远端止步由取消通知哨承担。
        let (cancel_tx, _cancel_rx) = watch::channel(false);
        let pause = PauseGate::new();
        let core = Arc::new(JobCore::new(
            id.clone(),
            "download",
            meta,
            state.events(),
            self.count.clone(),
            Some(state.tasks()),
        ));
        {
            let m = lock_ok(&core.m);
            core.emit_locked(&m);
        }
        // 建卡即落盘(v2-P2 Task 4,发起端 = 本机操作端;rels 待 Task 8
        // job_query 回填,建卡为空)。
        state.tasks().upsert(&TaskRec {
            id: id.clone(),
            kind: "download".into(),
            name: core.meta.name.clone(),
            from_dev: device.to_string(),
            to_dev: state.self_device_id().unwrap_or_default(),
            src_paths: paths.clone(),
            target_dir: dir.to_string(),
            policy_skip: policy.skip.clone(),
            policy_overwrite: policy.overwrite.clone(),
            cut,
            state: "queued".into(),
            created_ms: epoch_ms(),
            ..Default::default()
        });
        // 块内 shadow 同 upload:借用形参定形 owned,块外原绑定留给
        // JobHandle 与取消哨(watchman 还要用 device)。
        let driver = {
            let state = state.clone();
            let core = core.clone();
            let pause = pause.clone();
            let src_device = device.to_string();
            let dir = dir.to_string();
            let job = DownloadJob {
                src_device,
                paths,
                dir,
                policy,
                cut,
                old_rels: old_rels.to_vec(),
            };
            tokio::spawn(supervise_driver(
                core.clone(),
                async move {
                    download_driver(state, core, job, pause).await
                },
            ))
        };
        // 取消通知哨:独立任务等本地终态,cancelled 即通知源端停驱动重试
        // (abort 杀不到;done/failed 自然退出)。哨无 JoinHandle 追踪——
        // 作业终态即自灭,生命周期有界。
        let watchman_state = state.clone();
        let watchman_id = id.clone();
        let watchman_device = device.to_string();
        tokio::spawn(async move {
            // Err = 无此作业(spawn 与注册表插入竞窗的早到情形——
            // 旧失败快照口径同样即刻返回,通知面无变化)
            let Ok(fin) = watchman_state.jobs().wait(&watchman_id).await else {
                return;
            };
            if fin.state != "cancelled" {
                return;
            }
            // best-effort:连接失败静默——源端驱动反正因收端断链失败,重试
            // 预算耗尽自然终态;cancel_job 只是让重试更早止步
            if let Err(e) = watchman_state
                .ctrl()
                .cancel_job(&watchman_state, &watchman_device, &watchman_id)
                .await
            {
                tracing::debug!(job = %watchman_id, detail = %e.rpc().1, "通知源端取消失败(忽略)")
            }
        });
        self.insert_job(
            id.clone(),
            JobHandle {
                core,
                pause,
                cancel_tx,
                task: Mutex::new(Some(driver)),
                round: Mutex::new(None),
            },
        );
        Ok(id)
    }

    /// 起一个收件箱 download 重发的翻新作业(P4a T2,reinitiate 专用入口,
    /// spec §6.3):**同一新 job** 同承两相——republish 相(对 staged 集直
    /// 接驱动发布协调器,免登录/免对端)与网络相(缺失集走既有 download
    /// 编排分支,[`download_network_phase`])。与 [`Jobs::start_download`]
    /// 的差异面:
    /// - **dir 沿记录原值(收件箱哨兵)不重验**:首发 start 已按平台验证过
    ///   (桌面 transfer.start 的哨兵拒收面由契约 golden
    ///   `transfer.start.download-inbox` 钉死,不受影响)——重放持久记录
    ///   信任原判;host e2e 由此可跑通 download 驱动的发布门合成(T1 拓扑
    ///   注的空窗由本入口补上);
    /// - **登录门按相收紧**:网络相须已登录(orchestrate 经 ctrl 打洞需要
    ///   会话;同步拒,记录回插由 reinitiate 收口);republish-only 纯本
    ///   地无此门(spec §6.3 免登录);
    /// - **记录 rels 原样携带**(分区语义,非 P2 的建卡空集):发布再失败
    ///   的重发记录可再分区,不退化为无 rel 集的空面;
    /// - **取消通知哨仅网络相装配**:纯本地无对端可通知(cancel_job 反而
    ///   违反免对端语义)。
    ///
    /// 校验:策略上限(防御性,首发已验);id 命名空间沿 download 口径
    /// (`<本机 device_id>:<序号>`;免登录态回退记录 to_dev——download 记录
    /// 的目标端即本机)。
    fn start_download_republish(
        &self,
        state: &Arc<DaemonState>,
        rec: &TaskRec,
        staged: Vec<crate::inbox_dest::PublishItem>,
        republish_only: bool,
    ) -> Result<String, String> {
        if !republish_only && state.session().is_none() {
            return Err("未登录,请先 login.verify".into());
        }
        let policy = TransferPolicy {
            skip: rec.policy_skip.clone(),
            overwrite: rec.policy_overwrite.clone(),
        };
        check_policy_limits(&policy)?;
        // id 命名空间:登录态取本机 device_id;免登录的纯本地重发布回退记录
        // to_dev(download 记录的目标端 = 首发时的本机 id,同 home 同身份)。
        let me = state
            .self_device_id()
            .unwrap_or_else(|| rec.to_dev.clone());
        let id = format!("{}:{}", me, self.next_id.fetch_add(1, Ordering::Relaxed));
        let src = format!("{}:{}", rec.from_dev, rec.src_paths.join(", "));
        let meta = JobMeta {
            // 计数面同 download 口径:数据/发布集无预扫面,RecvProgress 与
            // 发布协调器不产 Files/Bytes 事件——files/bytes 恒 0(未知)。
            files_total: 0,
            bytes_total: 0,
            rel_sizes: HashMap::new(),
            rels: Vec::new(),
            name: display_name(
                rec.src_paths
                    .first()
                    .map(String::as_str)
                    .unwrap_or_default(),
            ),
            src,
            dst: rec.target_dir.clone(),
        };
        let (cancel_tx, _cancel_rx) = watch::channel(false);
        let pause = PauseGate::new();
        let core = Arc::new(JobCore::new(
            id.clone(),
            "download",
            meta,
            state.events(),
            self.count.clone(),
            Some(state.tasks()),
        ));
        {
            let m = lock_ok(&core.m);
            core.emit_locked(&m);
        }
        // 建卡即落盘(v2-P2 Task 4 口径):rels 携带分区语义全集(见函数
        // 文档——后续重发可再分区;网络相的 job_query 回填非空时覆盖为源
        // 端新全集,等价或更新)。
        state.tasks().upsert(&TaskRec {
            id: id.clone(),
            kind: "download".into(),
            name: core.meta.name.clone(),
            from_dev: rec.from_dev.clone(),
            to_dev: me,
            src_paths: rec.src_paths.clone(),
            target_dir: rec.target_dir.clone(),
            policy_skip: policy.skip.clone(),
            policy_overwrite: policy.overwrite.clone(),
            cut: rec.cut,
            rels: rec.rels.clone(),
            state: "queued".into(),
            created_ms: epoch_ms(),
            ..Default::default()
        });
        let driver = {
            let state = state.clone();
            let core = core.clone();
            let pause = pause.clone();
            let job = InboxRepubJob {
                staged,
                republish_only,
                net: DownloadJob {
                    src_device: rec.from_dev.clone(),
                    paths: rec.src_paths.clone(),
                    dir: rec.target_dir.clone(),
                    policy,
                    cut: rec.cut,
                    // 网络相编排携旧 rel 全集(与既有 download 重发同口径:
                    // 源端 partial 预检 + dropped 统一清本端残留)。
                    old_rels: rec.rels.clone(),
                },
            };
            tokio::spawn(supervise_driver(core.clone(), async move {
                inbox_republish_driver(state, core, job, pause).await
            }))
        };
        // 取消通知哨仅网络相(纯本地无对端面;哨语义同 start_download)。
        if !republish_only {
            let watchman_state = state.clone();
            let watchman_id = id.clone();
            let watchman_device = rec.from_dev.clone();
            tokio::spawn(async move {
                let Ok(fin) = watchman_state.jobs().wait(&watchman_id).await else {
                    return;
                };
                if fin.state != "cancelled" {
                    return;
                }
                if let Err(e) = watchman_state
                    .ctrl()
                    .cancel_job(&watchman_state, &watchman_device, &watchman_id)
                    .await
                {
                    tracing::debug!(job = %watchman_id, detail = %e.rpc().1, "通知源端取消失败(忽略)")
                }
            });
        }
        self.insert_job(
            id.clone(),
            JobHandle {
                core,
                pause,
                cancel_tx,
                task: Mutex::new(Some(driver)),
                round: Mutex::new(None),
            },
        );
        Ok(id)
    }

    /// 起一个 remote_copy 作业(Task 8 A→B 编排):操作端 X 让 **from 设备**
    /// 把 paths 直传给 to 设备的 dir(数据不经 X 落地)。预扫(from 端逐源
    /// stat × to 端目标目录 list)在驱动内;编排/cancel_job/job_query/cut
    /// 收尾的 ctrl 对端都是 from。终局:本端卡片 1.5s 轮询 from 端
    /// job_query 即时映射(done 无宽限窗,见模块文档);done 且 cut → 对
    /// from 端 ctrl delete 源路径(回收优先,尽力而为)。
    ///
    /// 校验:from/to 都不得是本机(from=本机属 upload 面、to=本机属
    /// download 面,各有作业型,此处显式指路);from == to 拒(同机复制走
    /// ctrl copy 面,编排自发自收只会困死在自拨守卫上);paths 非空;dir
    /// 合法(to 设备视角绝对形态);策略上限;须已登录(id 命名空间要用)。
    /// 计数面:数据在远端两端之间,预扫不可为——files/bytes 总数恒 0(未
    /// 知)。id 命名空间沿 Task 7 修复口径(`<本机 device_id>:<序号>`,评审
    /// Critical#1):回显进 from 端注册表后,与 from 端自增面/其他操作端
    /// 永不撞号。`old_rels` 非空 = 重发编排(v2-P2 Task 8):orchestrate 携
    /// 旧 rel 集,from 端 partial 预检 + dropped 由 from 端统一清 to 端
    /// 残留(spec §5.1)。
    #[allow(clippy::too_many_arguments)] // 编排面参数齐平 IPC 契约形态(from/paths/to/dir/policy/cut)+ 重发侧 old_rels 一位
    pub(crate) fn start_remote_copy(
        &self,
        state: &Arc<DaemonState>,
        from: &str,
        paths: Vec<String>,
        to: &str,
        dir: &str,
        policy: TransferPolicy,
        cut: bool,
        old_rels: &[String],
    ) -> Result<String, String> {
        let me = state
            .self_device_id()
            .ok_or_else(|| "未登录,请先 login.verify".to_string())?;
        if from == me {
            return Err("源设备不能是本机(本机→设备请用 upload)".into());
        }
        if to == me {
            return Err("目标设备不能是本机(设备→本机请用 download)".into());
        }
        if from == to {
            return Err("源与目标不得是同一台设备(同机复制请用 copy 面)".into());
        }
        if paths.is_empty() {
            return Err("paths 不能为空".into());
        }
        // dir 属于目标设备(to),平台未知——平台无关绝对形态预检(见
        // start_upload_internal 同注)。
        if !crate::flow::remote_absolute_form(dir) {
            return Err(format!("dir 非法(须为绝对路径,Windows 或 Unix 形态): {dir}"));
        }
        check_policy_limits(&policy)?;
        let src = format!("{from}:{}", paths.join(", "));
        let meta = JobMeta {
            files_total: 0,
            bytes_total: 0,
            rel_sizes: HashMap::new(),
            // 数据在远端两端之间,预扫不可为:空集待 Task 8 job_query 回填
            rels: Vec::new(),
            name: display_name(&paths[0]),
            src,
            dst: format!("{to}:{dir}"),
        };

        let id = format!("{}:{}", me, self.next_id.fetch_add(1, Ordering::Relaxed));
        // 取消面:驱动内不 select 取消 watch(registry.cancel 的 abort 直接
        // 杀任务,预扫/orchestrate/轮询的每个 await 点即时生效);远端止步
        // 由取消通知哨承担。
        let (cancel_tx, _cancel_rx) = watch::channel(false);
        let pause = PauseGate::new();
        let core = Arc::new(JobCore::new(
            id.clone(),
            "remote_copy",
            meta,
            state.events(),
            self.count.clone(),
            Some(state.tasks()),
        ));
        {
            let m = lock_ok(&core.m);
            core.emit_locked(&m);
        }
        // 建卡即落盘(v2-P2 Task 4,发起端 = 本机操作端 X;rels 待 Task 8
        // job_query 回填,建卡为空)。
        state.tasks().upsert(&TaskRec {
            id: id.clone(),
            kind: "remote_copy".into(),
            name: core.meta.name.clone(),
            from_dev: from.to_string(),
            to_dev: to.to_string(),
            src_paths: paths.clone(),
            target_dir: dir.to_string(),
            policy_skip: policy.skip.clone(),
            policy_overwrite: policy.overwrite.clone(),
            cut,
            state: "queued".into(),
            created_ms: epoch_ms(),
            ..Default::default()
        });
        // 块内 shadow 同 upload(块外 from 还要给取消哨)。
        let driver = {
            let state = state.clone();
            let core = core.clone();
            let pause = pause.clone();
            let from_dev = from.to_string();
            let to_dev = to.to_string();
            let dir = dir.to_string();
            let job = RemoteCopyJob {
                from: from_dev,
                paths,
                to: to_dev,
                dir,
                policy,
                cut,
                old_rels: old_rels.to_vec(),
            };
            tokio::spawn(supervise_driver(
                core.clone(),
                async move {
                    remote_copy_driver(state, core, job, pause).await
                },
            ))
        };
        // 取消通知哨(同 download):本地 cancelled 终态后 best-effort 通知
        // from 端停驱动重试(abort 杀不到;done/failed 自然退出,生命周期
        // 有界)。连接失败静默——from 端驱动因操作端不再轮询也不影响数据面,
        // cancel_job 只是让它更早止步。
        let watchman_state = state.clone();
        let watchman_id = id.clone();
        let watchman_from = from.to_string();
        tokio::spawn(async move {
            // Err 口径同 download 哨(spawn/插入竞窗早到即刻返回)
            let Ok(fin) = watchman_state.jobs().wait(&watchman_id).await else {
                return;
            };
            if fin.state != "cancelled" {
                return;
            }
            if let Err(e) = watchman_state
                .ctrl()
                .cancel_job(&watchman_state, &watchman_from, &watchman_id)
                .await
            {
                tracing::debug!(job = %watchman_id, detail = %e.rpc().1, "通知源端取消失败(忽略)")
            }
        });
        self.insert_job(
            id.clone(),
            JobHandle {
                core,
                pause,
                cancel_tx,
                task: Mutex::new(Some(driver)),
                round: Mutex::new(None),
            },
        );
        Ok(id)
    }

    /// 起一个 local_copy 作业(Task 7;M4b 移交债①收策略):本机复制/移动
    /// (spawn_blocking 跑 core fs 策略版 `copy_paths_with`/`move_entries_
    /// with`,与传输面同一匹配口径)。纯本地操作**不要求登录**;校验:
    /// paths 非空、dir 存在且为目录、dir 合法(绝对形态)、策略上限(与
    /// upload/download/remote_copy 同一收口——api 层 policy_from_params 已
    /// 校验,此处兜底保持「语义校验在 jobs 注册表收口」纪律)、预扫非空
    /// (同步反馈面)。
    pub(crate) async fn start_local_copy(
        &self,
        state: &Arc<DaemonState>,
        paths: Vec<PathBuf>,
        dir: &str,
        cut: bool,
        policy: TransferPolicy,
    ) -> Result<String, String> {
        if paths.is_empty() {
            return Err("paths 不能为空".into());
        }
        if !crate::flow::absolute_form(dir) {
            return Err(format!("dir 非法(须为绝对路径,Task 1 口径): {dir}"));
        }
        check_policy_limits(&policy)?;
        // 目录存在性检查 + 预扫:全树 rel(只读元数据)——files/bytes 总数
        // 与完成补齐表(都是阻塞调用,落线程池,直跑占死 async worker;
        // 目录缺失的业务文案原样保留)
        let scan_paths = paths.clone();
        let scan_dir = dir.to_string();
        let rels = tokio::task::spawn_blocking(move || -> Result<Vec<(String, bool, u64)>, String> {
            if !std::path::Path::new(&scan_dir).is_dir() {
                return Err(format!("目标目录不存在或不是目录: {scan_dir}"));
            }
            Ok(crate::ops::local_tree_rels(&scan_paths))
        })
        .await
        .map_err(|e| format!("扫描任务失败: {e}"))??;
        let mut rel_sizes = HashMap::new();
        let mut files_total = 0u64;
        let mut bytes_total = 0u64;
        for (rel, is_dir, size) in &rels {
            if !is_dir {
                files_total += 1;
                bytes_total += *size;
                rel_sizes.insert(rel.clone(), *size);
            }
        }
        if files_total == 0 {
            return Err("无可复制内容(输入缺失、空目录或全为符号链接)".into());
        }
        let src = paths
            .iter()
            .map(|p| p.to_string_lossy().into_owned())
            .collect::<Vec<_>>()
            .join(", ");
        let meta = JobMeta {
            files_total,
            bytes_total,
            rel_sizes,
            rels: rels.iter().map(|(rel, _, _)| rel.clone()).collect(),
            name: display_name(&paths[0].to_string_lossy()),
            src,
            dst: dir.to_string(),
        };

        let id = self.next_id.fetch_add(1, Ordering::Relaxed).to_string();
        let (cancel_tx, _cancel_rx) = watch::channel(false);
        let pause = PauseGate::new();
        // store=None:本机复制无远端面,不入持久层(spec §2,会话级)。
        let core = Arc::new(JobCore::new(
            id.clone(),
            "local_copy",
            meta,
            state.events(),
            self.count.clone(),
            None,
        ));
        {
            let m = lock_ok(&core.m);
            core.emit_locked(&m);
        }
        // 块内 shadow 同 upload(dir 为借用形参,先定形 owned)。
        let driver = {
            let core = core.clone();
            let pause = pause.clone();
            let dir = dir.to_string();
            tokio::spawn(supervise_driver(
                core.clone(),
                async move {
                    local_copy_driver(
                        core,
                        LocalCopyJob {
                            paths,
                            dir,
                            cut,
                            policy,
                        },
                        pause,
                    )
                    .await
                },
            ))
        };
        self.insert_job(
            id.clone(),
            JobHandle {
                core,
                pause,
                cancel_tx,
                task: Mutex::new(Some(driver)),
                round: Mutex::new(None),
            },
        );
        Ok(id)
    }

    /// 注册匿名 incoming 卡(Task 7,常驻接收循环调用):ConnectRequest 不
    /// 带 job_id(legacy send / 对端无任务面)→ 建 kind="incoming" 卡并绑定
    /// 轮,让每笔传输都有可见卡片。计数面同 download(远端数据,总数未知,
    /// RecvProgress 逐步填)。
    pub(crate) fn register_incoming(
        &self,
        state: &Arc<DaemonState>,
        from: &str,
        dir: &Path,
    ) -> String {
        let meta = JobMeta {
            files_total: 0,
            bytes_total: 0,
            rel_sizes: HashMap::new(),
            rels: Vec::new(),
            // 被动收件无预扫面:显示名取来源设备(UI 方向列另见 src/dst)
            name: from.to_string(),
            src: from.to_string(),
            dst: dir.to_string_lossy().into_owned(),
        };
        let id = self.next_id.fetch_add(1, Ordering::Relaxed).to_string();
        let (cancel_tx, _cancel_rx) = watch::channel(false);
        let pause = PauseGate::new();
        // store=None:被动收件不入持久层(spec §2,会话级)。
        let core = Arc::new(JobCore::new(
            id.clone(),
            "incoming",
            meta,
            state.events(),
            self.count.clone(),
            None,
        ));
        {
            let m = lock_ok(&core.m);
            core.emit_locked(&m);
        }
        // 无驱动任务:轮的收口(resident loop)即作业收口;cancel/暂停经
        // round 槽位直达该轮。
        self.insert_job(
            id.clone(),
            JobHandle {
                core,
                pause,
                cancel_tx,
                task: Mutex::new(None),
                round: Mutex::new(None),
            },
        );
        id
    }

    /// 接收进度路由(Task 7):RecvProgress → 绑定作业的聚合通道。作业不
    /// 存在(迟到/防御)静默忽略——进度回调只做毫秒级账,不报错。
    pub(crate) fn track_recv(&self, job_id: &str, p: &RecvProgress<'_>) {
        let core = lock_ok(&self.map).get(job_id).map(|h| h.core.clone());
        if let Some(core) = core {
            core.track_recv(p);
        }
    }

    /// 轮终局收口(Task 7,常驻接收循环调用):**匿名来件卡**的 done/failed
    /// 经 transition 落定(终态粘滞使与 cancel 的竞速无害)。**下载绑定轮
    /// 的 Ok 不走此面**(Task 8):混合后果(源端 partial 预检 missing)的
    /// 终因只在源端作业态里,先落 done 会被终态粘滞吞掉 failed 映射——
    /// 调用方改走 [`Jobs::round_complete`],failed 臂(轮失败保持 running
    /// 等源端重试)不变。作业不存在 → false(调用方忽略)。
    pub(crate) fn finish_incoming(&self, job_id: &str, state: &str, error: Option<String>) -> bool {
        let core = lock_ok(&self.map).get(job_id).map(|h| h.core.clone());
        match core {
            Some(core) => {
                core.finish(state, error);
                true
            }
            None => false,
        }
    }

    /// 下载绑定轮的 Ok 落定(Task 8,常驻接收循环调用):只记「本轮已完整
    /// 落定」事实,不改可见状态;终局由 download 驱动轮询源端作业态合流
    /// 收口(源端 done + 轮落定 → done;源端 failed → failed 携源端终因,
    /// spec §5.1 混合后果)。作业不存在/已终态(取消竞速)静默忽略。
    /// **收件箱轮不走此面**(P4a):落定路径非空时改走 [`Jobs::begin_publish`]
    /// (轮落定 + 发布开窗原子化,协调器驱动终态)。
    pub(crate) fn round_complete(&self, job_id: &str) {
        let core = lock_ok(&self.map).get(job_id).map(|h| h.core.clone());
        if let Some(core) = core {
            core.mark_round_ok();
        }
    }

    /// 收件箱轮 Ok 落定的**发布接管**(P4a,常驻接收循环调用;spec §6.1):
    /// 落定路径 → 待发布集(`inbox_dest::publish_backlog`:记录门 done 未
    /// published,final_rel 命中落定集)。非空即原子开窗
    /// ([`JobCore::mark_round_publishing`]:round_ok + 发布 Pending 同锁 +
    /// 翻可见态 publishing/保存中)并 spawn 发布协调器
    /// ([`publish_coordinator`]);空(全 Skip/记录缺席的退化面)沿旧收口
    /// ——绑定 download 卡记 round_ok(终局由驱动合成),匿名卡直接 done。
    /// 作业不存在/已终态(取消竞速)静默忽略:不发布,暂存保留(记录
    /// done 未 published,重发走 republish 分支,§6.3)。
    pub(crate) async fn begin_publish(
        &self,
        state: &Arc<DaemonState>,
        job_id: &str,
        landed: &[PathBuf],
    ) {
        let Some(core) = lock_ok(&self.map).get(job_id).map(|h| h.core.clone()) else {
            return;
        };
        let home = state.home();
        let scan_home = home.clone();
        let scan_landed = landed.to_vec();
        let items = match tokio::task::spawn_blocking(move || {
            crate::inbox_dest::publish_backlog(&scan_home, &scan_landed)
        })
        .await
        {
            Ok(v) => v,
            Err(e) => {
                tracing::warn!(%e, job = %job_id, "发布集扫描任务收尾失败,退化旧收口");
                Vec::new()
            }
        };
        if items.is_empty() {
            if core.kind == "download" {
                core.mark_round_ok();
            } else {
                core.finish("done", None);
            }
            return;
        }
        if !core.mark_round_publishing() {
            return; // 取消竞速先到:不开发布窗(终态粘滞)
        }
        let coord_state = state.clone();
        tokio::spawn(supervise_driver(
            core.clone(),
            async move { publish_coordinator(coord_state, core, items).await },
        ));
    }

    /// 中继兜底标记(v2-P1c T5):兜底生效即调恰一次——发送侧经 upload
    /// 驱动的 on_relay 回调、接收侧经常驻接收轮的轮内调用。作业不存在
    /// (迟到/防御)静默忽略——标记只做毫秒级置位,不报错。
    pub(crate) fn mark_relay(&self, id: &str) {
        let core = lock_ok(&self.map).get(id).map(|h| h.core.clone());
        if let Some(core) = core {
            core.mark_relay();
        }
    }

    /// 全量快照(transfer.list 数据源),按序号升序:排序键见 [`job_seq`]。
    pub(crate) fn snapshot(&self) -> Vec<JobSnap> {
        let map = lock_ok(&self.map);
        let mut out: Vec<(u64, JobSnap)> = map
            .iter()
            .map(|(id, h)| (job_seq(id), h.core.snap()))
            .collect();
        out.sort_by_key(|(n, _)| *n);
        out.into_iter().map(|(_, s)| s).collect()
    }

    /// 作业的预扫 rel 全集(v2-P2 Task 8 job_query 回填面):upload(含编
    /// 排回显)的 meta.rels = 建卡时预扫全集,随 JobQueryResponse.rels 回
    /// 给操作端写回 store;download/remote_copy 的 meta.rels 恒空(操作端
    /// 另有轮询回写 store 面,见 [`JobCore::backfill_rels`])。id 缺失 →
    /// 空集。meta 构造后不可变,只持注册表锁读。
    pub(crate) fn job_rels(&self, id: &str) -> Vec<String> {
        lock_ok(&self.map)
            .get(id)
            .map(|h| h.core.meta.rels.clone())
            .unwrap_or_default()
    }

    /// 暂停/恢复(false = resume):置任务暂停门 + 可见状态翻转;直达绑定的
    /// 接收轮门(Task 7 download 侧)。缺失或已终态 → false(不可暂停)。
    pub(crate) fn set_paused(&self, id: &str, paused: bool) -> bool {
        let core = {
            let map = lock_ok(&self.map);
            let Some(h) = map.get(id) else {
                return false;
            };
            if h.core.is_terminal() {
                return false;
            }
            // remote_copy 无本机数据面(暂停门没有消费方):放行只会翻转可见
            // 状态而 A→B 直传照跑——「状态 paused 实际在传」的假象比拒绝更
            // 糟,按不可暂停拒(false 面,与缺失/终态同一反馈;RPC 文案合并
            // 是接受的粗粒度)。
            if h.core.kind == "remote_copy" {
                return false;
            }
            h.pause.set(paused);
            if let Some(r) = lock_ok(&h.round).as_ref() {
                r.pause.set(paused); // 接收轮同门:双侧一起挂起/放开
            }
            h.core.clone()
        };
        core.transition_paused(paused);
        true
    }

    /// 取消:置 watch + 终态落定 cancelled + abort 驱动任务(语义见模块
    /// 文档;绑定接收轮若有,取消一并直达)。缺失或已终态 → false。
    pub(crate) fn cancel(&self, id: &str) -> bool {
        // 延迟初始化 + 块内赋值:JoinHandle 是 owned 值,块尾临时守卫不外泄
        let task: Option<JoinHandle<()>>;
        {
            let map = lock_ok(&self.map);
            let Some(h) = map.get(id) else {
                return false;
            };
            if h.core.is_terminal() {
                return false;
            }
            // 同一临界区内完成观察面三连:流内取消 watch、终态落定+事件、
            // 接收轮取消直达——abort 放到锁外(杀任务是纯同步快操作,但
            // 保持锁内零副作用原则)
            let _ = h.cancel_tx.send(true);
            if let Some(r) = lock_ok(&h.round).as_ref() {
                let _ = r.cancel.send(true);
            }
            h.core.transition("cancelled", None);
            task = lock_ok(&h.task).take();
        }
        if let Some(task) = task {
            task.abort(); // 连 run_send_with 重试循环一并杀,不复活
        }
        true
    }

    /// 终态长轮询(notify 唤醒)。不存在 id → 立即 `Err`(报文
    /// 「作业不存在: <id>」),绝不长轮询(M4a-1 的防御面,由「合成
    /// failed 快照」改为显式报错;内存面常态无驱逐,不存在 = id 打错或行
    /// 已被清除/翻新逐出,见 [`Jobs::evict_terminal`])。
    /// 已入轮的 waiter 手持 JobCore Arc,作业面随后无论如何变化都能等到
    /// 其终态。
    pub(crate) async fn wait(&self, id: &str) -> Result<JobSnap, String> {
        let Some(core) = lock_ok(&self.map).get(id).map(|h| h.core.clone()) else {
            return Err(format!("作业不存在: {id}"));
        };
        loop {
            // enable 先于查终态:Notified 的登记序闭合「翻转与登记竞速」的
            // 漏唤醒窗口(见 tokio Notify 文档的循环用法)
            let notified = core.notify.notified();
            tokio::pin!(notified);
            let _ = notified.as_mut().enable();
            let snap = core.snap();
            if is_terminal_state(&snap.state) {
                return Ok(snap);
            }
            notified.await;
        }
    }

    /// resident 接收轮绑定(轮自卡接线;Task 7 接线 + 终审修复三分支):
    /// job_id 存在**且为可绑定的来件作业**(download = 操作端预注册的
    /// 下载卡;incoming = 常驻循环刚注册的匿名卡——**第二轮**自卡接线
    /// 即经本入口)**且非终态**即占槽。重复绑定后写者胜(下载轮代际更替
    /// 重绑的自然覆盖)。ConnectRequest 的**首绑**不走本入口(裸数字
    /// job_id 与匿名卡撞号面,见 [`bind_connect`])。
    ///
    /// 绑定纪律(终审 Important#1):
    /// - **暂停种子**:绑定即把作业级暂停门现值抄进轮门。暂停可能落在轮
    ///   装配前(orchestrate/打洞/重试间隙——轮尚未绑定,作业级门无消费
    ///   方,下载驱动只读它一次 `begin_running`);不抄即静默丢失:新轮
    ///   全新门 = 全速传输 + 卡片 paused、bytes_done 可见爬升的假态。
    /// - **终态拒绝**:已终态(典型 = 取消后仍在途的 ConnectRequest)不
    ///   占槽,并驱动该轮取消 watch 让轮死亡——已取消下载的数据不得落地;
    ///   返回 [`BindOutcome::RefusedTerminal`] 让调用方弃轮(不得降级匿名
    ///   卡,降级数据照样落地)。
    pub(crate) fn bind_incoming(&self, job_id: &str, round: RoundHandle) -> BindOutcome {
        let map = lock_ok(&self.map);
        match map.get(job_id) {
            Some(h) if matches!(h.core.kind, "download" | "incoming") => {
                if h.core.is_terminal() {
                    // 取消/完成/失败后的迟到绑定:驱动轮取消 watch(消费方
                    // 在传输 select 处),数据不落地
                    let _ = round.cancel.send(true);
                    return BindOutcome::RefusedTerminal;
                }
                if h.pause.paused() {
                    round.pause.set(true); // 暂停种子:作业级门 → 轮门
                }
                *lock_ok(&h.round) = Some(round);
                BindOutcome::Bound
            }
            _ => BindOutcome::Miss,
        }
    }

    /// ConnectRequest 的**首绑**(v2-P3 T6 与 [`bind_incoming`] 拆分):
    /// 仅 **download** 卡是绑定目标(操作端预注册的下载卡——进度/取消
    /// 经它路由)。**incoming 卡命中一律 [`BindOutcome::Miss`]**:
    /// ConnectRequest 携带的 job_id 对 upload 传输是**发送端自身**的
    /// 作业 id(裸数字),与本端匿名卡的裸数字自增面天然同键空间——
    /// 首绑若沿用全种类匹配,upload 重试轮会撞上一张已终态匿名卡被整体
    /// 拒收(RefusedTerminal),单轮网络失败后该 upload 永远无法重试
    /// 落地(收件箱 e2e 的重试轮实测踩中;incoming 命中 = id 撞号,
    /// 新传输与旧卡无关,按未命中走新匿名卡)。download 命中语义与
    /// [`bind_incoming`] 完全一致:终态拒收并驱动轮取消、暂停种子、占槽。
    pub(crate) fn bind_connect(&self, job_id: &str, round: RoundHandle) -> BindOutcome {
        let map = lock_ok(&self.map);
        match map.get(job_id) {
            Some(h) if h.core.kind == "download" => {
                if h.core.is_terminal() {
                    let _ = round.cancel.send(true);
                    return BindOutcome::RefusedTerminal;
                }
                if h.pause.paused() {
                    round.pause.set(true); // 暂停种子:作业级门 → 轮门
                }
                *lock_ok(&h.round) = Some(round);
                BindOutcome::Bound
            }
            _ => BindOutcome::Miss,
        }
    }
}

// ---------- bridge 源解析(fs.request_source,v2-P3 T4) ----------

/// bridge 上行的源 fd 解析器(`fs.request_source` 的 daemon 侧执行体;
/// 收件侧查询/发布面(DestQueries,0x04/0x05)沿本模式,但无 fd)。
///
/// 协议(懒解析,逐文件):transfer.start(bridge) 只携带条目元数据
/// {ref,rel,size,mtime};驱动起步对每个条目广播 `fs.request_source
/// {"request_id":u64,"ref":str}` 并登记 oneshot,App 经 fdpass
/// `0x01 source_fd` 回 `{request_id}` + 1 个源 fd(resolve 落定);
/// **无 fd 的 0x01 = App 拒开**(授权被撤等,payload 可带 error 文案,
/// 立即失败不等超时)。等待上限 10s([`REQUEST_SOURCE_TIMEOUT`],测试
/// 注入缝 [`request_source_timeout`]);超时/拒开/通道异常一律
/// 「源不可用」语义上抛;超时请求即从 pending 摘除,迟到的 fd 应答按
/// 未知 request_id 拒(ack ok:false,fd 就地 Drop 关闭,不积陈项)。
///
/// fd 生命周期(评审焦点 #1,懒逐文件):fd 在驱动建表时**逐文件**请求
/// (串行,逐个 request_source,下一件的请求只在上一件 fd 落地后发出);
/// 建表后整表存活至该轮 hash+send 收尾——清单构建与块流重开各自
/// try_clone 出本段副本,原 fd 归驱动源表(重试轮可能重读任意未完成
/// 文件,表元素不可提前释放);表随驱动任务收尾(含失败/取消 abort)
/// Drop,fd 全释放,无逐文件泄漏面。峰值句柄数 = 条目数,T9/T11 实机
/// 大目录验证点。
#[cfg(unix)]
pub(crate) struct SourceResolver {
    /// 事件广播出口(fs.request_source 与作业事件同一 channel,所有已连
    /// IPC 客户端可见——Kotlin App 的订阅面)。
    events: tokio::sync::broadcast::Sender<EventFrame>,
    /// request_id → 等待方(oneshot);超时/交付/拒开即摘除。取消路径
    /// (驱动任务 abort 杀在 resolve 等待上)的残项由迟到的 fd/拒开应答
    /// 经 remove 兜底清理(此时 rx 已随任务死,send 失败 → ack ok:false);
    /// 每次取消至多残留一条,无逐项增长面。
    pending: Mutex<HashMap<u64, oneshot::Sender<Result<SourceFile, String>>>>,
    /// request_id 单调源(跨作业全局:多作业并发时不复用,App 侧免路由)。
    next_request_id: AtomicU64,
    /// 应答等待上限(默认 10s;测试注入小值)。
    timeout: Duration,
}

#[cfg(unix)]
impl SourceResolver {
    pub(crate) fn new(events: tokio::sync::broadcast::Sender<EventFrame>) -> Self {
        Self {
            events,
            pending: Mutex::new(HashMap::new()),
            next_request_id: AtomicU64::new(0),
            timeout: request_source_timeout(),
        }
    }

    /// 逐文件解析:登记 oneshot → 广播请求 → 有界等待。Err 恒为完整
    /// 「源不可用: …」文案(超时/拒开/通道异常),驱动原样落 failed。
    /// 参数名 ref_id(非 `r#ref`:格式串不认原始标识符,见各 format!)
    pub(crate) async fn resolve(&self, ref_id: &str) -> Result<SourceFile, String> {
        let (tx, rx) = oneshot::channel();
        let request_id = self.next_request_id.fetch_add(1, Ordering::Relaxed);
        // 登记先于广播:事件落地即必有等待方,无丢失窗口
        lock_ok(&self.pending).insert(request_id, tx);
        let _ = self.events.send(EventFrame {
            method: EV_REQUEST_SOURCE.into(),
            params: json!({ "request_id": request_id, "ref": ref_id }),
        });
        let outcome = tokio::time::timeout(self.timeout, rx).await;
        // 先摘 pending(超时/异常路径):迟到的 fd/拒开按未知 request_id
        // 拒,pending 表不积陈项(重复 request_id 由单调源杜绝)
        lock_ok(&self.pending).remove(&request_id);
        match outcome {
            // 交付(SourceFile::Fd)或拒开(Err 带 App 文案)
            Ok(Ok(Ok(source))) => Ok(source),
            Ok(Ok(Err(reason))) => Err(format!("源不可用: {reason}(ref={ref_id})")),
            // 等待方通道异常关闭(理论不可达:sender 恒先 send 或随 remove Drop)
            Ok(Err(_)) => Err(format!("源不可用: 应答通道异常关闭(ref={ref_id})")),
            Err(_) => Err(format!(
                "源不可用: 等待 App 应答超时({:?})(ref={ref_id})",
                self.timeout
            )),
        }
    }

    /// fdpass 0x01(+fd)落点:交付给等待方。false = 无此等待方(已超时/
    /// 已交付/跨会话残留),调用方回 ack ok:false,fd 就地 Drop 关闭。
    pub(crate) fn deliver(&self, request_id: u64, fd: OwnedFd) -> bool {
        match lock_ok(&self.pending).remove(&request_id) {
            Some(tx) => tx.send(Ok(SourceFile::Fd(fd))).is_ok(),
            None => false,
        }
    }

    /// fdpass 0x01(无 fd = App 拒开)落点:失败等待方(立即,不等超时)。
    /// false = 无此等待方(调用方回 ack ok:false)。
    pub(crate) fn refuse(&self, request_id: u64, reason: &str) -> bool {
        match lock_ok(&self.pending).remove(&request_id) {
            Some(tx) => tx.send(Err(reason.to_string())).is_ok(),
            None => false,
        }
    }
}

/// Windows 占位:`std::os::fd` 在 Windows 编译单元缺席,fdpass 通道
/// (Unix 域套接字)随之整体不存在——bridge 形态的 resolve 一律
/// 「源不可用」立即收口。IPC 契约面(golden)双平台逐字节一致:
/// transfer.start(bridge) 照常受理建卡(校验/计数同为声明值面),驱动
/// 起步即失败,与「无 App 应答超时」同语义、仅更快。
#[cfg(not(unix))]
pub(crate) struct SourceResolver;

#[cfg(not(unix))]
impl SourceResolver {
    pub(crate) fn new(events: tokio::sync::broadcast::Sender<EventFrame>) -> Self {
        // 事件出口在 Windows 无 fd 消费面;保留参数形与 unix 实现齐平,
        // 构造面单点(DaemonState::load 一处)
        let _ = events;
        Self
    }

    pub(crate) async fn resolve(&self, ref_id: &str) -> Result<SourceFile, String> {
        Err(format!("源不可用: 本平台无 fd 通道(ref={ref_id})"))
    }
}

// ---------- 收件目的地:在场查询 + 发布协议(P4a;0x02/0x03 已退役) ----------

/// App 在场应答(fdpass 0x04):行在且 size 与请求吻合才算
/// exists(行在而 size 不符 → App 侧折算 exists=false,引擎走 FULL)。
#[derive(Debug, Clone, Copy)]
pub(crate) struct DestPresence {
    pub exists: bool,
    pub size: u64,
}

/// 收件目的地的查询/发布解析器(`fs.query_dest` / `fs.publish` 的 daemon
/// 侧执行体;P4a 起承接 v2-P3 DestResolver 的全部幸存职责——0x02 fd 桥
/// 退役后收件侧再无 fd 交付面)。**cfg 无关**(查询与发布都不触 fd,
/// Windows host 单测与正式面同一路径)。
///
/// 协议(spec §6.5,通道实例纪律改二):0x04 与 0x05 共用**查询通道**与
/// **统一 id 计数器**——App 侧单通道单表,ack 不携 kind 的撞号串扰面
/// (F7 复审 Important A)由「单计数单通道」根治。两等待面:
/// - **在场查询**(0x04):`InboxDestIo::final_state` 广播 `fs.query_dest
///   {"request_id","rel","size"}` 并登记 oneshot,App 查 MediaStore 真实
///   行后回 `{request_id, exists, size}`。等待上限 3s
///   ([`QUERY_DEST_TIMEOUT`])。**失败方向铁律(F7,钉死不动)**:超时/
///   无应答/通道异常 → `None` = 「不在场」保守回答——对账不 Skip、引擎
///   重传。宁可重传,不可假完成。
/// - **发布**(0x05):发布协调器逐文件广播 `fs.publish
///   {"request_id","rel","staging_path","size"}`(rel = 记录 final_rel,
///   §6.4)并登记 oneshot,App 执行 MediaStore 发布后回
///   `{error?, ok, request_id}`(字母序);ok:false 即发布失败(error 可
///   带文案)。等待上限 30s 基值 + size/10MB 逐文件加算(尺寸感知,
///   [`publish_timeout_for_size`],终审 I-1;App 侧 insert+流拷贝+揭开
///   的 IO 上界;超窗「保存失败」收口,暂存保留待重试,§4)。
///   **逐文件独立 request_id、独立超时**(§6.1 多文件并发发布)。
///
/// 登记先于广播、id 单调、超时即摘(迟到应答按未知 request_id 拒,ack
/// ok:false,表不积陈项)——与 SourceResolver 同一纪律。
pub(crate) struct DestQueries {
    /// 事件广播出口(与作业事件同一 channel,所有已连 IPC 客户端可见)。
    events: tokio::sync::broadcast::Sender<EventFrame>,
    /// 在场查询等待面:request_id → 等待方(oneshot)。
    pending: Mutex<HashMap<u64, tokio::sync::oneshot::Sender<DestPresence>>>,
    /// 发布等待面:request_id → 等待方(oneshot;Ok(()) = 发布成功,
    /// Err = App 失败文案)。
    pending_publish: Mutex<HashMap<u64, tokio::sync::oneshot::Sender<Result<(), String>>>>,
    /// **统一 id 单调源**(0x04/0x05 共用,§6.5;与 request_source 的空间
    /// 仍独立——两通道实例各异,ack 不携 kind)。
    next_id: AtomicU64,
    /// 在场查询应答等待上限(默认 3s;测试注入小值)。
    query_timeout: Duration,
    /// 发布超时的测试注入整体覆盖值(None = 无注入,逐文件按
    /// [`publish_timeout_for_size`] 尺寸感知基线)。
    publish_timeout: Option<Duration>,
}

impl DestQueries {
    pub(crate) fn new(events: tokio::sync::broadcast::Sender<EventFrame>) -> Self {
        Self {
            events,
            pending: Mutex::new(HashMap::new()),
            pending_publish: Mutex::new(HashMap::new()),
            next_id: AtomicU64::new(0),
            query_timeout: query_dest_timeout(),
            publish_timeout: publish_timeout_override(),
        }
    }

    /// 在场查询:登记 oneshot → 广播事件 → 有界等待。`None` = 超时/
    /// 无应答/通道异常——**保守视为不在场**(失败方向铁律,见类型文档)。
    pub(crate) async fn query(&self, rel: &str, size: u64) -> Option<DestPresence> {
        let (tx, rx) = tokio::sync::oneshot::channel();
        let request_id = self.next_id.fetch_add(1, Ordering::Relaxed);
        // 登记先于广播:事件落地即必有等待方,无丢失窗口
        lock_ok(&self.pending).insert(request_id, tx);
        let _ = self.events.send(EventFrame {
            method: EV_QUERY_DEST.into(),
            params: json!({
                "request_id": request_id,
                "rel": rel,
                "size": size,
            }),
        });
        let outcome = tokio::time::timeout(self.query_timeout, rx).await;
        // 先摘 pending(超时/异常路径):迟到的 0x04 按未知 request_id 拒,
        // 表不积陈项(重复 id 由单调源杜绝)
        lock_ok(&self.pending).remove(&request_id);
        match outcome {
            Ok(Ok(presence)) => Some(presence),
            // 等待方通道异常关闭(理论不可达:sender 恒先 send 或随 remove Drop)
            Ok(Err(_)) | Err(_) => None,
        }
    }

    /// fdpass 0x04 落点:交付等待方。false = 无此等待方(已超时/已应答),
    /// 调用方回 ack ok:false。
    #[cfg_attr(all(not(unix), not(test)), allow(dead_code))] // 正式面经 FdBridgeHandler(unix 门);host 单测直驱
    pub(crate) fn answer(&self, request_id: u64, exists: bool, size: u64) -> bool {
        match lock_ok(&self.pending).remove(&request_id) {
            Some(tx) => tx.send(DestPresence { exists, size }).is_ok(),
            None => false,
        }
    }

    /// 单文件发布:登记 oneshot → 广播 fs.publish → 有界等待(逐文件独立
    /// request_id/独立超时,§6.1)。Err = App 明确回否(ok:false,error
    /// 文案透传)或等待超时/通道异常(文案收口供「保存失败(N 项): …」
    /// 拼装)。staging_path 为暂存终件绝对路径(App 拷贝源)。
    pub(crate) async fn publish(
        &self,
        rel: &str,
        staging_path: &Path,
        size: u64,
    ) -> Result<(), String> {
        let (tx, rx) = tokio::sync::oneshot::channel();
        let request_id = self.next_id.fetch_add(1, Ordering::Relaxed);
        // 登记先于广播:事件落地即必有等待方,无丢失窗口
        lock_ok(&self.pending_publish).insert(request_id, tx);
        let _ = self.events.send(EventFrame {
            method: EV_PUBLISH.into(),
            params: json!({
                "request_id": request_id,
                "rel": rel,
                "staging_path": staging_path.display().to_string(),
                "size": size,
            }),
        });
        // 尺寸感知超时(终审 I-1):注入覆盖值恒定;正式路径按本件 size
        // 加算(30s 基值 + 每 10MB 1s)。
        let timeout = self
            .publish_timeout
            .unwrap_or_else(|| publish_timeout_for_size(size));
        let outcome = tokio::time::timeout(timeout, rx).await;
        // 先摘 pending_publish(超时/异常路径):迟到的 0x05 按未知
        // request_id 拒,表不积陈项
        lock_ok(&self.pending_publish).remove(&request_id);
        match outcome {
            Ok(Ok(r)) => r,
            // 等待方通道异常关闭(理论不可达:sender 恒先 send 或随 remove Drop)
            Ok(Err(_)) => Err("应答通道异常关闭".to_string()),
            Err(_) => Err(format!("等待 App 发布应答超时({timeout:?})")),
        }
    }

    /// fdpass 0x05 落点:交付发布等待方。false = 无此等待方(已超时/已应
    /// 答),调用方回 ack ok:false。error 文案仅在 ok:false 时有意义
    /// (App 发布失败原因,透传口径同 0x01 拒开)。
    #[cfg_attr(all(not(unix), not(test)), allow(dead_code))] // 正式面经 FdBridgeHandler(unix 门);host 单测直驱
    pub(crate) fn answer_publish(&self, request_id: u64, ok: bool, error: Option<&str>) -> bool {
        let verdict = if ok {
            Ok(())
        } else {
            Err(match error {
                Some(e) => format!("App 发布失败: {e}"),
                None => "App 发布失败".to_string(),
            })
        };
        match lock_ok(&self.pending_publish).remove(&request_id) {
            Some(tx) => tx.send(verdict).is_ok(),
            None => false,
        }
    }
}
/// bridge 源表解析:逐条目串行 request_source(逐个,下一件的请求只在
/// 上一件 fd 落地后发出)→ `(SourceFile::Fd, rel)` 表(顺序保 items 序,
/// 与清单保序契约对齐)。任一条目失败即整单 Err(「源不可用: …」),
/// 已解析 fd 随半途表 Drop 全释放(无逐文件泄漏面)。
async fn resolve_bridge_sources(
    resolver: &SourceResolver,
    items: &[BridgeItem],
) -> Result<Vec<(SourceFile, String)>, String> {
    let mut sources = Vec::with_capacity(items.len());
    for it in items {
        let source = resolver.resolve(&it.r#ref).await?;
        sources.push((source, it.rel.clone()));
    }
    Ok(sources)
}

/// fdpass 消息的 daemon 分派(v2-P3 T4 落 `0x01 source_fd`;P4a 收件侧
/// 通道纪律改二:0x04/0x05 统一交付 [`DestQueries`],0x02 dest_fd/0x03
/// dest_commit 已随收件 fd 桥退役,值不复用):
/// - `0x01` + 1 fd → 交付 [`SourceResolver`];**无 fd 的 0x01 = App
///   拒开**(payload 可带 error 文案)→ resolve 立即失败;
/// - 未知 request_id(已超时/已交付/跨会话残留)→ ack ok:false,随帧
///   fd 就地 Drop 关闭;
/// - `0x04` dest_query(无 fd,F7 在场查询应答)→ 交付 DestQueries 的
///   查询等待面(final_state 对账的在场证明);字段缺失/随帧带 fd 按
///   未知消息口径 ack ok:false(不断连);
/// - `0x05` publish_result(无 fd,P4a 发布结果应答)→ 交付 DestQueries
///   的发布等待面(ok:false 时 error 文案可选,透传进「保存失败」终因);
///   ok 字段缺失/随帧带 fd 同按未知消息口径 ack ok:false(不断连);
/// - 未知 kind → ack ok:false(防御不断连——App 侧以 ack 判定应答是否
///   被受理)。
#[cfg(unix)]
pub(crate) struct FdBridgeHandler {
    sources: Arc<SourceResolver>,
    queries: Arc<DestQueries>,
}

#[cfg(unix)]
impl FdBridgeHandler {
    /// daemon 装配面(Daemon::start 的 fdpass 监听循环):resolver 句柄
    /// 经 [`Jobs::source_resolver`] / [`Jobs::dest_queries`] 取得,全进程
    /// 单实例共享。
    pub(crate) fn new(sources: Arc<SourceResolver>, queries: Arc<DestQueries>) -> Self {
        Self { sources, queries }
    }
}

#[cfg(unix)]
impl FdMsgHandler for FdBridgeHandler {
    fn on_msg(&self, msg: FdMsg, reply: FdReply) -> Pin<Box<dyn Future<Output = ()> + Send>> {
        let sources = self.sources.clone();
        let queries = self.queries.clone();
        Box::pin(async move {
            let request_id = msg.payload.get("request_id").and_then(Value::as_u64);
            let id_echo = msg.payload.get("request_id").cloned().unwrap_or(Value::Null);
            let ack = |ok: bool, error: Option<&str>| {
                let mut payload = json!({ "request_id": id_echo, "ok": ok });
                if let Some(e) = error {
                    payload["error"] = Value::from(e);
                }
                let _ = reply.send(FdMsg {
                    kind: KIND_ACK,
                    payload,
                    fd: None,
                });
            };
            match (msg.kind, msg.fd, request_id) {
                (KIND_SOURCE_FD, Some(fd), Some(id)) => {
                    if sources.deliver(id, fd) {
                        ack(true, None);
                    } else {
                        ack(false, Some("源请求已结束(超时或已交付)"));
                    }
                }
                // 无 fd 的 0x01 = 拒开(payload 的 error 文案可选)。文案
                // 由 daemon 收口成稳定词面(「App 拒开源(…)」)——任务
                // 失败语的词面是 UI/T10 的呈现契约,不透传 App 侧原始串
                (KIND_SOURCE_FD, None, Some(id)) => {
                    let reason = match msg.payload.get("error").and_then(Value::as_str) {
                        Some(detail) => format!("App 拒开源({detail})"),
                        None => "App 拒开源".to_string(),
                    };
                    if sources.refuse(id, &reason) {
                        ack(true, None);
                    } else {
                        ack(false, Some("源请求已结束(超时或已交付)"));
                    }
                }
                // 0x04 dest_query(F7 在场查询应答,无 fd):交付查询等待面;
                // exists/size 字段缺失按未知消息口径 ack ok:false(不断连)。
                (KIND_DEST_QUERY, None, Some(id)) => match (
                    msg.payload.get("exists").and_then(Value::as_bool),
                    msg.payload.get("size").and_then(Value::as_u64),
                ) {
                    (Some(exists), Some(size)) => {
                        if queries.answer(id, exists, size) {
                            ack(true, None);
                        } else {
                            ack(false, Some("查询请求已结束(超时或已应答)"));
                        }
                    }
                    _ => ack(false, Some("dest_query 字段缺失(exists/size)")),
                },
                // 0x05 publish_result(P4a 发布结果应答,无 fd):交付发布
                // 等待面;ok:false 时 error 文案可选(发布失败终因,透传)。
                (KIND_PUBLISH_RESULT, None, Some(id)) => {
                    match msg.payload.get("ok").and_then(Value::as_bool) {
                        Some(true) => {
                            if queries.answer_publish(id, true, None) {
                                ack(true, None);
                            } else {
                                ack(false, Some("发布请求已结束(超时或已应答)"));
                            }
                        }
                        Some(false) => {
                            let error = msg.payload.get("error").and_then(Value::as_str);
                            if queries.answer_publish(id, false, error) {
                                ack(true, None);
                            } else {
                                ack(false, Some("发布请求已结束(超时或已应答)"));
                            }
                        }
                        None => ack(false, Some("publish_result 字段缺失(ok)")),
                    }
                }
                _ => ack(false, Some("未知或暂不支持的 fdpass 消息")),
            }
        })
    }
}

// ---------- 发布协调器(P4a:收件私有暂存 + 完成发布) ----------

/// 发布协调器(spec §3.2/§6.1):round Ok(网络面收妥——全部落暂存、
/// finalize 过哈希)之后的**独立层**,驱动任务 保存中→(发布确认)→终态:
/// - 逐文件**并发** fs.publish(`DestQueries::publish`:独立 request_id、
///   独立 30s 超时;多文件不互相拖死);
/// - 全部应答 ok → 逐件收尾(删暂存终件 + 记录打 published,§6.2:记录
///   = Skip 锚保留)+ `settle_publish(true)`;匿名 incoming 卡直接落
///   done,**download 绑定卡的 done 合成留在驱动**(源端终态/混合后果的
///   终因只在源端,§6.1 终态合成增「发布已落定」条件——先落 done 会被
///   终态粘滞吞掉源端 failed 映射);
/// - 任一件失败 → `settle_publish(false)` + 立即 failed「保存失败(N 项):
///   首因」(**不分卡型**——本地终局事实,不让用户等源端);其余文件照常
///   发布保留(镜像 P2 部分缺失语义);失败件暂存保留待重试(§4)。
///
/// 取消竞速:起步即终态(取消先到)不发布——暂存保留;发布途中的取消不
/// 中断本协调器(收尾面的终态翻转对已终态卡自然无操作,已发布件照常
/// 清理,语义与「其余文件照常发布保留」一致)。
async fn publish_coordinator(
    state: Arc<DaemonState>,
    core: Arc<JobCore>,
    items: Vec<crate::inbox_dest::PublishItem>,
) {
    if core.is_terminal() {
        return; // 取消竞速先到:不发布(暂存保留,重发走 republish 分支)
    }
    let queries = state.jobs().dest_queries().clone();
    // 逐文件并发发布(独立 request_id/独立超时;克隆进各自 future)
    let futs: Vec<_> = items
        .iter()
        .cloned()
        .map(|it| {
            let queries = queries.clone();
            async move {
                let r = queries.publish(&it.final_rel, &it.path, it.size).await;
                (it, r)
            }
        })
        .collect();
    let results = futures_util::future::join_all(futs).await;
    let mut oks: Vec<crate::inbox_dest::PublishItem> = Vec::new();
    let mut fails: Vec<(crate::inbox_dest::PublishItem, String)> = Vec::new();
    for (it, r) in results {
        match r {
            Ok(()) => oks.push(it),
            Err(e) => fails.push((it, e)),
        }
    }
    // 成功收尾批(阻塞 fs 面:删暂存终件 + 记录打 published)落线程池;
    // 尽力而为——App 的 ok 应答是发布事实的终审,本地簿记失败仅留痕
    // (文件残留/记录未打标 → 下次对账重收重发布,App 侧撞名由发布面
    // policy 收口,§6.4)。
    if !oks.is_empty() {
        let home = state.home();
        let n_ok = oks.len();
        if let Err(e) = tokio::task::spawn_blocking(move || {
            for it in &oks {
                crate::inbox_dest::settle_published(&home, it);
            }
        })
        .await
        {
            tracing::warn!(%e, "发布收尾批任务收尾失败({n_ok} 件,簿记尽力而为)");
        }
    }
    if fails.is_empty() {
        core.settle_publish(true);
        if core.kind != "download" {
            core.finish("done", None); // 匿名卡无驱动,协调器直接收口
        }
        // download 绑定卡:done 合成留驱动(源端终态门 + 发布已落定)
        return;
    }
    let n = fails.len();
    let detail = fails
        .first()
        .map(|(_, e)| e.clone())
        .unwrap_or_else(|| "未知".to_string());
    core.settle_publish(false);
    core.finish(
        "failed",
        Some(format!("保存失败({n} 项): {detail}")),
    );
}

// ---------- 驱动监督(M4a-2) ----------

/// 驱动任务监督网:四个驱动(upload/download/remote_copy/local_copy)在
/// Ok/Err 两路都会 `core.finish(...)` 才返回,本包装只接**意外退出**——
/// panic(unwind)被 `catch_unwind` 捕获后,作业尚未终态即落 failed(文案
/// 钉死「内部错误: 传输驱动异常退出」),否则用户侧卡片永远 running 只能
/// 靠取消救。已终态(取消/完成竞速先到)不改写;正常路径透传无附加开销;
/// JoinHandle 形态不变(仍 JoinHandle<()>,返回 Ok);abort 取消路径不经
/// 此(挂起点直接杀任务,不产生 unwind)。panic 详情由默认 hook 照常落
/// stderr 留痕,此处不重复记日志。
async fn supervise_driver(core: Arc<JobCore>, fut: impl std::future::Future<Output = ()>) {
    use std::panic::AssertUnwindSafe;
    let caught = futures_util::FutureExt::catch_unwind(AssertUnwindSafe(fut)).await;
    if caught.is_err() && !core.is_terminal() {
        core.transition("failed", Some("内部错误: 传输驱动异常退出".into()));
    }
}

// ---------- upload 驱动 ----------

/// upload 的源形态(v2-P3 T4 泛化):路径形态(桌面/编排回显/重发,行为
/// 与泛化前一致)或 bridge 形态(Android fd 桥:仅 transfer.start
/// `source:"bridge"` 入口,驱动起步逐文件 request_source 懒解析)。
enum UploadSource {
    /// 路径形态:顶层输入(文件/目录),驱动内 collect_items 展开(部分
    /// 预检模式 = 已滤除消失项的现存输入)。
    Paths(Vec<PathBuf>),
    /// bridge 形态:逐文件元数据(元数据直构预扫,fd 懒解析)。
    Bridge(Vec<BridgeItem>),
}

/// bridge 上行条目(transfer.start `source:"bridge"` 的 items 元素;字段
/// 面钉死 v2-P3 T4)。`ref` 为 App 侧源引用(持久 URI 等,daemon 不
/// 解释,request_source 原样回传);`rel` 为清单相对路径(接收端落盘
/// 口径);`size`/`mtime` 为声明值(计数/记录面;清单 size 以 fd 的
/// fstat 为准)。
pub(crate) struct BridgeItem {
    pub r#ref: String,
    pub rel: String,
    pub size: u64,
    /// 契约钉死字段(计划 Interfaces 面;App 侧条目元数据)。daemon 当期
    /// 无消费点(清单面以 fd 的 fstat 为准),T6/T10 的对账/呈现面再取用
    /// ——按 allow 留痕,不删契约字段。
    #[allow(dead_code)]
    pub mtime: i64,
}

/// upload 驱动任务的载荷(spawn 边界一次性搬移)。
struct UploadJob {
    /// 源形态:路径(顶层输入)或 bridge(逐文件元数据)。
    source: UploadSource,
    to: String,
    dir: String,
    policy: TransferPolicy,
    cut_src: bool,
    /// 顶层已消失的输入(重发部分预检面,非空 = 重发):驱动照常传完在的
    /// 文件,Ok 后据此落 failed(spec §5.1 混合后果)。
    missing: Vec<String>,
    /// old_rels − 新预扫 rels:推流前对端 `.mtdss` 残留的清理清单(尽力
    /// 而为,失败仅 warn 不阻断,spec §5.3)。
    dropped: Vec<String>,
}

/// upload 驱动:跑 run_send_with(SendSpec 定向 + ctl 暂停/取消/进度),
/// 进度回调进注册表(聚合计数 + 速度 EMA + 节流事件);推流前尽力清理
/// dropped 的对端残留(重发路径);成功且 cut → 本地删源(先于 done 落
/// 定);终态按结果落定(取消竞速时粘滞,先到者胜):Ok 且 missing 非空
/// = 混合后果 failed(在的文件已照常交付),否则 done。
async fn upload_driver(
    state: Arc<DaemonState>,
    core: Arc<JobCore>,
    job: UploadJob,
    pause: Arc<PauseGate>,
    cancel_rx: watch::Receiver<bool>,
) {
    core.begin_running(pause.paused());
    let Some(session) = state.session() else {
        core.finish("failed", Some("未登录,作业无法发起".into()));
        return;
    };
    let spec = crate::flow::SendSpec {
        target_dir: job.dir.clone(),
        job_id: core.id.clone(),
        policy: job.policy.clone(),
    };
    // 源表物化(v2-P3 T4):路径形态 collect_items 展开(阻塞走盘,
    // collect_items 逻辑零改动);bridge 形态经 SourceResolver 逐文件
    // request_source 解析(逐个串行;建表后整表存活至 hash+send 收尾,
    // 见 SourceResolver 文档)。任一失败即整单收束 failed(路径形态
    // 文案与泛化前一致;bridge 为「源不可用: …」),已解析 fd 随半途
    // 表 Drop 释放。
    let sources: Vec<(SourceFile, String)> = match &job.source {
        UploadSource::Paths(files) => match crate::flow::path_sources(files).await {
            Ok(v) => v,
            Err(e) => {
                core.finish("failed", Some(format!("{e:#}")));
                return;
            }
        },
        UploadSource::Bridge(items) => {
            match resolve_bridge_sources(state.jobs().source_resolver(), items).await {
                Ok(v) => v,
                Err(e) => {
                    core.finish("failed", Some(e));
                    return;
                }
            }
        }
    };
    // 推流前(重发路径):dropped 的对端 `.mtdss` 残留清理,尽力而为——
    // 失败仅 warn 不阻断(spec §5.1;对端离线时传输自会按退避口径收场,
    // 清理缺席无害,残留留给下一次重发清理)。
    if !job.dropped.is_empty() {
        if let Err(e) = state
            .ctrl()
            .cleanup_outbox(&state, &job.to, &job.dir, &job.dropped)
            .await
        {
            tracing::warn!(detail = %e.rpc().1, "dropped 残留清理失败(不阻断)");
        }
    }
    // 进度回调:引擎 SendProgress → 注册表聚合/节流事件(克隆 Arc 进闭包,
    // out_report 另行借用 core 打日志,两者不冲突)
    let tracking = core.clone();
    let progress = Arc::new(
        move |p: &SendProgress<'_>| -> () { tracking.track_send(p) },
    );
    // on_relay(v2-P1c T5):兜底生效即调恰一次 → 注册表印 relay 标记(闭包
    // 捕获 state+job_id;遗留 send 无此面)
    let relay_state = state.clone();
    let relay_job = core.id.clone();
    let on_relay = Arc::new(move || relay_state.jobs().mark_relay(&relay_job));
    let ctl = crate::flow::SendCtl {
        pause: Some(pause),
        cancel: Some(cancel_rx),
        progress: Some(progress),
        on_relay: Some(on_relay),
    };
    let home = state.home();
    let opts = state.send_opts();
    // 遗留 send_log 事件不进作业面(event.transfer 是作业的进度契约);
    // 清单/完成行落 tracing 供排障。
    let out_report = |line: &str| tracing::info!(job = %core.id, "{line}");
    let result = crate::flow::run_send_with(
        &session,
        &home,
        state.hub(),
        sources,
        &job.to,
        &spec,
        &out_report,
        &opts,
        &ctl,
    )
    .await;
    match result {
        Ok(()) => {
            // cut 收尾:仅路径形态(bridge 形态 Android UI 恒 cut=false,
            // spec §5.2 裁定④;引擎侧防御 no-op 留痕,不 panic)
            if job.cut_src {
                match &job.source {
                    UploadSource::Paths(files) => cut_finalize(files).await,
                    UploadSource::Bridge(_) => {
                        tracing::warn!(job = %core.id, "bridge 形态不支持 cut,跳过删源")
                    }
                }
            }
            // 混合后果(spec §5.1):驱动 Ok 且有源已消失 → failed 带缺失
            // 计数(在的文件已照常传完交付);否则 done。
            if job.missing.is_empty() {
                core.finish("done", None);
            } else {
                core.finish(
                    "failed",
                    Some(format!(
                        "{} 项源已不存在(已清理对端残留)",
                        job.missing.len()
                    )),
                );
            }
        }
        Err(e) => core.finish("failed", Some(format!("{e:#}"))),
    }
}

/// cut 收尾:驱动成功后本地删源(delete_paths 回收优先,尽力而为——
/// 失败仅留痕不翻终态;删源先于 done 落定,wait 返回时磁盘事实已定)。
async fn cut_finalize(files: &[PathBuf]) {
    let paths: Vec<String> = files
        .iter()
        .map(|p| p.to_string_lossy().into_owned())
        .collect();
    match tokio::task::spawn_blocking(move || mtdss_core::fs::delete_paths(&paths, false)).await {
        Ok(Ok((deleted, recycled, failed))) if failed.is_empty() => {
            tracing::info!(deleted, recycled, "cut 删源完成(回收优先)")
        }
        Ok(Ok((deleted, recycled, failed))) => {
            tracing::warn!(deleted, recycled, ?failed, "cut 删源部分失败(尽力回收)")
        }
        Ok(Err(e)) => tracing::warn!(%e, "cut 删源失败(源保留)"),
        Err(e) => tracing::warn!(%e, "cut 删源任务收尾失败(源保留)"),
    }
}

// ---------- download 驱动(Task 7) ----------

/// download 驱动任务的载荷(spawn 边界一次性搬移)。
struct DownloadJob {
    /// 源设备(orchestrate 的 ctrl 对端;cancel_job 通知同一对端)。
    src_device: String,
    /// 远端绝对路径(源端视角)。
    paths: Vec<String>,
    /// 本机落盘目录(已校验绝对形态)。
    dir: String,
    policy: TransferPolicy,
    /// 成功后对源端 ctrl delete 源路径(回收优先,尽力而为)。
    cut: bool,
    /// 重发编排的旧 rel 集(reinitiate download,Task 8):orchestrate 携
    /// 给源端做 partial 预检 + dropped 统一清本端残留;空 = 普通首发。
    old_rels: Vec<String>,
}

/// download 驱动的源端作业态轮询间隔:轮失败后的源端重试间隙里,本地作业
/// 保持 running 等重来的 ConnectRequest;终局失败(源端重试耗尽/协议终局/
/// 查询持续不可达)由本探测收口。3s = 失败感知延迟与 ctrl 请求开销的折中
/// (池内连接复用,单次请求毫秒级)。
const DOWNLOAD_SOURCE_POLL: Duration = Duration::from_secs(3);

/// download 驱动:orchestrate 到源设备(源端起同 id upload 驱动;重发编排
/// 携 old_rels,源端 partial 预检 + dropped 统一清本端残留)→ 周期 job_query
/// 探测源端作业态合流收口(见 `download_wait_terminal`):源端 failed/
/// missing → 本地 failed;源端 cancelled → 本地 cancelled;源端 done + 轮
/// Ok 落定 → done。回带 rels 非空即回填 store(重发原料)。done 且 cut →
/// 对源端 ctrl delete 源路径(回收优先,尽力而为,失败仅留痕不翻终态)。
async fn download_driver(state: Arc<DaemonState>, core: Arc<JobCore>, job: DownloadJob, pause: Arc<PauseGate>) {
    core.begin_running(pause.paused());
    download_network_phase(&state, core, job).await;
}

/// download 的网络相本体(编排 → 源端作业态轮询合流收口):download_driver
/// 与收件箱重发驱动([`inbox_republish_driver`] 的相位二)共用——后者在
/// republish 相落定后以同一编排面重收缺失集,staged 件经发布落定的记录门
/// + 0x04 在场查询天然 Skip(分区保证,spec §6.3)。
async fn download_network_phase(state: &Arc<DaemonState>, core: Arc<JobCore>, job: DownloadJob) {
    let ctrl = state.ctrl();
    // orchestrate:to_device = 自己(源端把数据传回本机)
    let me = match state.self_device_id() {
        Some(me) => me,
        None => {
            core.finish("failed", Some("未登录,作业无法发起".into()));
            return;
        }
    };
    let resp = match ctrl
        .orchestrate(
            state,
            &job.src_device,
            &core.id,
            &me,
            &job.paths,
            &job.dir,
            &job.policy,
            &job.old_rels,
        )
        .await
    {
        Ok(r) => r,
        Err(e) => {
            let (_, msg) = e.rpc();
            core.finish("failed", Some(msg));
            return;
        }
    };
    if !resp.accepted {
        core.finish("failed", Some(format!("源端拒绝编排: {}", resp.detail)));
        return;
    }
    // 源端作业态轮询的注入面(评审 Important#2 可测缝):生产 = ctrl
    // job_query,查询失败映射 None(忽略,下轮再试);测试注入合成态。
    // 回带 rels 原样透传(非空回填在 wait_terminal 收口,spec §4.1)。
    let poll_ctrl = ctrl.clone();
    let poll_state = state.clone();
    let poll_id = core.id.clone();
    let poll_src = job.src_device.clone();
    let mut poll = move || {
        let (ctrl, st, id, src) =
            (poll_ctrl.clone(), poll_state.clone(), poll_id.clone(), poll_src.clone());
        async move {
            ctrl.job_query(&st, &src, &id)
                .await
                .ok()
                .map(|q| (q.state, q.error, q.rels))
        }
    };
    download_wait_terminal(
        state,
        core,
        &job,
        DOWNLOAD_SOURCE_POLL,
        DOWNLOAD_SOURCE_DONE_GRACE,
        SOURCE_POLL_FAIL_BOUND,
        &mut poll,
    )
    .await;
}

/// 源端作业「已完成」后本地轮尚未 Ok 落定的宽限窗:源端发完到接收端
/// finalize/EOF 舞步收尾有正常时差(Task 8 起轮 Ok 只记落定事实,done
/// 收口等驱动轮询合流),窗内等轮落定;超窗仍落 = 本地轮已死(收尾错误
/// /取消竞速,再无 ConnectRequest 可救)——按失败收口,杜绝 `wait_job`
/// 无界挂起(评审 Important#2)。**只作用于网络相**(§6.1):轮一旦 Ok
/// 落定、收件箱轮进入发布窗(P4a 的 publishing,每文件独立 30s),本窗
/// 即不再判杀——发布窗由发布门(`PublishPhase`)治理,宽限侵入即假失败。
const DOWNLOAD_SOURCE_DONE_GRACE: Duration = Duration::from_secs(10);

/// download 驱动的终局等待(编排接受后):周期轮询源端作业态合流收口
/// ——failed/cancelled/missing 即时映射本地终态;**done 需双条件**(源端
/// done + 本地轮 Ok 落定,Task 8 起轮成功不再直接落 done,混合后果的
/// failed 终因只在源端,先落 done 会被终态粘滞吞掉映射;**P4a 再增发布
/// 门**——收件箱轮的发布窗 Pending 期间继续等,发布未落定不合成 done,
/// §6.1);源端 done 而轮未落定进宽限窗,超窗 = 本地轮已死(收尾错误/
/// 取消竞速)按失败收口。
/// 查询持续不可达(`fail_bound` 内无一次成功查询)→ 失败收口(对端真下
/// 线不悬挂;单次瞬时失败不烧计数,任一成功查询归零;取消经 waiter 臂/
/// abort 即时胜出)。回带 rels 非空即回填 store(spec §4.1:源端 upload
/// 驱动的预扫全集 → 本端记录的 reinitiate 原料)。收口 done 且 cut → 对
/// 源端 ctrl delete 源路径(回收优先,尽力而为)。
/// `poll` 为源端作业态查询的注入缝,返回 `None` = 查询失败(单次忽略,
/// 持续超 `fail_bound` 失败收口);Some 携 (state, error, rels)。
#[allow(clippy::too_many_arguments)] // 注入缝全参数面(poll_tick/done_grace/fail_bound 皆测试可调)
async fn download_wait_terminal<F, Fut>(
    state: &Arc<DaemonState>,
    core: Arc<JobCore>,
    job: &DownloadJob,
    poll_tick: Duration,
    done_grace: Duration,
    fail_bound: Duration,
    mut poll: F,
) where
    F: FnMut() -> Fut,
    Fut: std::future::Future<Output = Option<(String, String, Vec<String>)>>,
{
    let mut ticker = tokio::time::interval(poll_tick);
    ticker.set_missed_tick_behavior(MissedTickBehavior::Delay);
    let waiter = state.jobs().wait(&core.id);
    tokio::pin!(waiter);
    let mut done_since: Option<Instant> = None;
    // 查询连续失败计时:首败记时,任一成功查询归零;超界 → 失败收口
    let mut fail_since: Option<Instant> = None;
    loop {
        tokio::select! {
            fin = &mut waiter => {
                // 本地终态:取消(cancel watch + abort)或防御面直接终态。
                // Err = 无此作业(极端竞窗早到)同口径退出。cut 收尾仅 done
                // 执行(轮 Ok 不再于此臂产生 done,done 收口在 tick 臂)。
                if matches!(&fin, Ok(s) if s.state == "done") && job.cut {
                    cut_finalize_remote(state, &job.src_device, &job.paths).await;
                }
                return;
            }
            _ = ticker.tick() => {
                match poll().await {
                    Some((q, err, rels)) => {
                        // 任一成功查询:连续失败计时归零(单次瞬时失败不计)
                        fail_since = None;
                        // rels 回填(spec §4.1):源端回带预扫全集,非空即
                        // 写回 store;空 = 尚未回填,退化可接受面
                        if !rels.is_empty() {
                            core.backfill_rels(&rels);
                        }
                        if q == "failed" {
                            let detail = if err.is_empty() {
                                "源端作业失败".to_string()
                            } else {
                                format!("源端作业失败: {err}")
                            };
                            core.finish("failed", Some(detail));
                            return;
                        }
                        if q == "cancelled" {
                            core.finish("cancelled", None);
                            return;
                        }
                        // missing:源端无此作业(重启/登出拆卸)——永不完成,按失败收口
                        if q == "missing" {
                            core.finish("failed", Some("源端已无此作业(设备可能重启)".into()));
                            return;
                        }
                        if q == "done" {
                            if core.round_ok() {
                                // 发布门(P4a,§6.1 终态合成增「发布已落定」
                                // 条件):收件箱轮 Ok 落定后进入发布窗
                                // (Pending)——宽限**不适用**(只作用于网络
                                // 相),等待协调器落定;NotApplicable(桌面轮/
                                // 无待发布件)与 Settled(true) 放行合成。
                                match core.publish_phase() {
                                    PublishPhase::NotApplicable | PublishPhase::Settled(true) => {
                                        // 源端 done + 本地轮已 Ok 落定(+
                                        // 发布已落定/不适用)→ 收口 done
                                        // (源端发完 = 收端已 finalize 的数据
                                        // 事实);done 且 cut → 删源(先于
                                        // done 落定)
                                        if job.cut {
                                            cut_finalize_remote(state, &job.src_device, &job.paths).await;
                                        }
                                        core.finish("done", None);
                                        return;
                                    }
                                    // Settled(false):协调器已落 failed「保存
                                    // 失败」——终态粘滞下本臂理论不可达,
                                    // 防御退出(waiter 臂早已收口)
                                    PublishPhase::Settled(false) => return,
                                    // 发布窗:继续轮询(宽限不侵入,见上)
                                    PublishPhase::Pending => {}
                                }
                            } else {
                                // 首见记时,宽限内等轮落定;超窗仍未落定 = 本地
                                // 轮已死(收尾错误/取消竞速,再无 ConnectRequest
                                // 可救)——按失败收口,杜绝无界挂起。
                                let since = *done_since.get_or_insert_with(Instant::now);
                                if since.elapsed() >= done_grace {
                                    core.finish(
                                        "failed",
                                        Some(
                                            "源端已完成,但本地接收未在宽限期内收口(本地轮收尾失败或已取消)"
                                                .into(),
                                        ),
                                    );
                                    return;
                                }
                            }
                        }
                        // queued/running:继续等
                    }
                    None => {
                        // 查询失败(打洞中断/对端不可达):单次瞬时忽略;持续超
                        // 界 = 对端真下线,失败收口(永不下线的 running 比显式
                        // 失败更糟,评审 Important)。
                        let since = *fail_since.get_or_insert_with(Instant::now);
                        if since.elapsed() >= fail_bound {
                            core.finish(
                                "failed",
                                Some(format!(
                                    "源端持续不可达(>{}s),已停止等待——可重试",
                                    fail_bound.as_secs()
                                )),
                            );
                            return;
                        }
                    }
                }
            }
        }
    }
}

/// download 的 cut 收尾:本地 done 后对源端 ctrl delete 源路径(force=false,
/// 回收优先;尽力而为——失败仅留痕不翻终态,done 已落定)。
async fn cut_finalize_remote(state: &Arc<DaemonState>, device: &str, paths: &[String]) {
    match state.ctrl().delete(state, device, paths, false).await {
        Ok(r) => {
            if r.failed.is_empty() {
                tracing::info!(deleted = r.deleted, recycled = r.recycled, "download cut 收尾完成(源端回收删除)")
            } else {
                tracing::warn!(?r.failed, "download cut 收尾部分失败(尽力回收)")
            }
        }
        Err(e) => tracing::warn!(e = %e.rpc().1, "download cut 收尾失败(源端路径保留)"),
    }
}

// ---------- 收件箱重发驱动(P4a T2:重发分区与 republish) ----------

/// 收件箱 download 重发驱动载荷(P4a T2,spawn 边界一次性搬移)。
struct InboxRepubJob {
    /// 分区的 staged 集(相位一:暂存 done 未发布件直接重发布;空 = 纯网络
    /// 重发,相位一起步即空转)。
    staged: Vec<crate::inbox_dest::PublishItem>,
    /// 网络相载荷(既有 [`DownloadJob`]:缺失集编排/合流的原参数面)。
    net: DownloadJob,
    /// 缺失集为空 = **纯本地**(相位二不起:免登录/免对端,spec §6.3)。
    republish_only: bool,
}

/// 收件箱 download 重发驱动(P4a T2,spec §6.3):**同一新 job** 同承两相
/// ——
/// - **相位一 republish**:staged 集直接驱动发布协调器(翻可见态
///   publishing/保存中 + 逐文件并发 fs.publish)。**先于网络相**(顺序钉
///   死):发布落定(记录打 published)后,网络相里源端重推的同名件经
///   final_state 记录门 + 0x04 在场查询天然 Skip——staged 件永不进接收
///   集,引擎 Full 截断与 unique_target 改名 "(1)" 均无从触及(评审验收
///   #1/#2 的分区根)。协调器任一件失败已落 failed「保存失败(N 项)」
///   (终态粘滞),驱动即返、网络相不起——失败保留待重试(§4)。
/// - **相位二网络相**:缺失集走既有 download 编排/轮询合流
///   ([`download_network_phase`],与首发驱动同一本体;**起跑前重置相位
///   一的轮落定面 round_ok**——done 合成只认本相的轮落定,陈旧值会把
///   「源端 done 早于轮落定」的常态序窗短路成假 done,缺件的发布协调
///   器随之永不 spawn,评审 Important#1);republish_only 无此相,驱动
///   按发布落定直接合成 done(纯本地无源端面可等)。
///
/// 取消竞速:mark_round_publishing 返回 false(取消先到)不发布即返
/// (暂存保留,重发再走分区);发布途中的取消不中断协调器(收尾翻转对已
/// 终态卡无操作,已发布件照常清理——与既有协调器语义一致)。
async fn inbox_republish_driver(
    state: Arc<DaemonState>,
    core: Arc<JobCore>,
    job: InboxRepubJob,
    pause: Arc<PauseGate>,
) {
    core.begin_running(pause.paused());
    if !job.staged.is_empty() {
        if !core.mark_round_publishing() {
            return; // 取消竞速先到:不开发布窗(暂存保留,重发再走分区)
        }
        publish_coordinator(state.clone(), core.clone(), job.staged).await;
        if !matches!(core.publish_phase(), PublishPhase::Settled(true)) {
            // Settled(false)=协调器已落 failed「保存失败」(粘滞);理论不可
            // 达的其它相位按同口径退出——网络相不得在发布未落定下起跑。
            return;
        }
        // 发布已落定 → 回网络相可见态(保存中→传输中,状态诚实:publishing
        // 只属于真发布窗);暂停面随门(M-6:发布窗内 set_paused 已把可见
        // 态翻 paused,此处不得翻回 running 制造「暂停被吞」)。
        core.begin_running(pause.paused());
    }
    if job.republish_only {
        core.finish("done", None); // 纯本地:无源端面,驱动直接合成终态
        return;
    }
    // 相位切换点(P4a T2 fix,评审 Important#1):相位一的轮落定面
    // (round_ok=true)是 republish 相的事实,不得带入网络相——否则源端
    // done 与 B 侧轮落定(begin_publish 重开窗)之间的常态序窗内,done
    // 合成门被陈旧 round_ok+Settled(true) 短路,缺件的发布协调器因终态
    // 粘滞永不 spawn(见 [`JobCore::reset_round_gate_for_network`])。
    core.reset_round_gate_for_network();
    download_network_phase(&state, core, job.net).await;
}

// ---------- remote_copy 驱动(Task 8:A→B 编排) ----------

/// remote_copy 驱动任务的载荷(spawn 边界一次性搬移)。
struct RemoteCopyJob {
    /// 源设备(orchestrate/cancel_job/job_query/cut 收尾的 ctrl 对端;数据
    /// 从它读出)。
    from: String,
    /// 远端绝对路径(from 设备视角)。
    paths: Vec<String>,
    /// 目标设备(数据直落它,不经操作端)。
    to: String,
    /// 目标目录(to 设备视角,已校验绝对形态)。
    dir: String,
    policy: TransferPolicy,
    /// 成功后对源端 ctrl delete 源路径(回收优先,尽力而为)。
    cut: bool,
    /// 重发编排的旧 rel 集(reinitiate remote_copy,Task 8):orchestrate
    /// 携给 from 端做 partial 预检 + dropped 统一清 to 端残留;空 = 首发。
    old_rels: Vec<String>,
}

/// remote_copy 的源端作业态轮询间隔(brief 钉死 1.5s):本端卡片可见期间
/// 持续探测 from 端作业态,终局即时映射本端终态。池内连接复用,单次请求
/// 毫秒级;1.5s = 终局感知延迟与 ctrl 请求开销的折中。
const REMOTE_COPY_SOURCE_POLL: Duration = Duration::from_millis(1500);

/// remote_copy 驱动:预扫(① from 端逐源 stat——源缺失在编排前快败;②
/// to 端目标目录 list——目录缺失 = 无冲突,截断按顶层可见集比对 + 警示;
/// 命中同名仅留痕,策略以调用方下发表为准)→ orchestrate 发给 **from 设备**
/// (to_device = to;from 起回显 job_id 的 upload 驱动,ConnectRequest 定向
/// to)→ 1.5s 轮询 from 端 job_query 至终态并即时映射本端终态;done 且
/// cut → 对 from 端 ctrl delete 源路径(回收优先,尽力而为)。
async fn remote_copy_driver(state: Arc<DaemonState>, core: Arc<JobCore>, job: RemoteCopyJob, pause: Arc<PauseGate>) {
    core.begin_running(pause.paused());
    let ctrl = state.ctrl();
    // 预扫 ①:from 端逐源 stat(顶层名 + is_dir;已知 V1 限制:冲突策略
    // 顶层粒度,嵌套同名交接收端引擎安全网)。重发编排(old_rels 非空)
    // 容忍顶层已消失项:NOT_FOUND 留痕跳过冲突比对(消失项无从冲突)——
    // 剔除与 missing 计数、dropped 残留清理都是源端 partial 预检的职责
    // (spec §5.1);首发仍保持快败口径。
    let mut src_names: Vec<(String, bool)> = Vec::with_capacity(job.paths.len());
    for p in &job.paths {
        match ctrl.stat(&state, &job.from, p).await {
            Ok(d) => src_names.push((d.name, d.is_dir)),
            Err(CtrlErr::Peer { code, .. }) if code == "NOT_FOUND" && !job.old_rels.is_empty() => {
                tracing::info!(path = %p, "重发预扫:源已消失,交源端 partial 预检剔除");
            }
            Err(e) => {
                let (_, msg) = e.rpc();
                core.finish("failed", Some(format!("源端预扫失败({p}): {msg}")));
                return;
            }
        }
    }
    // 预扫 ②:to 端目标目录 list。目录缺失(NOT_FOUND)= 无冲突——接收端
    // 会按定向目录建出,与 upload 的目录语义一致。
    let (entries, truncated) = match ctrl.list(&state, &job.to, &job.dir).await {
        Ok(out) => (out.entries, out.truncated),
        Err(CtrlErr::Peer { code, .. }) if code == "NOT_FOUND" => (Vec::new(), false),
        Err(e) => {
            let (_, msg) = e.rpc();
            core.finish("failed", Some(format!("目标目录预扫失败: {msg}")));
            return;
        }
    };
    if truncated {
        tracing::warn!(dir = %job.dir, "目标目录列表截断:预扫按顶层可见集比对(V1 顶层粒度策略边界)");
    }
    let conflicts: Vec<&str> = src_names
        .iter()
        .filter(|(name, _)| entries.iter().any(|e| e.name == *name))
        .map(|(name, _)| name.as_str())
        .collect();
    if !conflicts.is_empty() {
        tracing::info!(?conflicts, "remote_copy 预扫命中目标同名条目(策略以调用方下发表为准)");
    }
    // orchestrate:发给 **from 设备**,to_device = to(A 的 upload 驱动把
    // 数据直传给 B,数据不经本机落地;重发编排携 old_rels,from 端 partial
    // 预检 + dropped 统一清 to 端残留)
    let resp = match ctrl
        .orchestrate(
            &state,
            &job.from,
            &core.id,
            &job.to,
            &job.paths,
            &job.dir,
            &job.policy,
            &job.old_rels,
        )
        .await
    {
        Ok(r) => r,
        Err(e) => {
            let (_, msg) = e.rpc();
            core.finish("failed", Some(msg));
            return;
        }
    };
    if !resp.accepted {
        core.finish("failed", Some(format!("源端拒绝编排: {}", resp.detail)));
        return;
    }
    // 源端作业态轮询的注入面(评审 Important#2 同款可测缝):生产 = ctrl
    // job_query,查询失败映射 None(忽略,下轮再试);测试注入合成态。
    // 回带 rels 原样透传(非空回填在 wait_terminal 收口,spec §4.1)。
    let poll_ctrl = ctrl.clone();
    let poll_state = state.clone();
    let poll_id = core.id.clone();
    let poll_from = job.from.clone();
    let mut poll = move || {
        let (ctrl, st, id, from) =
            (poll_ctrl.clone(), poll_state.clone(), poll_id.clone(), poll_from.clone());
        async move {
            ctrl.job_query(&st, &from, &id)
                .await
                .ok()
                .map(|q| (q.state, q.error, q.rels))
        }
    };
    remote_copy_wait_terminal(
        &state,
        core,
        &job,
        REMOTE_COPY_SOURCE_POLL,
        SOURCE_POLL_FAIL_BOUND,
        &mut poll,
    )
    .await;
}

/// remote_copy 的终局等待(from 端编排接受后):周期轮询 from 端作业态并
/// **即时映射**本端终态——done 无宽限窗(与 download 不同:本机无数据面,
/// from 端驱动收口 = to 端已 finalize,不存在收端时差);failed/cancelled/
/// missing 同 download 口径。查询持续不可达(`fail_bound` 内无一次成功查
/// 询)→ 失败收口(对端真下线不悬挂;单次瞬时失败不烧计数,任一成功查询
/// 归零;取消经 waiter 臂/abort 即时胜出)。回带 rels 非空即回填 store
/// (同 download,spec §4.1)。done 且 cut → 对 from 端 ctrl delete 源
/// 路径(回收优先,尽力而为;删源先于 done 落定)。本地 cancel 落定即出
/// (waiter 臂,防御性:registry.cancel 同时在 abort 本任务,双臂竞速
/// 无害——终态粘滞)。
/// `poll` 为源端作业态查询的注入缝(评审 Important#2 同款),返回 None =
/// 查询失败(单次忽略,持续超 `fail_bound` 失败收口);Some 携
/// (state, error, rels)。
async fn remote_copy_wait_terminal<F, Fut>(
    state: &Arc<DaemonState>,
    core: Arc<JobCore>,
    job: &RemoteCopyJob,
    poll_tick: Duration,
    fail_bound: Duration,
    mut poll: F,
) where
    F: FnMut() -> Fut,
    Fut: std::future::Future<Output = Option<(String, String, Vec<String>)>>,
{
    let mut ticker = tokio::time::interval(poll_tick);
    ticker.set_missed_tick_behavior(MissedTickBehavior::Delay);
    let waiter = state.jobs().wait(&core.id);
    tokio::pin!(waiter);
    // 查询连续失败计时:首败记时,任一成功查询归零;超界 → 失败收口
    // (与 download_wait_terminal 同一兜底语义,评审 Important)
    let mut fail_since: Option<Instant> = None;
    loop {
        tokio::select! {
            // 本地终态先行(cancel 竞速;终态粘滞使双臂无害)
            _ = &mut waiter => return,
            _ = ticker.tick() => {
                match poll().await {
                    Some((q, err, rels)) => {
                        // 任一成功查询:连续失败计时归零(单次瞬时失败不计)
                        fail_since = None;
                        // rels 回填(spec §4.1):from 端回带预扫全集,非空
                        // 即写回 store;空 = 尚未回填,退化可接受面
                        if !rels.is_empty() {
                            core.backfill_rels(&rels);
                        }
                        if q == "done" {
                            if job.cut {
                                cut_finalize_remote(state, &job.from, &job.paths).await;
                            }
                            core.finish("done", None);
                            return;
                        }
                        if q == "failed" {
                            let detail = if err.is_empty() {
                                "源端作业失败".to_string()
                            } else {
                                format!("源端作业失败: {err}")
                            };
                            core.finish("failed", Some(detail));
                            return;
                        }
                        if q == "cancelled" {
                            core.finish("cancelled", None);
                            return;
                        }
                        // missing:from 端无此作业(重启/登出拆卸)——永不完成,按失败收口
                        if q == "missing" {
                            core.finish("failed", Some("源端已无此作业(设备可能重启)".into()));
                            return;
                        }
                        // queued/running:继续等
                    }
                    None => {
                        // 查询失败(打洞中断/对端不可达):单次瞬时忽略;持续超
                        // 界 = 对端真下线,失败收口(永不下线的 running 比显式
                        // 失败更糟,评审 Important)。
                        let since = *fail_since.get_or_insert_with(Instant::now);
                        if since.elapsed() >= fail_bound {
                            core.finish(
                                "failed",
                                Some(format!(
                                    "源端持续不可达(>{}s),已停止等待——可重试",
                                    fail_bound.as_secs()
                                )),
                            );
                            return;
                        }
                    }
                }
            }
        }
    }
}

// ---------- local_copy 驱动(Task 7) ----------

/// local_copy 驱动任务的载荷(spawn 边界一次性搬移)。
struct LocalCopyJob {
    /// 本机源路径(顶层)。
    paths: Vec<PathBuf>,
    /// 本机目标目录(已校验存在且为目录)。
    dir: String,
    /// true = 移动快路(逐顶层条目 rename);false = fs::copy_paths 复制。
    cut: bool,
    /// 冲突策略(M4b 移交债①):skip 命中整项跳过(计入 skipped),与
    /// 传输面同口径。
    policy: TransferPolicy,
}

/// local_copy 驱动:spawn_blocking 跑 core fs 策略版(整树复制/回收删除级
/// 阻塞调用直跑会占死异步 worker,与 ctrl 面同一收口纪律);起步先过暂停
/// 门(门上等恢复,cancel 由注册表 abort 本任务兜底——阻塞中的 fs 操作不可
/// 中断,abort 落在 await 点,复制线程自然跑完被弃)。cut=false →
/// `copy_paths_with`,true → `move_entries_with`;终局:outcome.failed 非
/// 空 → failed 携带失败表明细(沿现形态),空 → mark_all_done 补齐计数面
/// 再 done;skip 命中数终局前回填(`record_skipped`,终态帧与快照自带)。
async fn local_copy_driver(core: Arc<JobCore>, job: LocalCopyJob, pause: Arc<PauseGate>) {
    core.begin_running(pause.paused());
    if pause.paused() {
        pause.wait().await;
    }
    let paths = job.paths.clone();
    let dir = job.dir.clone();
    let cut = job.cut;
    let policy = job.policy.clone();
    // 统一结果面 (成功计数, 逐条失败表, skip 命中数):copy/move 两路同形,
    // 终局判定不分支(copy 沿 legacy「任一条失败整批 Err」,失败表恒空——
    // 失败经 Ok(Err) 臂;move 逐对失败表不弃批,失败经 failed 表臂)。
    let outcome = tokio::task::spawn_blocking(move || -> Result<(usize, Vec<mtdss_core::fs::PathOpFailure>, u64), mtdss_core::fs::FsError> {
        if cut {
            // 移动快路:逐顶层条目 rename 入目标目录(文件覆盖既有文件、
            // 目录撞既有名逐对失败,与 rename 原生语义一致;跨卷回退暂存
            // 复制 + 源永久删除,fs 层透明处理)。skip 命中整对跳过,
            // overwrite 命中 rename 直通(判定键 = 目标 to 串,fs 层口径)。
            let mut pairs: Vec<(PathBuf, PathBuf)> = Vec::with_capacity(paths.len());
            for p in &paths {
                let Some(name) = p.file_name() else {
                    return Err(mtdss_core::fs::FsError::InvalidPath(format!(
                        "移动输入无文件名(卷根等): {}",
                        p.display()
                    )));
                };
                let to = std::path::Path::new(&dir).join(name);
                pairs.push((p.clone(), to));
            }
            let out = mtdss_core::fs::move_entries_with(&pairs, &policy)?;
            Ok((out.moved.len(), out.failed, out.skipped.len() as u64))
        } else {
            // 复制:skip 命中整项跳过不落盘,overwrite 命中清文件型占用后
            // 原位落位(判定键 = 目标目录拼名,fs 层口径);未命中沿 legacy
            // 自动改名。
            let out = mtdss_core::fs::copy_paths_with(
                &paths,
                std::path::Path::new(&dir),
                &policy,
            )?;
            Ok((out.copied.len(), out.failed, out.skipped.len() as u64))
        }
    })
    .await;
    match outcome {
        Ok(Ok((n, failed, skipped))) => {
            core.record_skipped(skipped);
            if failed.is_empty() {
                core.mark_all_done();
                core.finish("done", None);
            } else {
                let detail: Vec<String> = failed
                    .iter()
                    .map(|f| format!("{}({}: {})", f.path, f.code, f.message))
                    .collect();
                core.finish(
                    "failed",
                    Some(format!("移动 {n} 项成功,{} 项失败: {}", failed.len(), detail.join("; "))),
                );
            }
        }
        Ok(Err(e)) => core.finish("failed", Some(e.to_string())),
        Err(e) => core.finish("failed", Some(format!("本地复制任务收尾失败: {e}"))),
    }
}

// ---------- 单元测试(评审 Critical#1 / Important#2 回归) ----------

#[cfg(test)]
mod tests {
    use super::*;
    use crate::session::Session;

    /// 换装 std panic hook 的测试共用此串行化锁:hook 是进程全局,并行
    /// 测试线程交错 take/set 会互相踩(把别人的 hook 当 prev 恢复)。
    /// 守卫在 take_hook 前取,保持到末次 set_hook 恢复之后。
    static PANIC_HOOK_LOCK: std::sync::Mutex<()> = std::sync::Mutex::new(());

    /// 测试态:独立临时 home + 已登录会话(device_id 即参数;home 生命周期
    /// 到函数尾即弃,后续仅用内存面)。
    fn test_state(device_id: &str) -> Arc<DaemonState> {
        let home = tempfile::tempdir().unwrap();
        test_state_at(device_id, home.path())
    }

    /// test_state 的指定 home 变体(重启播种等跨会话用例:调用方持有
    /// TempDir 以在两次「会话」装配之间保住磁盘事实)。
    fn test_state_at(device_id: &str, home: &std::path::Path) -> Arc<DaemonState> {
        let (events, _) = tokio::sync::broadcast::channel(16);
        // 更新服务(Task 11):缺省失败安全配置(.invalid/无钥)——jobs
        // 测试不触更新面,占位即可;非 Windows 为单元占位。
        #[cfg(windows)]
        let update = std::sync::Arc::new(crate::update::UpdateService::new(
            crate::update::UpdateConfig::default(),
            events.clone(),
            tokio::sync::watch::channel(false).0,
        ));
        #[cfg(not(windows))]
        let update = ();
        let state = DaemonState::load(
            home,
            home.join("inbox"),
            None,
            Default::default(),
            Default::default(),
            events,
            update,
        )
        .unwrap();
        state
            .login(Session {
                server: "http://test".into(),
                jwt: "jwt".into(),
                refresh: "refresh".into(),
                device_id: device_id.into(),
                ca: None,
            })
            .unwrap();
        Arc::new(state)
    }

    /// 平台中立的绝对形态测试目录。
    fn abs_dir() -> String {
        if cfg!(windows) { "C:\\dst".into() } else { "/dst".into() }
    }

    /// 预置一个真实文件(start_upload/accept_orchestration 的预扫要求)。
    fn make_file() -> (tempfile::TempDir, PathBuf) {
        let dir = tempfile::tempdir().unwrap();
        let f = dir.path().join("a.bin");
        std::fs::write(&f, b"data").unwrap();
        (dir, f)
    }

    /// 模块内直构 download 作业卡(测试注入:绕过 start_download 的驱动
    /// spawn 与离线编排),经 insert_job 注册进注册表供 `wait` 长轮询。
    /// store 挂 `state.tasks()`:download 是持久化 kind,翻转/进度同步
    /// 镜像进测试 home 的 store(建卡 upsert 属 start_download,不在本
    /// 装配件——store 起初无记录,首写走 store_sync 的最小卡路径)。
    fn make_download_card(state: &Arc<DaemonState>, seq: u64) -> (Arc<JobCore>, String) {
        let id = format!("{}:{}", state.self_device_id().unwrap(), seq);
        let meta = JobMeta {
            files_total: 0,
            bytes_total: 0,
            rel_sizes: HashMap::new(),
            rels: Vec::new(),
            name: "x".into(),
            src: "srcB:/x".into(),
            dst: "/dst".into(),
        };
        let core = Arc::new(JobCore::new(
            id.clone(),
            "download",
            meta,
            state.events(),
            state.jobs().count.clone(),
            Some(state.tasks()),
        ));
        {
            let m = lock_ok(&core.m);
            core.emit_locked(&m);
        }
        let (cancel_tx, _cancel_rx) = watch::channel(false);
        state.jobs().insert_job(
            id.clone(),
            JobHandle {
                core: core.clone(),
                pause: PauseGate::new(),
                cancel_tx,
                task: Mutex::new(None),
                round: Mutex::new(None),
            },
        );
        (core, id)
    }

    /// make_download_card 的终态变体(终态装配件:active_count 计数面等
    /// 需要「入场即终态」的用例):先落终态再插入。
    fn make_terminal_card(state: &Arc<DaemonState>, seq: u64) -> String {
        let id = format!("{}:{}", state.self_device_id().unwrap(), seq);
        let meta = JobMeta {
            files_total: 0,
            bytes_total: 0,
            rel_sizes: HashMap::new(),
            rels: Vec::new(),
            name: "x".into(),
            src: "srcB:/x".into(),
            dst: "/dst".into(),
        };
        let core = Arc::new(JobCore::new(
            id.clone(),
            "download",
            meta,
            state.events(),
            state.jobs().count.clone(),
            Some(state.tasks()),
        ));
        core.finish("done", None);
        let (cancel_tx, _cancel_rx) = watch::channel(false);
        state.jobs().insert_job(
            id.clone(),
            JobHandle {
                core,
                pause: PauseGate::new(),
                cancel_tx,
                task: Mutex::new(None),
                round: Mutex::new(None),
            },
        );
        id
    }

    fn download_job() -> DownloadJob {
        DownloadJob {
            src_device: "srcB".into(),
            paths: vec!["/x".into()],
            dir: "/dst".into(),
            policy: TransferPolicy::default(),
            cut: false,
            old_rels: Vec::new(),
        }
    }

    /// 托盘计数 watch(Task 13):建队 +1、终态 -1;非终态翻转不计数,
    /// 终态粘滞(迟到翻帧)不重复扣。
    #[tokio::test]
    async fn jobs_count_watch_tracks_active() {
        let state = test_state("opA");
        let rx = state.jobs().count_rx();
        assert_eq!(*rx.borrow(), 0, "空注册表计数为 0");
        let (core, _id) = make_download_card(&state, 1);
        assert_eq!(*rx.borrow(), 1, "建队 +1");
        core.transition("running", None);
        assert_eq!(*rx.borrow(), 1, "非终态翻转不计数");
        core.transition("done", None);
        assert_eq!(*rx.borrow(), 0, "终态落定 -1");
        core.transition("failed", Some("迟到错误".into()));
        assert_eq!(*rx.borrow(), 0, "终态粘滞:迟到翻帧不重复扣");
    }

    /// M4b Task 10(D12):`active_count` = 非终态作业数——paused 计入
    /// (无独立 stalled 态,stalled 即 running),终态不计。更新器空闲门
    /// (Task 11)的数据源语义在此钉死。
    #[tokio::test]
    async fn active_count_includes_paused_excludes_terminal() {
        let state = test_state("opA");
        assert_eq!(state.jobs().active_count(), 0, "空注册表计数为 0");
        let (core1, id1) = make_download_card(&state, 1);
        let (_core2, id2) = make_download_card(&state, 2);
        let _terminal = make_terminal_card(&state, 3); // 终态入场:+1 后即 -1
        assert_eq!(state.jobs().active_count(), 2, "两张活跃卡;终态卡不计");
        assert!(state.jobs().set_paused(&id1, true), "running 卡可暂停");
        assert_eq!(core1.snap().state, "paused");
        assert_eq!(
            state.jobs().active_count(),
            2,
            "paused 非终态,必须计入活跃(D12)"
        );
        assert!(state.jobs().cancel(&id2), "取消成功");
        assert_eq!(
            state.jobs().active_count(),
            1,
            "取消终态落定 -1,只剩 paused 一张"
        );
    }

    /// 终审 I-1(发布超时尺寸感知)基线纯函数钉面:小文件恒 30s 基值
    /// (不缩水),每 10MB +1s,大 size 超时下界必超 30s 基值(多 GB 拷贝
    /// 不再被固定墙钟掐成永久「保存失败」环)。MIX 3 级 ~100-200MB/s 吞吐
    /// 的 ≥10 倍余量依据见 [`publish_timeout_for_size`]。
    #[test]
    fn publish_timeout_for_size_scales_and_keeps_base_floor() {
        assert_eq!(
            publish_timeout_for_size(0),
            PUBLISH_TIMEOUT,
            "零字节小文件走 30s 基值"
        );
        assert_eq!(
            publish_timeout_for_size(PUBLISH_TIMEOUT_PER_10MB - 1),
            PUBLISH_TIMEOUT,
            "不足 10MB 不加算"
        );
        assert_eq!(
            publish_timeout_for_size(PUBLISH_TIMEOUT_PER_10MB),
            PUBLISH_TIMEOUT + Duration::from_secs(1),
            "每 10MB 恰 +1s"
        );
        assert_eq!(
            publish_timeout_for_size(1024 * 1024 * 1024),
            PUBLISH_TIMEOUT + Duration::from_secs(102),
            "1GB ≈ +100s(+102s 精确值)"
        );
        let four_gb = publish_timeout_for_size(4 * 1024 * 1024 * 1024);
        assert_eq!(
            four_gb,
            PUBLISH_TIMEOUT + Duration::from_secs(409),
            "4GB ≈ +400s ≈ 430s(+409s 精确值)"
        );
        assert!(
            four_gb > PUBLISH_TIMEOUT,
            "大 size 超时下界必超 30s 基值(I-1 死环根除): {four_gb:?}"
        );
    }

    /// 构造一个未绑定轮的 RoundHandle(cancel watch + 新暂停门;进度回调
    /// 空实现)——bind 面单测的装配件。
    fn loose_round() -> (RoundHandle, watch::Receiver<bool>, Arc<PauseGate>) {
        let (cancel_tx, cancel_rx) = watch::channel(false);
        let gate = PauseGate::new(); // 生来即 Arc(跨任务共享门)
        let progress: Arc<dyn Fn(&RecvProgress<'_>) + Send + Sync> = Arc::new(|_| {});
        (
            RoundHandle {
                cancel: cancel_tx,
                pause: gate.clone(),
                progress,
            },
            cancel_rx,
            gate,
        )
    }

    /// 终审修复 Important#1(暂停面):暂停落在轮装配/绑定**之前**
    /// (orchestrate/打洞/重试间隙的等价物——作业级门无消费方)时,
    /// bind_incoming 必须把作业级暂停门现值种子进轮门;未暂停作业绑定
    /// 不得引入暂停。
    #[tokio::test]
    async fn bind_incoming_seeds_job_pause_into_round_gate() {
        let state = test_state("opA");
        // 暂停先于绑定
        let (core, id) = make_download_card(&state, 1);
        core.begin_running(false);
        assert!(state.jobs().set_paused(&id, true), "running 卡可暂停");
        assert_eq!(core.snap().state, "paused");
        let (handle, _cancel_rx, round_gate) = loose_round();
        assert_eq!(state.jobs().bind_incoming(&id, handle), BindOutcome::Bound);
        assert!(round_gate.paused(), "绑定时必须把作业级暂停态抄进轮门(否则新轮全新门 = 暂停静默丢失)");
        // 对照:未暂停作业绑定不引入暂停
        let (core2, id2) = make_download_card(&state, 2);
        core2.begin_running(false);
        let (handle2, _cancel_rx2, round_gate2) = loose_round();
        assert_eq!(state.jobs().bind_incoming(&id2, handle2), BindOutcome::Bound);
        assert!(!round_gate2.paused(), "未暂停作业绑定不得引入暂停");
    }

    /// 终审修复 Important#1(终态面):已取消作业的迟到绑定必须拒绝
    /// (RefusedTerminal)并驱动该轮取消 watch——取消后仍在途的
    /// ConnectRequest 不得把已取消下载的数据带落地。
    #[tokio::test]
    async fn bind_incoming_refuses_terminal_and_drives_round_cancel() {
        let state = test_state("opA");
        let (core, id) = make_download_card(&state, 3);
        core.begin_running(false);
        assert!(state.jobs().cancel(&id), "取消成功");
        assert_eq!(core.snap().state, "cancelled");
        // ConnectRequest 在取消前已发出,此刻才到 bind
        let (handle, mut cancel_rx, _gate) = loose_round();
        assert_eq!(
            state.jobs().bind_incoming(&id, handle),
            BindOutcome::RefusedTerminal,
            "终态作业不得绑定"
        );
        assert!(*cancel_rx.borrow_and_update(), "拒绝绑定时必须驱动轮取消 watch(轮死亡,数据不落地)");
        // 未命中的 id 仍是 Miss(降级匿名卡语义不变)
        let (handle2, _cancel_rx2, _gate2) = loose_round();
        assert_eq!(
            state.jobs().bind_incoming(&format!("{}:99", state.self_device_id().unwrap()), handle2),
            BindOutcome::Miss
        );
    }

    /// v2-P3 T6(bind_connect 首绑口径,upload 重试轮撞号修复):ConnectRequest
    /// 的 job_id 对 upload 传输是发送端自身裸数字 id,与本端匿名卡的裸数字
    /// 自增面撞号——首绑(bind_connect)遇 **incoming 卡(不论终态)一律
    /// Miss**(新传输走新匿名卡,重试轮不再被已终态匿名卡整体拒收);
    /// download 卡语义不变(终态 RefusedTerminal + 驱动轮取消)。同 id 的
    /// 自卡二绑(bind_incoming)不受影响(仍可绑)。
    #[tokio::test]
    async fn bind_connect_misses_incoming_but_keeps_download_semantics() {
        let state = test_state("opA");
        // 撞号构形:匿名卡铸 id "1"(与发送端 upload 作业 id 同号),轮失败
        // 落终态——重试轮的 ConnectRequest(job_id="1")到达
        let incoming_id = state
            .jobs()
            .register_incoming(&state, "srcB", std::path::Path::new("/dst"));
        assert_eq!(incoming_id, "1", "空注册表首张匿名卡 = 裸数字 1(撞号构形)");
        state.jobs().cancel(&incoming_id); // 匿名卡无驱动,取消即落终态
        assert_eq!(
            state.jobs().bind_connect(&incoming_id, loose_round().0),
            BindOutcome::Miss,
            "终态匿名卡撞号必须 Miss(走新匿名卡),不得 RefusedTerminal"
        );
        // 活跃匿名卡撞号同样 Miss(不得把新传输的轮接进旧卡)
        let live_id = state
            .jobs()
            .register_incoming(&state, "srcB", std::path::Path::new("/dst"));
        assert_eq!(
            state.jobs().bind_connect(&live_id, loose_round().0),
            BindOutcome::Miss,
            "活跃匿名卡撞号必须 Miss(新传输 = 新卡)"
        );
        // 自卡二绑不受影响:同 id 经 bind_incoming 照常占槽
        assert_eq!(
            state.jobs().bind_incoming(&live_id, loose_round().0),
            BindOutcome::Bound,
            "匿名卡的自卡接线(bind_incoming)语义不变"
        );

        // download 卡语义不变:活跃 Bound、终态 RefusedTerminal 并驱动轮取消
        let (core, dl_id) = make_download_card(&state, 1);
        core.begin_running(false);
        assert_eq!(
            state.jobs().bind_connect(&dl_id, loose_round().0),
            BindOutcome::Bound,
            "活跃 download 卡照常绑定"
        );
        assert!(state.jobs().cancel(&dl_id));
        let (handle, mut cancel_rx, _gate) = loose_round();
        assert_eq!(
            state.jobs().bind_connect(&dl_id, handle),
            BindOutcome::RefusedTerminal,
            "终态 download 卡拒收语义不变"
        );
        assert!(*cancel_rx.borrow_and_update(), "拒收时驱动轮取消 watch");
    }

    /// 评审 Critical#1:download id 必须带操作端命名空间(`<dev>:<seq>`),
    /// 且同操作端连续 mint 序号单调。
    #[tokio::test]
    async fn download_id_is_operator_namespaced() {
        let state = test_state("opA");
        let dir = abs_dir();
        let id1 = state
            .jobs()
            .start_download(&state, "srcB", vec!["/x".into()], &dir, TransferPolicy::default(), false, &[])
            .unwrap();
        let id2 = state
            .jobs()
            .start_download(&state, "srcB", vec!["/x".into()], &dir, TransferPolicy::default(), false, &[])
            .unwrap();
        assert_eq!(id1, "opA:1", "download id 必须是 `<操作端>:<序号>`: {id1}");
        assert_eq!(id2, "opA:2", "同操作端序号必须单调: {id2}");
    }

    /// 评审 Critical#1:源端接受回显后,自身自增面不得撞回显键、不得覆盖
    /// 回显条目(驱动 JoinHandle 丢失即作业孤儿化)。
    #[tokio::test]
    async fn accept_echo_then_self_mint_no_clobber() {
        let op = test_state("opA");
        let src = test_state("srcB");
        let dir = abs_dir();
        let (_keep, f) = make_file();
        let f_str = f.to_string_lossy().into_owned();

        let id = op
            .jobs()
            .start_download(&op, "srcB", vec!["/x".into()], &dir, TransferPolicy::default(), false, &[])
            .unwrap();
        assert_eq!(id, "opA:1");

        src.jobs()
            .accept_orchestration(&src, &id, "opA", std::slice::from_ref(&f_str), &dir, TransferPolicy::default(), &[])
            .await
            .unwrap();
        let minted = src
            .jobs()
            .start_upload(&src, vec![f.clone()], "peer", &dir, TransferPolicy::default(), false)
            .await
            .unwrap();

        let snaps = src.jobs().snapshot();
        let ids: Vec<&str> = snaps.iter().map(|s| s.id.as_str()).collect();
        assert_eq!(ids.len(), 2, "回显条目不得被自增条目覆盖: {ids:?}");
        assert!(ids.contains(&id.as_str()), "回显条目必须在场: {ids:?}");
        assert!(ids.contains(&minted.as_str()), "自增条目必须在场: {ids:?}");
        let echo = snaps.iter().find(|s| s.id == id).unwrap();
        assert_eq!(echo.kind, "upload", "回显条目面必须完好");
    }

    /// 评审 Critical#1:两台操作端同序号并发下载,源端必须都接受且互不
    /// 污染(命名空间隔离)。
    #[tokio::test]
    async fn accept_two_operators_same_counter() {
        let src = test_state("srcB");
        let dir = abs_dir();
        let (_keep, f) = make_file();
        let f_str = f.to_string_lossy().into_owned();

        src.jobs()
            .accept_orchestration(&src, "opA:1", "opA", std::slice::from_ref(&f_str), &dir, TransferPolicy::default(), &[])
            .await
            .unwrap();
        src.jobs()
            .accept_orchestration(&src, "opC:1", "opC", &[f_str], &dir, TransferPolicy::default(), &[])
            .await
            .unwrap();

        let snaps = src.jobs().snapshot();
        let ids: Vec<&str> = snaps.iter().map(|s| s.id.as_str()).collect();
        assert!(ids.contains(&"opA:1"), "操作端 A 的作业必须在场: {ids:?}");
        assert!(ids.contains(&"opC:1"), "操作端 C 的作业必须在场: {ids:?}");
        assert_eq!(ids.len(), 2, "同序号不同操作端不得互斥/覆盖: {ids:?}");
    }

    /// 评审 Critical#1a:纯数字外部 id 插入即前推自增计数器,后续 mint
    /// 永不与回显条目同号(否则注册表插入静默覆盖、驱动孤儿化)。
    #[tokio::test]
    async fn explicit_numeric_id_advances_counter() {
        let src = test_state("srcB");
        let dir = abs_dir();
        let (_keep, f) = make_file();
        let f_str = f.to_string_lossy().into_owned();

        src.jobs()
            .accept_orchestration(&src, "50", "opA", &[f_str], &dir, TransferPolicy::default(), &[])
            .await
            .unwrap();
        let minted = src
            .jobs()
            .start_upload(&src, vec![f], "peer", &dir, TransferPolicy::default(), false)
            .await
            .unwrap();

        assert_eq!(minted, "51", "自增面必须越过纯数字回显 id: {minted}");
        let snaps = src.jobs().snapshot();
        let ids: Vec<&str> = snaps.iter().map(|s| s.id.as_str()).collect();
        assert!(ids.contains(&"50"), "回显条目必须在场: {ids:?}");
        assert_eq!(ids.len(), 2, "不得覆盖: {ids:?}");
    }

    /// 评审 Important#2:源端 done 而本地轮已死(宽限内未收口)→ 作业必须
    /// 在有界窗口落 failed(此前 `running` 永久搁浅,wait_job 无界挂起)。
    #[tokio::test]
    async fn download_done_grace_fails_stranded_job() {
        let state = test_state("opA");
        let (core, _id) = make_download_card(&state, 1);
        let job = download_job();

        // 前两拍 running,第三拍起 done;宽限 120ms 内本地无收口 → failed
        let n = std::sync::atomic::AtomicU32::new(0);
        let mut poll = || {
            let k = n.fetch_add(1, Ordering::Relaxed);
            async move {
                if k < 2 {
                    Some(("running".to_string(), String::new(), Vec::new()))
                } else {
                    Some(("done".to_string(), String::new(), Vec::new()))
                }
            }
        };
        download_wait_terminal(
            &state,
            core.clone(),
            &job,
            Duration::from_millis(40),
            Duration::from_millis(120),
            SOURCE_POLL_FAIL_BOUND,
            &mut poll,
        )
        .await;

        let snap = core.snap();
        assert_eq!(snap.state.as_str(), "failed", "搁浅作业必须落 failed: {:?}", snap.state);
        assert!(
            snap.error.as_deref().unwrap_or_default().contains("源端已完成"),
            "error 必须说明源端已完成: {:?}",
            snap.error
        );
    }

    /// 评审 Important#2 反面:源端 done 但本地在宽限内自然收口 → done,
    /// 不得被过早判失败。
    #[tokio::test]
    async fn download_local_done_beats_source_done() {
        let state = test_state("opA");
        let (core, _id) = make_download_card(&state, 1);
        let job = download_job();

        let mut poll = || async { Some(("done".to_string(), String::new(), Vec::new())) };
        let finisher = {
            let core = core.clone();
            tokio::spawn(async move {
                tokio::time::sleep(Duration::from_millis(120)).await;
                core.finish("done", None);
            })
        };
        download_wait_terminal(
            &state,
            core.clone(),
            &job,
            Duration::from_millis(40),
            Duration::from_secs(30),
            SOURCE_POLL_FAIL_BOUND,
            &mut poll,
        )
        .await;
        finisher.await.unwrap();

        assert_eq!(core.snap().state.as_str(), "done", "宽限内自然收口必须保持 done");
    }

    /// P4a §6.1/评审焦点 2(e2e ④ 的 host 钉死面——download+收件箱哨兵
    /// 不在 host 可跑面,android cfg 分支,见 inbox_staging_e2e 拓扑注):
    /// 源端 done 且轮已 Ok 落定、进入发布窗(publishing)后,**10s 宽限
    /// 不得侵入**——注入紧宽限(60ms),三拍(>宽限)后任务必须仍
    /// publishing(既不 done——发布未落定;也不 failed——宽限只作用于
    /// 网络相);发布落定(Settled(true))后下一拍合成 done。旧病灶:无
    /// 发布门时「源端 done + 轮落定」立即合成 done(未发布先假完成);
    /// 宽限误入发布窗则三拍内假失败。
    #[tokio::test]
    async fn download_publish_window_not_killed_by_tight_grace() {
        let state = test_state("opA");
        let (core, _id) = make_download_card(&state, 1);
        let job = download_job();
        core.begin_running(false);

        // 轮 Ok 落定 + 发布开窗(原子:round_ok 与 Pending 同锁)+ 翻
        // publishing(保存中)。
        assert!(
            core.mark_round_publishing(),
            "非终态开窗必须成功"
        );
        assert_eq!(core.snap().state, "publishing", "发布窗可见态 = 保存中");

        // 驱动在独立任务:恒 done 源端 + 30ms 拍 + 60ms 紧宽限;发布窗期
        // 间不收口。
        let driver_state = state.clone();
        let driver_core = core.clone();
        let driver = tokio::spawn(async move {
            let mut poll = || async { Some(("done".to_string(), String::new(), Vec::new())) };
            download_wait_terminal(
                &driver_state,
                driver_core,
                &job,
                Duration::from_millis(30),
                Duration::from_millis(60),
                SOURCE_POLL_FAIL_BOUND,
                &mut poll,
            )
            .await;
        });
        // 三拍(≈90ms > 60ms 宽限)后仍 publishing:宽限未误杀、done 未
        // 抢跑。
        tokio::time::sleep(Duration::from_millis(100)).await;
        let snap = core.snap();
        assert_eq!(
            snap.state, "publishing",
            "发布窗内既不 done(发布未落定)也不 failed(宽限不侵入): state={}",
            snap.state
        );
        // 发布落定(协调器收尾的等价物)→ 下一拍合成 done。
        core.settle_publish(true);
        driver.await.expect("驱动任务不应 panic");
        assert_eq!(
            core.snap().state,
            "done",
            "发布落定后 done 合成放行"
        );
    }

    /// P4a T2 fix(评审 Important#1,混合重发相位切换点):相位一
    /// (republish)的 `mark_round_publishing`+`settle_publish(true)` 落定
    /// 面不得带入相位二——陈旧 `round_ok=true`+`Settled(true)` 会在「源端
    /// done 早于 B 侧轮落定」的常态序窗内把 done 合成门短路成假 done(缺
    /// 件的发布协调器因终态粘滞永不 spawn)。钉法:同构形 + 恒 done 源端
    /// + 本地无轮落定——**重置后**按宽限窗耗尽失败收口(不合成 done);
    /// **不重置**(病灶构形)同构形秒合成 done,因果对照。
    #[tokio::test]
    async fn repub_network_phase_reset_keeps_done_synthesis_gated() {
        let state = test_state("opA");
        let job = download_job();

        // 修复面:相位一落定 → 相位切换重置 → 紧宽限下源端恒 done、本地
        // 无轮落定 → done 合成门不被陈旧值放行,宽限耗尽按失败收口。
        let (core, _id) = make_download_card(&state, 1);
        core.begin_running(false);
        assert!(core.mark_round_publishing(), "相位一开窗(republish)");
        core.settle_publish(true); // 相位一发布落定
        core.reset_round_gate_for_network(); // 相位切换点(驱动同款调用)
        let mut poll = || async { Some(("done".to_string(), String::new(), Vec::new())) };
        download_wait_terminal(
            &state,
            core.clone(),
            &job,
            Duration::from_millis(30),
            Duration::from_millis(90), // 紧宽限:3-4 拍即耗尽
            SOURCE_POLL_FAIL_BOUND,
            &mut poll,
        )
        .await;
        let snap = core.snap();
        assert_eq!(
            snap.state, "failed",
            "陈旧 round_ok 不得短路 done 合成(轮未落定按宽限失败): {}",
            snap.state
        );
        assert!(
            snap.error.as_deref().unwrap_or_default().contains("宽限期"),
            "失败终因 = 宽限耗尽(本相轮未落定): {:?}",
            snap.error
        );

        // 因果对照(病灶构形,不重置):同构形下源端 done 首拍即经陈旧
        // round_ok+Settled(true) 直通 done 合成——正是修复所消灭的窗口。
        let (core2, _id2) = make_download_card(&state, 2);
        core2.begin_running(false);
        assert!(core2.mark_round_publishing());
        core2.settle_publish(true);
        let mut poll2 = || async { Some(("done".to_string(), String::new(), Vec::new())) };
        download_wait_terminal(
            &state,
            core2.clone(),
            &job,
            Duration::from_millis(30),
            Duration::from_secs(30), // 宽宽限:若门正确应等待,病灶则秒 done
            SOURCE_POLL_FAIL_BOUND,
            &mut poll2,
        )
        .await;
        assert_eq!(
            core2.snap().state,
            "done",
            "对照钉:不重置即被相位一陈旧值短路(首拍合成 done)"
        );
    }

    /// remote_copy 轮询映射(Task 8):from 端 done **即时**映射本端 done
    /// (无 download 的宽限窗语义——X 无本机数据面,from 发完 = to 已收口)。
    #[tokio::test]
    async fn remote_copy_poll_maps_source_done_immediately() {
        let state = test_state("opX");
        let id = "opX:7".to_string();
        let meta = JobMeta {
            files_total: 0,
            bytes_total: 0,
            rel_sizes: HashMap::new(),
            rels: Vec::new(),
            name: "x".into(),
            src: "srcA:/x".into(),
            dst: "dstB:/d".into(),
        };
        let core = Arc::new(JobCore::new(id.clone(), "remote_copy", meta, state.events(), state.jobs().count.clone(), None));
        {
            let m = lock_ok(&core.m);
            core.emit_locked(&m);
        }
        let (cancel_tx, _cancel_rx) = watch::channel(false);
        lock_ok(&state.jobs().map).insert(
            id.clone(),
            JobHandle {
                core: core.clone(),
                pause: PauseGate::new(),
                cancel_tx,
                task: Mutex::new(None),
                round: Mutex::new(None),
            },
        );
        let job = RemoteCopyJob {
            from: "srcA".into(),
            paths: vec!["/x".into()],
            to: "dstB".into(),
            dir: "/d".into(),
            policy: TransferPolicy::default(),
            cut: false,
            old_rels: Vec::new(),
        };

        // 前两拍 running,第三拍起 done → 首见即收口(不等第二拍)
        let n = std::sync::atomic::AtomicU32::new(0);
        let mut poll = || {
            let k = n.fetch_add(1, Ordering::Relaxed);
            async move {
                if k < 2 {
                    Some(("running".to_string(), String::new(), Vec::new()))
                } else {
                    Some(("done".to_string(), String::new(), Vec::new()))
                }
            }
        };
        remote_copy_wait_terminal(
            &state,
            core.clone(),
            &job,
            Duration::from_millis(30),
            SOURCE_POLL_FAIL_BOUND,
            &mut poll,
        )
        .await;

        let snap = core.snap();
        assert_eq!(snap.state.as_str(), "done", "from 端 done 必须即时映射: {:?}", snap.state);
        assert!(snap.error.is_none(), "成功作业无 error: {:?}", snap.error);
    }

    /// 查询持续不可达(评审 Important:None 原为无限重试——A 中途下线即永
    /// 久 running,只有用户取消能救):连续失败超过 `fail_bound` → 本端卡片
    /// 失败收口,文案可行动。download 侧。
    #[tokio::test]
    async fn download_persistent_poll_failure_fails_card() {
        let state = test_state("opA");
        let (core, _id) = make_download_card(&state, 1);
        let job = download_job();

        // 恒 None:t0 首败计时,t120(>100ms 界)失败收口
        let mut poll = || async { None::<(String, String, Vec<String>)> };
        download_wait_terminal(
            &state,
            core.clone(),
            &job,
            Duration::from_millis(40),
            Duration::from_secs(30),
            Duration::from_millis(100),
            &mut poll,
        )
        .await;

        let snap = core.snap();
        assert_eq!(snap.state.as_str(), "failed", "持续不可达必须失败收口: {:?}", snap.state);
        assert!(
            snap.error.as_deref().unwrap_or_default().contains("持续不可达"),
            "error 必须可行动(持续不可达): {:?}",
            snap.error
        );
    }

    /// 单次瞬时失败不烧计数(归零生效):None → None → 成功查询归零 →
    /// None → None → done。时序设计(bound=100ms,tick=50ms):若归零失效,
    /// 第四拍(t150)累计 ≥100ms 已翻 failed;本测断言卡片收 done,且终态
    /// 由 finisher 给出(本地自然收口先于失败界)。
    #[tokio::test]
    async fn download_transient_poll_failure_resets_counter() {
        let state = test_state("opA");
        let (core, _id) = make_download_card(&state, 1);
        let job = download_job();

        let step = std::sync::atomic::AtomicUsize::new(0);
        let mut poll = || {
            let k = step.fetch_add(1, Ordering::Relaxed);
            async move {
                match k {
                    0 | 1 => None,
                    2 => Some(("running".to_string(), String::new(), Vec::new())), // 归零点
                    3 | 4 => None,                                     // 新窗(重置已生效)
                    _ => Some(("running".to_string(), String::new(), Vec::new())),
                }
            }
        };
        // 300ms 后本地自然收口(waiter 臂出;终态粘滞使与失败界竞速无害)
        let finisher = {
            let core = core.clone();
            tokio::spawn(async move {
                tokio::time::sleep(Duration::from_millis(300)).await;
                core.finish("done", None);
            })
        };
        download_wait_terminal(
            &state,
            core.clone(),
            &job,
            Duration::from_millis(50),
            Duration::from_secs(30),
            Duration::from_millis(100),
            &mut poll,
        )
        .await;
        finisher.await.unwrap();

        assert_eq!(
            core.snap().state.as_str(),
            "done",
            "瞬时失败(任一成功归零)不得翻车: {:?}",
            core.snap().state
        );
    }

    /// 查询持续不可达 → 失败收口(同 [`download_persistent_poll_failure_
    /// fails_card`],remote_copy 侧;与 download 共用同一兜底语义)。
    #[tokio::test]
    async fn remote_copy_persistent_poll_failure_fails_card() {
        let state = test_state("opX");
        let id = "opX:9".to_string();
        let meta = JobMeta {
            files_total: 0,
            bytes_total: 0,
            rel_sizes: HashMap::new(),
            rels: Vec::new(),
            name: "x".into(),
            src: "srcA:/x".into(),
            dst: "dstB:/d".into(),
        };
        let core = Arc::new(JobCore::new(id.clone(), "remote_copy", meta, state.events(), state.jobs().count.clone(), None));
        {
            let m = lock_ok(&core.m);
            core.emit_locked(&m);
        }
        let (cancel_tx, _cancel_rx) = watch::channel(false);
        lock_ok(&state.jobs().map).insert(
            id.clone(),
            JobHandle {
                core: core.clone(),
                pause: PauseGate::new(),
                cancel_tx,
                task: Mutex::new(None),
                round: Mutex::new(None),
            },
        );
        let job = RemoteCopyJob {
            from: "srcA".into(),
            paths: vec!["/x".into()],
            to: "dstB".into(),
            dir: "/d".into(),
            policy: TransferPolicy::default(),
            cut: false,
            old_rels: Vec::new(),
        };

        // 恒 None:t120(>100ms 界)失败收口
        let mut poll = || async { None::<(String, String, Vec<String>)> };
        remote_copy_wait_terminal(
            &state,
            core.clone(),
            &job,
            Duration::from_millis(40),
            Duration::from_millis(100),
            &mut poll,
        )
        .await;

        let snap = core.snap();
        assert_eq!(snap.state.as_str(), "failed", "持续不可达必须失败收口: {:?}", snap.state);
        assert!(
            snap.error.as_deref().unwrap_or_default().contains("持续不可达"),
            "error 必须可行动(持续不可达): {:?}",
            snap.error
        );
    }

    /// 单次瞬时失败不烧计数(remote_copy 侧,归零设计同
    /// [`download_transient_poll_failure_resets_counter`]):若归零失效,
    /// t150 累计 ≥100ms 已翻 failed;本测断言卡片收 done。
    #[tokio::test]
    async fn remote_copy_transient_poll_failure_resets_counter() {
        let state = test_state("opX");
        let id = "opX:11".to_string();
        let meta = JobMeta {
            files_total: 0,
            bytes_total: 0,
            rel_sizes: HashMap::new(),
            rels: Vec::new(),
            name: "x".into(),
            src: "srcA:/x".into(),
            dst: "dstB:/d".into(),
        };
        let core = Arc::new(JobCore::new(id.clone(), "remote_copy", meta, state.events(), state.jobs().count.clone(), None));
        {
            let m = lock_ok(&core.m);
            core.emit_locked(&m);
        }
        let (cancel_tx, _cancel_rx) = watch::channel(false);
        lock_ok(&state.jobs().map).insert(
            id.clone(),
            JobHandle {
                core: core.clone(),
                pause: PauseGate::new(),
                cancel_tx,
                task: Mutex::new(None),
                round: Mutex::new(None),
            },
        );
        let job = RemoteCopyJob {
            from: "srcA".into(),
            paths: vec!["/x".into()],
            to: "dstB".into(),
            dir: "/d".into(),
            policy: TransferPolicy::default(),
            cut: false,
            old_rels: Vec::new(),
        };

        let step = std::sync::atomic::AtomicUsize::new(0);
        let mut poll = || {
            let k = step.fetch_add(1, Ordering::Relaxed);
            async move {
                match k {
                    0 | 1 => None,
                    2 => Some(("running".to_string(), String::new(), Vec::new())), // 归零点
                    3 | 4 => None,                                     // 新窗(重置已生效)
                    _ => Some(("running".to_string(), String::new(), Vec::new())),
                }
            }
        };
        // 300ms 后本地自然收口(cancel 竞速路径之外唯一出口;粘滞无害)
        let finisher = {
            let core = core.clone();
            tokio::spawn(async move {
                tokio::time::sleep(Duration::from_millis(300)).await;
                core.finish("done", None);
            })
        };
        remote_copy_wait_terminal(
            &state,
            core.clone(),
            &job,
            Duration::from_millis(50),
            Duration::from_millis(100),
            &mut poll,
        )
        .await;
        finisher.await.unwrap();

        assert_eq!(
            core.snap().state.as_str(),
            "done",
            "瞬时失败(任一成功归零)不得翻车: {:?}",
            core.snap().state
        );
    }

    /// M4a-2(毒锁恢复):持锁 panic 毒化 std Mutex 后,`lock_ok` 仍必须
    /// 取到守卫并可写(Err(poisoned) → into_inner)——锁面不再因一次毒化
    /// 对所有后续访问永久 panic。
    #[test]
    fn lock_ok_recovers_poison() {
        let m = Mutex::new(1u64);
        let _hook_guard = PANIC_HOOK_LOCK.lock().unwrap();
        let prev = std::panic::take_hook();
        std::panic::set_hook(Box::new(|_| {})); // 预期 panic,静默输出
        let poisoned = std::panic::catch_unwind(std::panic::AssertUnwindSafe(|| {
            // 原生 std 锁持守卫 panic → 毒化( arrange 不经被测函数)
            let _g = m.lock().unwrap();
            panic!("poison the lock");
        }))
        .is_err();
        std::panic::set_hook(prev);
        assert!(poisoned, "前置:锁必须已被毒化");
        let mut g = lock_ok(&m);
        *g += 1;
        assert_eq!(*g, 2, "毒锁恢复后必须可读写");
    }

    /// M4a-2(驱动监督):驱动 panic(以合成 panic future 代驱动本体)→
    /// `supervise_driver` 捕获 unwind 且作业未终态 → 落 failed(文案钉死
    /// 「内部错误: 传输驱动异常退出」);已终态再遇 panic 不得改写首次
    /// 终态(粘滞,不覆盖取消等先到终态)。
    #[tokio::test]
    // PANIC_HOOK_LOCK 守卫横跨两段 hook 换装间的 await:争用者仅另一条同步
    // #[test],且 #[tokio::test] 默认 current_thread 运行时,持 std 锁过
    // await 不会跨线程调度,无死锁面。
    #[allow(clippy::await_holding_lock)]
    async fn panicking_driver_marks_failed() {
        let state = test_state("opA");
        let meta = JobMeta {
            files_total: 0,
            bytes_total: 0,
            rel_sizes: HashMap::new(),
            rels: Vec::new(),
            name: "x".into(),
            src: "srcB:/x".into(),
            dst: "/dst".into(),
        };
        let core = Arc::new(JobCore::new(
            "sup:1".into(),
            "download",
            meta,
            state.events(),
            state.jobs().count.clone(),
            None,
        ));
        let _hook_guard = PANIC_HOOK_LOCK.lock().unwrap();
        let prev = std::panic::take_hook();
        std::panic::set_hook(Box::new(|_| {})); // 预期 panic,静默输出
        supervise_driver(core.clone(), async { panic!("boom") }).await;
        std::panic::set_hook(prev);
        let snap = core.snap();
        assert_eq!(
            snap.state.as_str(),
            "failed",
            "panic 驱动必须落 failed: {:?}",
            snap.state
        );
        assert_eq!(
            snap.error.as_deref(),
            Some("内部错误: 传输驱动异常退出"),
            "文案钉死: {:?}",
            snap.error
        );
        // 已终态:再遇 panic 不改写(终态粘滞;监督不覆盖先到终态)
        let prev = std::panic::take_hook();
        std::panic::set_hook(Box::new(|_| {}));
        supervise_driver(core.clone(), async { panic!("again") }).await;
        std::panic::set_hook(prev);
        let snap = core.snap();
        assert_eq!(snap.state.as_str(), "failed", "终态粘滞: {:?}", snap.state);
        assert_eq!(
            snap.error.as_deref(),
            Some("内部错误: 传输驱动异常退出"),
            "首次终态文案不得被改写: {:?}",
            snap.error
        );
    }

    /// v2-P2 Task 4 ①(建卡即落盘):upload 建卡点把 queued 全字段记录写进
    /// store——name=首项末段名、rels=预扫 rel 全集、src_paths/target_dir/
    /// from/to/policy/cut/计数面齐;download 建卡点同(远端数据预扫不可为
    /// ——rels/总数为空,待 Task 8 job_query 回填)。
    #[tokio::test]
    async fn card_creation_persists_full_record() {
        let state = test_state("opA");
        let dir = abs_dir();
        let (_keep, f) = make_file();
        let policy = TransferPolicy {
            skip: vec!["杂项.tmp".into()],
            overwrite: Vec::new(),
        };
        let id = state
            .jobs()
            .start_upload(&state, vec![f.clone()], "peerB", &dir, policy, true)
            .await
            .unwrap();
        let rec = state
            .tasks()
            .get(&id)
            .expect("建卡即落盘: upload 记录必须在 store");
        assert_eq!(rec.kind, "upload");
        assert_eq!(rec.name, "a.bin", "显示名 = 首项末段名: {}", rec.name);
        assert_eq!(rec.from_dev, "opA", "upload 源端 = 本机");
        assert_eq!(rec.to_dev, "peerB");
        assert_eq!(rec.src_paths, vec![f.to_string_lossy().into_owned()]);
        assert_eq!(rec.target_dir, dir);
        assert_eq!(rec.policy_skip, vec!["杂项.tmp".to_string()]);
        assert!(rec.policy_overwrite.is_empty());
        assert!(rec.cut, "cut 语义入卡(重发原料)");
        assert_eq!(rec.rels, vec!["a.bin".to_string()], "rels = 预扫 rel 全集");
        assert_eq!(rec.files_total, 1);
        assert_eq!(rec.bytes_total, 4, "make_file 载荷 b\"data\"");
        assert_eq!(rec.files_done, 0);
        assert!(rec.created_ms > 0, "创建时刻必须落盘");
        assert_eq!(rec.finished_ms, None);
        assert!(rec.error.is_none());
        assert!(
            matches!(rec.state.as_str(), "queued" | "running" | "failed"),
            "建卡落盘态(驱动并发起步推进,不钉死某一帧): {}",
            rec.state
        );

        // download:发起端卡同样入 store;src_paths = 远端路径原样
        let id2 = state
            .jobs()
            .start_download(
                &state,
                "srcB",
                vec!["/data/远端文件.bin".into()],
                &dir,
                TransferPolicy::default(),
                false,
                &[],
            )
            .unwrap();
        let rec2 = state
            .tasks()
            .get(&id2)
            .expect("建卡即落盘: download 记录必须在 store");
        assert_eq!(rec2.kind, "download");
        assert_eq!(rec2.name, "远端文件.bin", "远端路径取末段名: {}", rec2.name);
        assert_eq!(rec2.from_dev, "srcB");
        assert_eq!(rec2.to_dev, "opA", "download 目标端 = 本机");
        assert_eq!(rec2.src_paths, vec!["/data/远端文件.bin".to_string()]);
        assert_eq!(rec2.target_dir, dir);
        assert!(
            rec2.rels.is_empty(),
            "download 预扫不可为,rels 待 job_query 回填: {:?}",
            rec2.rels
        );
        assert_eq!(rec2.files_total, 0);
        assert!(!rec2.state.is_empty(), "建卡态已落盘: {}", rec2.state);
    }

    /// v2-P2 Task 4 ②(状态翻转即时落盘):非终态翻转(running/paused)与
    /// 终态落定(state/error/finished_ms)都即时镜像进 store,不等进度节流窗。
    #[tokio::test]
    async fn state_transitions_persist_immediately() {
        let state = test_state("opA");
        let (core, id) = make_download_card(&state, 1);
        assert!(
            state.tasks().get(&id).is_none(),
            "测试直构卡不经建卡点,store 此刻应无记录"
        );
        core.begin_running(false);
        let rec = state.tasks().get(&id).expect("running 翻转即时落盘");
        assert_eq!(rec.state, "running");
        assert_eq!(rec.finished_ms, None, "非终态无终态时刻");

        core.transition("paused", None);
        assert_eq!(
            state.tasks().get(&id).unwrap().state,
            "paused",
            "暂停翻转即时落盘"
        );

        core.finish("failed", Some("对端失联".into()));
        let rec = state.tasks().get(&id).expect("终态即时落盘");
        assert_eq!(rec.state, "failed");
        assert_eq!(rec.error.as_deref(), Some("对端失联"), "失败原因入卡");
        assert!(rec.finished_ms.is_some(), "终态落定时刻必须落盘");
    }

    /// v2-P2 Task 4 ③(进度节流落盘):窗口内两次 Bytes 进度只落一次
    /// (首条即写,其后 ≥5s 窗内跳过;两次 track 间隔 ≪5s);FileDone 不受
    /// 节流约束恒写(spec §4.1「≥5s 或 FileDone」——文件粒度是中断面最后
    /// 已知值的最小有意义单位)。
    #[tokio::test]
    async fn progress_store_write_throttled_filedone_flushes() {
        let state = test_state("opA");
        let (core, id) = make_download_card(&state, 1);
        core.begin_running(false);

        // 首条进度即写
        core.track_recv(&RecvProgress::Bytes {
            rel: "a.bin",
            done: 100,
            total: 1000,
        });
        assert_eq!(state.tasks().get(&id).unwrap().bytes_done, 100);

        // 窗口内第二条:节流跳过,store 仍是首条的账(两 track 间隔 ≪5s)
        core.track_recv(&RecvProgress::Bytes {
            rel: "a.bin",
            done: 200,
            total: 1000,
        });
        assert_eq!(
            state.tasks().get(&id).unwrap().bytes_done,
            100,
            "5s 窗内第二次进度不得落盘"
        );

        // FileDone 恒写:文件粒度进度立即镜像(字节以流面总量补齐)
        core.track_recv(&RecvProgress::FileDone { rel: "a.bin" });
        let rec = state.tasks().get(&id).unwrap();
        assert_eq!(rec.files_done, 1, "FileDone 不受节流约束: {rec:?}");
        assert_eq!(rec.bytes_done, 1000, "FileDone 以累计 total 补齐字节");
    }

    /// v2-P2 Task 4 ④(transfer.list 合并):持久层打底,live 快照按 id
    /// 覆盖——live 权威盖掉同 id 的陈旧持久态;store 独有行(重启中断/
    /// 历史)照常出现;name 两侧行都并入返回 JSON。
    #[tokio::test]
    async fn transfer_list_merges_store_over_live() {
        let state = test_state("opA");
        let (core, id) = make_download_card(&state, 1);
        core.begin_running(false);
        // 同 id 的陈旧持久态(state/files_done 均陈旧:必须被 live 覆盖)
        state.tasks().upsert(&crate::taskstore::TaskRec {
            id: id.clone(),
            kind: "download".into(),
            name: "陈旧名".into(),
            state: "interrupted".into(),
            files_done: 3,
            ..Default::default()
        });
        // store 独有行:live 无此作业(重启历史),照常出现
        state.tasks().upsert(&crate::taskstore::TaskRec {
            id: "legacy:7".into(),
            kind: "upload".into(),
            name: "历史任务".into(),
            state: "interrupted".into(),
            ..Default::default()
        });

        let jobs = crate::api::transfer_list(&state)["jobs"]
            .as_array()
            .expect("jobs 数组")
            .clone();
        assert_eq!(jobs.len(), 2, "恰两条(1 live 覆盖 + 1 store 独有): {jobs:?}");
        let live = jobs
            .iter()
            .find(|j| j["id"].as_str() == Some(id.as_str()))
            .expect("live 作业必须在场");
        assert_eq!(
            live["state"].as_str(),
            Some("running"),
            "live 权威覆盖陈旧持久态(interrupted): {live}"
        );
        assert_eq!(
            live["files_done"].as_u64(),
            Some(0),
            "live 进度覆盖 store 的陈旧 3: {live}"
        );
        assert!(
            live["name"].as_str().is_some(),
            "name 字段并入 live 行: {live}"
        );
        let old = jobs
            .iter()
            .find(|j| j["id"].as_str() == Some("legacy:7"))
            .expect("store 独有行必须出现在列表");
        assert_eq!(old["state"].as_str(), Some("interrupted"));
        assert_eq!(old["name"].as_str(), Some("历史任务"));
        assert_eq!(old["kind"].as_str(), Some("upload"));
    }

    /// v2-P2 Task 4 评审 Important(id 计数器播种):重启后 Jobs 自持久层
    /// 播种 next_id——上一会话已铸造的 id(裸数字 upload 型与
    /// `<dev>:<seq>` download 型)不得被新会话重用;否则建卡 upsert 按 id
    /// 静默覆盖持久历史。断言面:①新会话首个/次个铸造 id 越过全部历史
    /// seq;②上一会话的记录内容原样保留(可辨 name 未被覆盖);③新作业
    /// 以新 id 正常入 store。
    #[tokio::test]
    async fn job_id_counter_seeded_from_store_across_restart() {
        let home = tempfile::tempdir().unwrap();
        // 上一会话的磁盘事实:seq 1..3 的持久记录(裸数字与命名空间两型
        // 都含 seq 1——未播种的会话首铸恰会撞上它们并按 id 覆盖)
        {
            let store = TaskStore::load(home.path());
            store.upsert(&TaskRec {
                id: "1".into(),
                kind: "upload".into(),
                name: "上一会话裸数字".into(),
                state: "done".into(),
                ..Default::default()
            });
            store.upsert(&TaskRec {
                id: "opA:1".into(),
                kind: "download".into(),
                name: "上一会话下载".into(),
                state: "interrupted".into(),
                ..Default::default()
            });
            store.upsert(&TaskRec {
                id: "opA:3".into(),
                kind: "remote_copy".into(),
                name: "上一会话调度".into(),
                state: "failed".into(),
                ..Default::default()
            });
        } // store 弃:重启语义 = 磁盘事实进新会话,内存面不携带

        // 新会话(重启):完整装配(store 整载 + Jobs 播种)
        let state = test_state_at("opA", home.path());
        assert_eq!(state.tasks().list().len(), 3, "前置:三条持久记录在场");

        // 新会话首铸(download 命名空间型)必须越过历史最大 seq 3
        let dir = abs_dir();
        let id1 = state
            .jobs()
            .start_download(&state, "srcB", vec!["/x".into()], &dir, TransferPolicy::default(), false, &[])
            .unwrap();
        assert_eq!(id1, "opA:4", "重启后首铸 id 不得重用历史 id: {id1}");
        // 次铸(upload 裸数字型)继续单调
        let (_keep, f) = make_file();
        let id2 = state
            .jobs()
            .start_upload(&state, vec![f], "peerB", &dir, TransferPolicy::default(), false)
            .await
            .unwrap();
        assert_eq!(id2, "5", "播种后自增面继续单调: {id2}");

        // 上一会话的记录原样保留(name 可辨:若被重铸覆盖,此断言红)
        let old = state.tasks().get("1").expect("裸数字历史记录必须在场");
        assert_eq!(old.name, "上一会话裸数字", "持久历史不得被新会话覆盖: {old:?}");
        assert_eq!(old.state, "done");
        let old2 = state.tasks().get("opA:1").expect("命名空间历史记录必须在场");
        assert_eq!(old2.name, "上一会话下载", "持久历史不得被新会话覆盖: {old2:?}");

        // 新作业以新 id 正常入 store(记录面继续滚动,不撞历史)
        assert_eq!(
            state.tasks().get(&id1).expect("新作业建卡入 store").kind,
            "download"
        );
    }

    /// v2-P2 Task 6 ①(重发资格门):已完成(done)与进行中(running)记
    /// 录拒绝重新发起(queued/paused/cancelled 同门拒绝);被拒不摘记录;
    /// 不存在的 id 报「任务不存在」。
    #[tokio::test]
    async fn reinitiate_rejects_done_and_running() {
        let state = test_state("opA");
        let dir = abs_dir();
        for (id, st) in [
            ("1", "done"),
            ("2", "running"),
            ("3", "queued"),
            ("4", "paused"),
            ("5", "cancelled"),
        ] {
            state.tasks().upsert(&TaskRec {
                id: id.into(),
                kind: "upload".into(),
                name: "x".into(),
                to_dev: "peerB".into(),
                target_dir: dir.clone(),
                src_paths: vec!["/x".into()],
                state: st.into(),
                ..Default::default()
            });
        }
        for (id, st) in [
            ("1", "done"),
            ("2", "running"),
            ("3", "queued"),
            ("4", "paused"),
            ("5", "cancelled"),
        ] {
            let err = state.jobs().reinitiate(&state, id).await.unwrap_err();
            assert!(
                err.contains("不可重新发起"),
                "{st} 记录必须拒绝重发: {err}"
            );
            assert!(
                state.tasks().get(id).is_some(),
                "{st} 被拒不摘记录"
            );
        }
        let err = state.jobs().reinitiate(&state, "99").await.unwrap_err();
        assert!(err.contains("任务不存在"), "缺失 id 必须报不存在: {err}");
    }

    /// v2-P2 Task 6 ②(翻新换新 id,陈旧同号无撞):注册表预插旧 id "1"
    /// 的终态卡(显式回显 id 面的等价构形——对端/本面注册表持旧号;手构
    /// 无驱动卡钉死时序,真驱动的落败时序不定),失败记录重发必须铸造
    /// **新** id:不拒、不撞(计数器自 store 播种前推,同号再插入会静默
    /// 覆盖旧驱动句柄致作业孤儿化);旧终态条目随摘旧同步逐出(终审修复
    /// #2 的单元面,e2e `reinitiate_upload_resume_and_missing` 钉行面),
    /// 新记录以原参数翻新。
    #[tokio::test]
    async fn reinitiate_upload_survives_stale_peer_job_id() {
        // 记录先于会话落盘(cross-session 真构形):Jobs 自 store 播种计数
        // 器,翻新铸造的新 id 高于旧号;源文件真实在场(重发 partial 预检
        // 要能扫到内容)
        let home = tempfile::tempdir().unwrap();
        let src = home.path().join("a.bin");
        std::fs::write(&src, b"data").unwrap();
        {
            let store = TaskStore::load(home.path());
            store.upsert(&TaskRec {
                id: "1".into(),
                kind: "upload".into(),
                name: "a.bin".into(),
                from_dev: "opA".into(),
                to_dev: "peerB".into(),
                src_paths: vec![src.to_string_lossy().into_owned()],
                target_dir: abs_dir(),
                rels: vec!["a.bin".into()],
                state: "failed".into(),
                ..Default::default()
            });
        }
        let state = test_state_at("opA", home.path());
        let dir = abs_dir();

        // 预插旧 id "1" 进注册表:终态 failed 卡,手构无驱动(store=None,
        // 同编排回显口径)——同会话重发的真实构形即 failed live 行在场
        {
            let meta = JobMeta {
                files_total: 1,
                bytes_total: 4,
                rel_sizes: HashMap::new(),
                rels: vec!["a.bin".into()],
                name: "a.bin".into(),
                src: src.to_string_lossy().into_owned(),
                dst: format!("peerB:{dir}"),
            };
            let core = Arc::new(JobCore::new(
                "1".into(),
                "upload",
                meta,
                state.events(),
                state.jobs().count.clone(),
                None,
            ));
            core.finish("failed", None);
            let (cancel_tx, _cancel_rx) = watch::channel(false);
            state.jobs().insert_job(
                "1".into(),
                JobHandle {
                    core,
                    pause: PauseGate::new(),
                    cancel_tx,
                    task: Mutex::new(None),
                    round: Mutex::new(None),
                },
            );
        }

        let new_id = state.jobs().reinitiate(&state, "1").await.unwrap();
        assert_ne!(new_id, "1", "翻新必须换新 id: {new_id}");

        // 注册表:旧终态条目随摘旧同步逐出(行随记录走,终审修复#2),
        // 恰剩新翻新作业——mint 高于旧号,从未覆盖旧条目
        let snaps = state.jobs().snapshot();
        let ids: Vec<&str> = snaps.iter().map(|s| s.id.as_str()).collect();
        assert_eq!(
            ids,
            vec![new_id.as_str()],
            "旧终态条目已逐出,恰剩新作业: {ids:?}"
        );

        // 持久面:旧记录摘除,新记录以原参数翻新(src_paths 原样携带,
        // rels = 新预扫集)
        assert!(state.tasks().get("1").is_none(), "旧记录必须摘除");
        let rec = state.tasks().get(&new_id).expect("翻新记录必须在 store");
        assert_eq!(rec.kind, "upload");
        assert_eq!(rec.to_dev, "peerB");
        assert_eq!(rec.target_dir, dir);
        assert_eq!(
            rec.src_paths,
            vec![src.to_string_lossy().into_owned()],
            "原参数(重发原料)不缩水"
        );
        assert_eq!(rec.rels, vec!["a.bin".to_string()], "新卡 rels = 新预扫集");
    }

    /// v2-P2 Task 8(reinitiate 幂等门,评审 carry):`store.get` 返回克隆,
    /// 双双过校验门的并发重发不得各造一个并行驱动——`remove` 的命中返回值
    /// 是唯一原子裁决,败者按存在性报错。用**真线程**并发(tokio 任务协作
    /// 式调度不会在无 await 的临界区内让位,缝打不开;OS 线程任意抢占才能
    /// 真踩 get→remove 窗):两路同 id 重发,恰一路 Ok;持久面旧记录摘除、
    /// 恰一张新卡。
    #[test]
    fn reinitiate_concurrent_calls_mint_exactly_one_job() {
        // 记录先于会话落盘(cross-session 真构形):Jobs 自 store 播种计数
        // 器,胜者铸造的新 id 高于旧号(同号再插会静默覆盖、驱动孤儿化)。
        // 源文件真实在场:重发 partial 预检要能扫到内容(全缺失 = 「无可
        // 传内容」拒绝,胜者也会失败)。
        let home = tempfile::tempdir().unwrap();
        let src = home.path().join("a.bin");
        std::fs::write(&src, b"data").unwrap();
        {
            let store = TaskStore::load(home.path());
            store.upsert(&TaskRec {
                id: "1".into(),
                kind: "upload".into(),
                name: "a.bin".into(),
                from_dev: "opA".into(),
                to_dev: "peerB".into(),
                src_paths: vec![src.to_string_lossy().into_owned()],
                target_dir: abs_dir(),
                rels: vec!["a.bin".into()],
                state: "failed".into(),
                ..Default::default()
            });
        }
        let state = test_state_at("opA", home.path());

        let rt = || {
            tokio::runtime::Builder::new_current_thread()
                .enable_all()
                .build()
                .unwrap()
        };
        // 屏障对齐:两线程同时进 reinitiate,最大化踩窗概率
        let barrier = Arc::new(std::sync::Barrier::new(2));
        let (s1, s2) = (state.clone(), state.clone());
        let (bar1, bar2) = (barrier.clone(), barrier.clone());
        let t1 = std::thread::spawn(move || {
            rt().block_on(async {
                bar1.wait();
                s1.jobs().reinitiate(&s1, "1").await
            })
        });
        let t2 = std::thread::spawn(move || {
            rt().block_on(async {
                bar2.wait();
                s2.jobs().reinitiate(&s2, "1").await
            })
        });
        let (r1, r2) = (t1.join().unwrap(), t2.join().unwrap());

        match (r1, r2) {
            (Ok(a), Err(e)) | (Err(e), Ok(a)) => {
                assert!(
                    e.contains("任务不存在"),
                    "败者必须按存在性报错(已被并发重发摘走): {e}"
                );
                assert!(a != "1", "翻新必须换新 id: {a}");
                assert!(state.tasks().get("1").is_none(), "旧记录必须被胜者摘除");
                let recs = state.tasks().list();
                assert_eq!(recs.len(), 1, "持久面恰一张新卡(不得重复建卡): {recs:?}");
                assert_eq!(recs[0].id, a, "新卡 id = 胜者铸造 id");
                assert_eq!(recs[0].kind, "upload");
            }
            (Ok(a), Ok(b)) => panic!("并发重发不得双双成功(幂等门失守): {a} / {b}"),
            (Err(x), Err(y)) => panic!("并发重发不得双双失败: {x} / {y}"),
        }
    }

    /// 终审修复(Important#1):摘旧成功而重发起动失败不得吞记录。全部源
    /// 文件已消失 → partial 预检 present 空 → 「无可传内容」Err——原记录
    /// 必须原样回插(记录不自动删除,spec 裁定 6;否则记录无声蒸发,对端
    /// .mtdss 残留也失去清除入口),状态保持 failed 等用户重试。
    #[tokio::test]
    async fn reinitiate_all_sources_missing_keeps_record() {
        // 记录先于会话落盘(cross-session 真构形);源路径指向随后消失的
        // 文件(src_paths/rels 非空 = partial 预检模式,全缺失收进 missing)
        let home = tempfile::tempdir().unwrap();
        let src = home.path().join("gone.bin");
        std::fs::write(&src, b"data").unwrap();
        {
            let store = TaskStore::load(home.path());
            store.upsert(&TaskRec {
                id: "1".into(),
                kind: "upload".into(),
                name: "gone.bin".into(),
                from_dev: "opA".into(),
                to_dev: "peerB".into(),
                src_paths: vec![src.to_string_lossy().into_owned()],
                target_dir: abs_dir(),
                rels: vec!["gone.bin".into()],
                state: "failed".into(),
                ..Default::default()
            });
        }
        let state = test_state_at("opA", home.path());
        std::fs::remove_file(&src).unwrap();

        let err = state.jobs().reinitiate(&state, "1").await.unwrap_err();
        assert!(err.contains("无可传内容"), "全源缺失按既有口径拒绝: {err}");
        let rec = state
            .tasks()
            .get("1")
            .expect("起动失败的记录必须回插,不得吞掉");
        assert_eq!(rec.state, "failed", "回插记录保持原状态: {rec:?}");
        assert_eq!(
            rec.src_paths,
            vec![src.to_string_lossy().into_owned()],
            "回插记录原样(重发原料不缩水)"
        );
        assert_eq!(rec.rels, vec!["gone.bin".to_string()], "回插记录原样");
    }

    /// v2-P2 Task 8(download 分支):失败 download 记录重发 = 翻新换新 id
    /// (本端命名空间铸造)+ 原参数原样(from/to/paths/dir/policy/cut);
    /// 新卡 rels 空(待重发轮 job_query 回填,旧集已携进驱动编排面——全真
    /// 断言在 e2e `reinitiate_download_resume`);cancelled 记录仍拒(仅
    /// failed/interrupted,同 T6 门),被拒不摘记录。
    #[tokio::test]
    async fn reinitiate_download_renews_with_original_params() {
        // 记录先于会话落盘(cross-session 真构形):计数器自 store 播种,
        // 翻新铸造的新 id 高于旧号(会话内直构高序号记录会撞号回写旧 id)
        let home = tempfile::tempdir().unwrap();
        {
            let store = TaskStore::load(home.path());
            store.upsert(&TaskRec {
                id: "opA:1".into(),
                kind: "download".into(),
                name: "a.bin".into(),
                from_dev: "srcB".into(),
                to_dev: "opA".into(),
                src_paths: vec!["/data/a.bin".into(), "/data/b.bin".into()],
                target_dir: abs_dir(),
                policy_skip: vec!["杂.tmp".into()],
                cut: true,
                rels: vec!["a.bin".into(), "b.bin".into()],
                state: "failed".into(),
                ..Default::default()
            });
        }
        let state = test_state_at("opA", home.path());
        let dir = abs_dir();
        // cancelled 同门拒
        store_cancelled_download(&state, "opA:9", &dir);
        let err = state.jobs().reinitiate(&state, "opA:9").await.unwrap_err();
        assert!(err.contains("不可重新发起"), "cancelled download 必须拒: {err}");
        assert!(state.tasks().get("opA:9").is_some(), "被拒不摘记录");

        // failed 记录重发:翻新 + 原参数
        let new_id = state.jobs().reinitiate(&state, "opA:1").await.unwrap();
        assert_ne!(new_id, "opA:1", "翻新必须换新 id: {new_id}");
        assert!(
            new_id.starts_with("opA:"),
            "新 id 必须带本端命名空间(`<本机>:<序号>`): {new_id}"
        );
        assert!(state.tasks().get("opA:1").is_none(), "旧记录摘除");
        let rec = state.tasks().get(&new_id).expect("翻新记录在场");
        assert_eq!(rec.kind, "download");
        assert_eq!(rec.from_dev, "srcB", "原参数:源端");
        assert_eq!(rec.to_dev, "opA", "原参数:目标端 = 本机");
        assert_eq!(
            rec.src_paths,
            vec!["/data/a.bin".to_string(), "/data/b.bin".to_string()],
            "原参数:源路径(重发原料)不缩水"
        );
        assert_eq!(rec.target_dir, dir, "原参数:目标目录");
        assert_eq!(rec.policy_skip, vec!["杂.tmp".to_string()], "原参数:策略");
        assert!(rec.cut, "原参数:cut");
        assert!(
            rec.rels.is_empty(),
            "download 建卡 rels 空,待重发轮 job_query 回填: {:?}",
            rec.rels
        );
    }

    /// v2-P2 Task 8(remote_copy 分支):interrupted 记录重发 = 翻新换新 id
    /// + 原参数(from/to/paths/dir/cut);新卡 rels 空待回填。
    #[tokio::test]
    async fn reinitiate_remote_copy_renews_with_original_params() {
        // 记录先于会话落盘(cross-session 真构形,同 download 分支口径)
        let home = tempfile::tempdir().unwrap();
        {
            let store = TaskStore::load(home.path());
            store.upsert(&TaskRec {
                id: "opX:3".into(),
                kind: "remote_copy".into(),
                name: "a.bin".into(),
                from_dev: "srcA".into(),
                to_dev: "dstB".into(),
                src_paths: vec!["/data/a.bin".into()],
                target_dir: abs_dir(),
                cut: true,
                rels: vec!["a.bin".into()],
                state: "interrupted".into(),
                ..Default::default()
            });
        }
        let state = test_state_at("opX", home.path());
        let dir = abs_dir();
        let new_id = state.jobs().reinitiate(&state, "opX:3").await.unwrap();
        assert_ne!(new_id, "opX:3", "翻新必须换新 id: {new_id}");
        assert!(
            new_id.starts_with("opX:"),
            "新 id 必须带本端命名空间(`<本机>:<序号>`): {new_id}"
        );
        assert!(state.tasks().get("opX:3").is_none(), "旧记录摘除");
        let rec = state.tasks().get(&new_id).expect("翻新记录在场");
        assert_eq!(rec.kind, "remote_copy");
        assert_eq!(rec.from_dev, "srcA", "原参数:from");
        assert_eq!(rec.to_dev, "dstB", "原参数:to");
        assert_eq!(rec.src_paths, vec!["/data/a.bin".to_string()], "原参数:源路径");
        assert_eq!(rec.target_dir, dir, "原参数:目标目录");
        assert!(rec.cut, "原参数:cut");
        assert!(
            rec.rels.is_empty(),
            "remote_copy 建卡 rels 空,待重发轮 job_query 回填: {:?}",
            rec.rels
        );
    }

    /// P4a T2(收件箱重发分区,spec §6.3 裁定 3):republish-only 的资格门
    /// ——**cancelled 亦放行**(republish 资格=暂存记录 done,不限显示态;
    /// P2「cancelled 拒」约束网络重发);纯本地重发布全程零 ctrl 面(免
    /// 登录语义由 republish-only 分支承载,登录态下同样零对端),驱动直收
    /// 口 done;翻新记录携带 rels 全集(分区语义,后续重发可再分区)。
    #[tokio::test]
    async fn reinitiate_inbox_republish_only_allows_cancelled_and_publishes() {
        // 暂存记录先落盘:done 未 published + 终件在场(分区 staged 判据)
        let home = tempfile::tempdir().unwrap();
        put_inbox_staged(home.path(), "a.bin");
        // 记录先于会话落盘(cross-session 真构形):cancelled + 收件箱哨兵
        {
            let store = TaskStore::load(home.path());
            store.upsert(&TaskRec {
                id: "opA:1".into(),
                kind: "download".into(),
                name: "a.bin".into(),
                from_dev: "srcB".into(),
                to_dev: "opA".into(),
                src_paths: vec!["/data/a.bin".into()],
                target_dir: "inbox".into(),
                rels: vec!["a.bin".into()],
                state: "cancelled".into(),
                ..Default::default()
            });
        }
        let state = test_state_at("opA", home.path());

        // 伪 App:订阅事件面,fs.publish 到达即经 0x05 应答 ok(host 直驱,
        // 不触 fdpass——answer_publish 是查询面的直驱入口)
        let queries = state.jobs().dest_queries().clone();
        let mut evs = state.events().subscribe();
        let answerer = tokio::spawn(async move {
            while let Ok(ev) = evs.recv().await {
                if ev.method == EV_PUBLISH {
                    let id = ev.params["request_id"].as_u64().unwrap();
                    assert!(
                        queries.answer_publish(id, true, None),
                        "0x05 应答必须被受理(等待方在册)"
                    );
                }
            }
        });

        // cancelled + republish-only → 放行(评审钉死的资格门语义)
        let new_id = state.jobs().reinitiate(&state, "opA:1").await.unwrap();
        assert_ne!(new_id, "opA:1", "翻新必须换新 id: {new_id}");
        assert!(new_id.starts_with("opA:"), "命名空间沿 download 口径: {new_id}");
        assert!(state.tasks().get("opA:1").is_none(), "旧记录摘除");
        let rec = state.tasks().get(&new_id).expect("翻新记录在场");
        assert_eq!(rec.kind, "download");
        assert_eq!(rec.target_dir, "inbox", "原参数:收件箱哨兵");
        assert_eq!(rec.rels, vec!["a.bin".to_string()], "rels 全集携带(分区语义)");

        // 纯本地重发布到 done(零网络相:srcB 不存在且无信令,网络相若被
        // 误起,任务只可能 failed——done 即纯本地的证据)
        let fin = state.jobs().wait(&new_id).await.expect("等终态");
        assert_eq!(fin.state, "done", "republish-only 驱动直接收口: {:?}", fin.state);
        assert_eq!(fin.bytes_done, 0, "零网络字节");

        // 发布落定面:暂存终件删除 + 记录打 published(settle_published)
        let staging = home.path().join(crate::inbox_dest::STAGING_DIR);
        assert!(!staging.join("a.bin").exists(), "发布后暂存终件删除");
        let rec_path = staging
            .join(mtdss_core::transport::intake::INTAKE_DIR)
            .join(format!(
                "{}.json",
                mtdss_core::transport::intake::record_key("a.bin")
            ));
        let rec_json: serde_json::Value =
            serde_json::from_str(&std::fs::read_to_string(rec_path).unwrap()).unwrap();
        assert_eq!(
            rec_json["published"].as_bool(),
            Some(true),
            "记录打 published(Skip 锚,§6.2): {rec_json}"
        );
        answerer.abort();
    }

    /// P4a T2(分区资格门的反面):混合集(staged+missing)下 cancelled 仍
    /// 拒(网络相不得因发布分流而复活,P2 门);failed 正常翻新。
    #[tokio::test]
    async fn reinitiate_inbox_mixed_still_rejects_cancelled() {
        let home = tempfile::tempdir().unwrap();
        put_inbox_staged(home.path(), "a.bin"); // staged
        // "b.bin" 无任何记录 → missing
        {
            let store = TaskStore::load(home.path());
            for (id, st) in [("opA:1", "cancelled"), ("opA:2", "failed")] {
                store.upsert(&TaskRec {
                    id: id.into(),
                    kind: "download".into(),
                    name: "x".into(),
                    from_dev: "srcB".into(),
                    to_dev: "opA".into(),
                    src_paths: vec!["/data/a.bin".into(), "/data/b.bin".into()],
                    target_dir: "inbox".into(),
                    rels: vec!["a.bin".into(), "b.bin".into()],
                    state: st.into(),
                    ..Default::default()
                });
            }
        }
        let state = test_state_at("opA", home.path());

        // cancelled + 混合集 → 拒(网络相在,P2 门把关),被拒不摘记录
        let err = state.jobs().reinitiate(&state, "opA:1").await.unwrap_err();
        assert!(
            err.contains("不可重新发起"),
            "cancelled + 混合集必须拒(网络相不得复活): {err}"
        );
        assert!(state.tasks().get("opA:1").is_some(), "被拒不摘记录");

        // failed + 混合集 → 翻新(republish 相 + 网络相同一新 job);记录
        // rels 全集携带。网络相(srcB 无信令)异步自败,不在此等待终态。
        let new_id = state.jobs().reinitiate(&state, "opA:2").await.unwrap();
        assert_ne!(new_id, "opA:2");
        assert!(state.tasks().get("opA:2").is_none(), "旧记录摘除");
        let rec = state.tasks().get(&new_id).expect("翻新记录在场");
        assert_eq!(
            rec.rels,
            vec!["a.bin".to_string(), "b.bin".to_string()],
            "rels 全集携带(后续重发可再分区)"
        );
    }

    /// 落一份收件箱暂存记录 + 终件(重发分区单测的构形件:done 未
    /// published、终件在场 = staged 判据)。字节面与 intake FileRecord 一致。
    fn put_inbox_staged(home: &std::path::Path, rel: &str) {
        let staging = home.join(crate::inbox_dest::STAGING_DIR);
        std::fs::create_dir_all(staging.join(mtdss_core::transport::intake::INTAKE_DIR)).unwrap();
        let v = serde_json::json!({
            "rel": rel,
            "size": 8,
            "sha256": format!("{:064}", 0),
            "chunk_count": 1,
            "bitmap": "01",
            "done": true,
            "final_rel": rel,
            "published": false,
        });
        let key = mtdss_core::transport::intake::record_key(rel);
        std::fs::write(
            staging
                .join(mtdss_core::transport::intake::INTAKE_DIR)
                .join(format!("{key}.json")),
            serde_json::to_vec(&v).unwrap(),
        )
        .unwrap();
        std::fs::write(staging.join(rel), b"staged!!").unwrap();
    }

    /// 直构一张 cancelled download 记录(reinitiate 拒门的构形件)。
    fn store_cancelled_download(state: &Arc<DaemonState>, id: &str, dir: &str) {
        state.tasks().upsert(&TaskRec {
            id: id.into(),
            kind: "download".into(),
            name: "x".into(),
            from_dev: "srcB".into(),
            to_dev: "opA".into(),
            src_paths: vec!["/x".into()],
            target_dir: dir.into(),
            state: "cancelled".into(),
            ..Default::default()
        });
    }

    /// v2-P2 Task 8(rels 回填,spec §4.1):job_query 轮询回带 rels 非空 →
    /// 写回 store 记录(重发 diff 的「全集」原料);其后空集稳态不冲掉已
    /// 回填值;终态收口的 store_sync 读改写保留回填面。
    #[tokio::test]
    async fn download_poll_backfills_rels_to_store() {
        let state = test_state("opA");
        let (core, id) = make_download_card(&state, 1);
        core.begin_running(false);
        // 建卡(生产路径 start_download 建卡;此处直构等价记录面——直构卡
        // 不经建卡点,store 起初无记录,回填面读改写需记录在场)
        state.tasks().upsert(&TaskRec {
            id: id.clone(),
            kind: "download".into(),
            name: "x".into(),
            state: "running".into(),
            ..Default::default()
        });

        // 首拍回填非空 rels,其后空集(稳态)
        let step = std::sync::atomic::AtomicU32::new(0);
        let mut poll = || {
            let k = step.fetch_add(1, Ordering::Relaxed);
            async move {
                if k == 0 {
                    Some((
                        "running".to_string(),
                        String::new(),
                        vec!["a.bin".to_string(), "d/b.bin".to_string()],
                    ))
                } else {
                    Some(("running".to_string(), String::new(), Vec::new()))
                }
            }
        };
        // 150ms 后本地自然收口(waiter 臂;终态 store_sync 保留回填面)
        let finisher = {
            let core = core.clone();
            tokio::spawn(async move {
                tokio::time::sleep(Duration::from_millis(150)).await;
                core.finish("done", None);
            })
        };
        download_wait_terminal(
            &state,
            core,
            &download_job(),
            Duration::from_millis(40),
            Duration::from_secs(30),
            SOURCE_POLL_FAIL_BOUND,
            &mut poll,
        )
        .await;
        finisher.await.unwrap();

        let rec = state.tasks().get(&id).expect("记录在场");
        assert_eq!(
            rec.rels,
            vec!["a.bin".to_string(), "d/b.bin".to_string()],
            "轮询回带非空 rels 必须回填 store: {:?}",
            rec.rels
        );
        assert_eq!(rec.state, "done", "终态收口不受回填影响");
    }

    /// v2-P2 Task 7 ①(download 分支,本地残留面):download 记录的清除本地
    /// 清目标目录 `.mtdss` 的记录与 `.part`(恒可执行,不依赖对端——单测台
    /// 无对端 devices,若误走对端臂会得 PeerOffline 红灯),记录随删。
    #[tokio::test]
    async fn clear_download_cleans_local_intake_and_removes_record() {
        use mtdss_core::transport::intake::{INTAKE_DIR, part_path, record_path};
        let state = test_state("opA");
        let out = tempfile::tempdir().unwrap();
        // 手工放一份身份吻合的接收记录 + `.part`(字段面与 FileRecord 一致,
        // 身份终审 rec.rel == rel 才会删)
        let rel = "docs/a.bin";
        let rec = serde_json::json!({
            "rel": rel,
            "size": 1,
            "sha256": format!("{:064}", 0),
            "chunk_count": 1,
            "bitmap": "00",
            "done": false,
            "final_rel": serde_json::Value::Null,
        });
        std::fs::create_dir_all(out.path().join(INTAKE_DIR)).unwrap();
        std::fs::write(record_path(out.path(), rel), serde_json::to_vec(&rec).unwrap()).unwrap();
        std::fs::write(part_path(out.path(), rel), b"junk").unwrap();

        state.tasks().upsert(&TaskRec {
            id: "opA:1".into(),
            kind: "download".into(),
            name: "a.bin".into(),
            from_dev: "srcB".into(),
            to_dev: "opA".into(),
            target_dir: out.path().to_string_lossy().into_owned(),
            rels: vec![rel.into()],
            state: "interrupted".into(),
            ..Default::default()
        });

        state.jobs().clear(&state, "opA:1").await.unwrap();
        assert!(!record_path(out.path(), rel).exists(), "记录须被本地清");
        assert!(!part_path(out.path(), rel).exists(), ".part 须被本地清");
        assert!(state.tasks().get("opA:1").is_none(), "记录须随清摘除");
    }

    /// v2-P2 Task 7 ②(upload 离线):对端不在 devices 快照(单测台无在线
    /// 面)→ PeerOffline,记录保留可重试。
    #[tokio::test]
    async fn clear_upload_offline_keeps_record() {
        let state = test_state("opA");
        state.tasks().upsert(&TaskRec {
            id: "1".into(),
            kind: "upload".into(),
            name: "a.bin".into(),
            to_dev: "peerB".into(),
            target_dir: abs_dir(),
            rels: vec!["a.bin".into()],
            state: "failed".into(),
            ..Default::default()
        });
        let err = state.jobs().clear(&state, "1").await.unwrap_err();
        assert!(
            matches!(err, ClearErr::PeerOffline(_)),
            "离线必须 PeerOffline: {err:?}"
        );
        assert!(state.tasks().get("1").is_some(), "离线失败记录必须保留");
    }

    /// v2-P2 Task 7 ③(门面):done 拒清(无可清除面,不删任何东西——对端
    /// done 记录是后续 SKIP 素材,spec §5.2 明文不动);缺失 id 报不存在;
    /// rels 为空直接删记录(无可清面短路在 ctrl 之前,离线对端也放行)。
    #[tokio::test]
    async fn clear_gates_done_missing_and_empty_rels() {
        let state = test_state("opA");
        state.tasks().upsert(&TaskRec {
            id: "1".into(),
            kind: "upload".into(),
            name: "a.bin".into(),
            to_dev: "peerB".into(),
            target_dir: abs_dir(),
            rels: vec!["a.bin".into()],
            state: "done".into(),
            ..Default::default()
        });
        let err = state.jobs().clear(&state, "1").await.unwrap_err();
        let ClearErr::Other(m) = &err else {
            panic!("done 拒清是业务失败: {err:?}")
        };
        assert!(m.contains("无可清除面"), "done 文案钉死口径: {m}");
        assert!(state.tasks().get("1").is_some(), "done 拒清不删任何东西");

        let err = state.jobs().clear(&state, "99").await.unwrap_err();
        assert!(
            matches!(&err, ClearErr::Other(m) if m.contains("任务不存在")),
            "缺失 id 报不存在: {err:?}"
        );

        // rels 为空:直接删记录,不触网络(离线对端不构成障碍)
        state.tasks().upsert(&TaskRec {
            id: "2".into(),
            kind: "upload".into(),
            name: "b.bin".into(),
            to_dev: "peerB".into(),
            target_dir: abs_dir(),
            rels: Vec::new(),
            state: "cancelled".into(),
            ..Default::default()
        });
        state.jobs().clear(&state, "2").await.unwrap();
        assert!(state.tasks().get("2").is_none(), "无可清面直接删记录");
    }

    /// v2-P2 Task 7 ④(live 态门,评审 Important 修复):queued/running/
    /// paused 一律拒清(进行中清除会搅在途传输面,且 store.remove 后驱动
    /// 回写经 store_sync 重建记录——Ok 而清除效果不持久);P4a 增
    /// publishing(发布窗:清除会搅协调器的暂存件/记录簿记)。门的位置
    /// 钉死:在空 rels 短路**之前**——rels 为空的 download 记录(当期建卡
    /// 恒空,门放短路之后会被绕过)同样被拒。被拒记录保留,取消或等待
    /// 终态后再清。
    #[tokio::test]
    async fn clear_rejects_live_states_before_empty_rels_shortcircuit() {
        let state = test_state("opA");
        for (id, st) in [("1", "queued"), ("2", "running"), ("3", "paused"), ("4", "publishing")] {
            state.tasks().upsert(&TaskRec {
                id: id.into(),
                kind: "download".into(), // 门在分臂之前,kind 无碍
                name: "x".into(),
                to_dev: "peerB".into(),
                target_dir: abs_dir(),
                rels: Vec::new(), // 空 rels:若门位置错误,此例会走短路误 Ok
                state: st.into(),
                ..Default::default()
            });
            let err = state.jobs().clear(&state, id).await.unwrap_err();
            assert!(
                matches!(&err, ClearErr::Other(m) if m.contains("任务进行中")),
                "{st} 记录必须拒清: {err:?}"
            );
            assert!(
                state.tasks().get(id).is_some(),
                "{st} 被拒后记录必须保留"
            );
        }
    }
}

/// 授权根守卫的发送面测试(v2-P5 T2,spec §3):send 源读取路径的收口
/// 矩阵——集内放行(直达登录检查)、集外/`..` 逃逸拒绝(先于登录检查)。
/// 独立 mod:与主 tests 的已登录态反着来,未登录态恰好把「守卫放行」的
/// 断言钉在登录检查的业务文案上,无需真网络面。
#[cfg(test)]
mod roots_guard_tests {
    use super::*;
    use std::path::PathBuf;

    /// 守卫测试态:独立临时 home,授权根 = `{home}/inbox`(iOS 形态的
    /// 收件箱注入同款);返回 (state, TempDir 保活)。未登录——守卫放行
    /// 的路径会落到「未登录」业务错,拒绝的路径落到守卫文案。
    async fn state_with_inbox_root() -> (Arc<DaemonState>, tempfile::TempDir) {
        let home = tempfile::tempdir().unwrap();
        std::fs::create_dir_all(home.path().join("inbox")).unwrap();
        let (events, _) = tokio::sync::broadcast::channel(16);
        #[cfg(windows)]
        let update = std::sync::Arc::new(crate::update::UpdateService::new(
            crate::update::UpdateConfig::default(),
            events.clone(),
            tokio::sync::watch::channel(false).0,
        ));
        #[cfg(not(windows))]
        let update = ();
        let state = DaemonState::load(
            home.path(),
            home.path().join("inbox"),
            Some(vec![home.path().join("inbox")]),
            Default::default(),
            Default::default(),
            events,
            update,
        )
        .unwrap();
        (Arc::new(state), home)
    }

    /// 集内源放行:守卫直过,落到既有「未登录」业务检查(守卫若误拒,
    /// 文案不会是未登录,本例红)。
    #[tokio::test]
    async fn send_source_inside_root_reaches_login_check() {
        let (state, home) = state_with_inbox_root().await;
        let src = home.path().join("inbox").join("ok.txt");
        std::fs::write(&src, b"x").unwrap();
        let err = state
            .jobs()
            .start_upload(
                &state,
                vec![src],
                "dev-x",
                "/tmp/dest-x",
                Default::default(),
                false,
            )
            .await
            .unwrap_err();
        assert!(err.contains("未登录"), "集内源应过守卫直达登录检查: {err}");
    }

    /// 集外源拒绝:守卫先于登录检查收口(授权集形态下,集外路径即使未
    /// 登录也不泄露校验次序信息——先拒路径)。
    #[tokio::test]
    async fn send_source_outside_root_rejected() {
        let (state, home) = state_with_inbox_root().await;
        let src = home.path().join("evil.txt");
        std::fs::write(&src, b"x").unwrap();
        let err = state
            .jobs()
            .start_upload(
                &state,
                vec![src],
                "dev-x",
                "/tmp/dest-x",
                Default::default(),
                false,
            )
            .await
            .unwrap_err();
        assert!(err.contains("授权"), "集外源应被守卫拒绝: {err}");
    }

    /// `..` 逃逸拒绝:形似集内(`{root}/../evil`)的路径词法规范化后落
    /// 集外,必拒(spec §3 手法:规范化 + 前缀判定,拒绝集外)。
    #[tokio::test]
    async fn send_source_dotdot_escape_rejected() {
        let (state, home) = state_with_inbox_root().await;
        let src: PathBuf = home.path().join("inbox").join("../evil.txt");
        std::fs::write(home.path().join("evil.txt"), b"x").unwrap();
        let err = state
            .jobs()
            .start_upload(
                &state,
                vec![src],
                "dev-x",
                "/tmp/dest-x",
                Default::default(),
                false,
            )
            .await
            .unwrap_err();
        assert!(err.contains("授权"), "`..` 逃逸应被守卫拒绝: {err}");
    }
}
