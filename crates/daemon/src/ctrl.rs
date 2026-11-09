//! 控制通道(发起端连接池 + 响应端常驻循环,Task 4/M3b)。
//!
//! 远程浏览与操作(list/stat/delete/move/copy)走与传输面同一套信令枢纽
//! 与打洞原语,只是消息语义换成 CtrlConnect/CtrlAccept(与
//! ConnectRequest/ConnectAccept 平行的变体,常驻接收按变体过滤,互不
//! 串扰)与会话内 QUIC 请求-响应
//! (`transport::ctrl::send_request/serve_conn`,每请求一条独立双向流)。
//!
//! 发起端([`CtrlPool`]):本机请求短路 core fs 直调(fs 调用一律经
//! [`blocking_fs`] 落阻塞线程池——std-fs 的根卷枚举/整树复制/回收删除是
//! 毫秒~秒级阻塞调用,直跑会占死异步 worker,M3b 携带债随本任务清偿);
//! 目录快照离线预检先于任何打洞(快速失败 code 3);per-device 单飞锁让
//! 并发首查不双打洞(不同设备互不阻塞);连接缓存按 `conn.closed()` 惰性
//! 驱逐(15s keep-alive 保活直连,[`ctrl_default_tuning`] 60s 空闲上界
//! 治理静默死链),请求
//! 级失败一律驱逐后整程重试恰一次;登出时 [`CtrlPool::reset`] 清池拆卸
//! (端点随条目 drop,连接一并关闭)。端点随连接一并入池——quinn 端点
//! drop 会终止其驱动并连带关闭其上的连接。
//!
//! 响应端([`responder_loop`]):常驻消费枢纽的 CtrlConnect 定向通道
//! (随代际安装即存在,取件晚于上线的窗口里来件在缓冲排队),每条
//! CtrlConnect(按 sid 去重,重发噪声不重复起会话)spawn 一个会话任务
//! (多会话并发互不拖累);会话按对端来的方向天然定为响应侧,编舞按
//! brief 钉的顺序:CtrlAccept → 双方各自建端点 → 互发 Candidates →
//! punch → serve_conn。请求分发([`handle_ctrl_request`],async handler)
//! 把 List/Stat/Delete/Move/Copy 映射 core fs(fs 调用一律 spawn_blocking
//! 见上),version 不符回 UNSUPPORTED_VERSION,FsError 原码透传(截断
//! 标志原样携带,批量逐条失败表随响应携带);CleanupOutbox(v2-P2
//! spec §5.3)在 dir 同口径校验后跑 `intake::cleanup_rels` 清对端隐藏区。
//!
//! v2-P1c 第二刀:两处打洞点位([`CtrlPool::dial_session`] 发起方与
//! [`respond_session`] 响应方)的 punch 段均接入 connect-with-fallback
//! (与传输面同参同错误映射)——直连穷尽且中继资格在手时自动走服务端
//! 中继,ctrl 通道全 RPC 面随 [`CtrlPool::request`] 获得兜底;V3-5 起同一
//! 发起方编舞收敛于 [`CtrlPool::dial_session`] 并带 direct-only 变体
//! ([`punch_selector`]):预热路径只直连,打不通即失败,不烧服务端中继
//! (spec V3-5 §3.1(4))。

use crate::fallback::{self, FallbackErr, FallbackRole, RelayPurpose};
use crate::flow;
use crate::hub::wait_signal;
use crate::jobs::check_policy_limits;
use crate::state::DaemonState;
use ed25519_dalek::VerifyingKey;
use mtdss_core::signaling::envelope::{seal, SignalBody};
use mtdss_core::transport::ctrl::{send_request, serve_conn};
use mtdss_core::transport::punch;
use mtdss_proto::ctrl::{CtrlRequest, CtrlResponse};
use std::collections::{HashMap, HashSet};
use std::path::{Path, PathBuf};
use std::sync::{Arc, Mutex};
use std::time::Duration;

/// ctrl 直连默认调优(V3-5 调优值化):15s QUIC keep-alive PING 保池内
/// 空闲连接长活(每次冷浏览不再重打洞),60s 空闲上界降格为纯死链检测
/// (心跳在喂,正常永不触达),MTU 1452 直连安全值。`pub` 供 lib 装配与
/// flow 的 env 推导共用;数值可经 env 旋钮族实机调(`MTDSS_TEST_CTRL_*`,
/// 见 flow)。
pub const fn ctrl_default_tuning() -> mtdss_core::transport::endpoint::TransportTuning {
    mtdss_core::transport::endpoint::TransportTuning {
        keep_alive_interval: Some(Duration::from_secs(15)),
        max_idle_timeout: Duration::from_secs(60),
        max_mtu: 1452,
    }
}

/// ctrl 中继路径调优:绝不心跳保活(keep-alive None——中继会话吃服务器
/// 配额,必须 60s 空闲自灭,spec V3-5 §3.1(5)),MTU 1400 中继线路界。
/// fallback 的 relay_dial 以本值整份覆写互拨调优(端点+拨号双保险)。
pub const fn ctrl_relay_tuning() -> mtdss_core::transport::endpoint::TransportTuning {
    mtdss_core::transport::endpoint::TransportTuning {
        keep_alive_interval: None,
        max_idle_timeout: Duration::from_secs(60),
        max_mtu: 1400,
    }
}

/// 发起端单次 ctrl 请求-响应往返上界(打洞已另算;覆盖慢盘大目录枚举的
/// 合理余量,对端死链靠 QUIC 连接关闭/超时显形)。
const CTRL_SEND_TIMEOUT: Duration = Duration::from_secs(15);

/// CtrlAccept 单次等待窗口:CtrlConnect 本身已由枢纽定向通道保帧(随代际
/// 安装即存在,不存在「订阅前丢弃」窗),但响应端会话任务仍可能有启动
/// 开销(身份加载/目录缓存未命中),Accept 晚到时发起端短窗重发同一
/// CtrlConnect 兜底——任一次等到即过,多余会话随 ctrl 调优空闲上界
/// 自灭(见 [`respond_session`] 注)。
const CTRL_ACCEPT_WINDOW: Duration = Duration::from_millis(2500);
/// CtrlAccept 重发上限(同一会话 id;总预算 3×2.5s ≈ 8s,与请求级 15s
/// 上界相加仍满足被杀对端场景 ≤30s 的有界失败约束;第二刀兜底接入后整程
/// 最坏上界再叠直连 10s + 分配 10s + 互拨 10s,值均不动)。
const CTRL_ACCEPT_ATTEMPTS: u32 = 3;
/// CtrlAccept 重发间隔(窗口之间的小歇,给响应端留建端点的时间)。
const CTRL_ACCEPT_RETRY_PACE: Duration = Duration::from_millis(400);

/// ctrl 打洞参数(与传输面同规格:budget 10s / 握手 2s,调优按调用面:
/// 发起端取池字段,响应端取响应方池)。
fn ctrl_punch(tuning: mtdss_core::transport::endpoint::TransportTuning) -> punch::PunchConfig {
    punch::PunchConfig {
        budget: Duration::from_secs(10),
        handshake_timeout: Duration::from_secs(2),
        tuning,
    }
}

/// 控制通道错误。RPC 面的码位映射见 [`CtrlErr::rpc`](对端 FsError 的
/// 「远端: 」前缀也收口在那,本枚举只存原文)。
#[derive(Debug)]
pub(crate) enum CtrlErr {
    /// 对端离线(目录快照 online=false/无快照)→ RPC code 3。
    PeerOffline(String),
    /// 打洞失败(punch_err 诊断全文,含本侧 NAT 标签)→ code 1。
    Punch(String),
    /// 对端 FsError(CtrlError 透传;code 即 fs 稳定码)→ code 1。
    Peer { code: String, message: String },
    /// 信令等待/请求往返超时 → code 1。
    Timeout(String),
    /// 其余本地错误 → code 1。
    Internal(String),
}

impl CtrlErr {
    /// RPC 映射(占位契约,Task 5 沿用):离线 code 3;其余 code 1;对端
    /// 错误格式「远端: {code} {message}」,本地错误诊断全文透传。
    pub(crate) fn rpc(&self) -> (i64, String) {
        const RPC_PEER_OFFLINE: i64 = 3;
        match self {
            CtrlErr::PeerOffline(m) => (RPC_PEER_OFFLINE, m.clone()),
            CtrlErr::Peer { code, message } => (1, format!("远端: {code} {message}")),
            CtrlErr::Punch(m) | CtrlErr::Timeout(m) | CtrlErr::Internal(m) => (1, m.clone()),
        }
    }
}

// ---------- 发起端连接池 ----------

/// 本机保留别名(api `list`/`stat`/`delete` 的 `device` 参数取此值即视为
/// 本机,无需会话):未登录时本机 device_id 不存在(state.self_device_id
/// 为 None),iOS 未登录收件箱浏览(T13b 修#4)以 `device="self"` 走本机
/// 短路。真实 device_id 是服务器侧 SHA-256 十六进制前缀(32 位十六进制),
/// 与保留字无撞面;已登录时真实 id 短路语义不变。仅浏览/本机删三臂收口
/// (move/copy/orchestrate 留真实 id 面——iOS 本地浏览动作集只有查看与删)。
pub(crate) const SELF_DEVICE_ALIAS: &str = "self";

/// 本机判定:保留别名或已登录的真实 device_id。
fn is_self(state: &DaemonState, device: &str) -> bool {
    device == SELF_DEVICE_ALIAS || state.self_device_id().as_deref() == Some(device)
}

/// 互拨段形态(V3-5 direct-only 变体的分流判定):[`CtrlPool::dial_session`]
/// 按调用场景在「connect-with-fallback」与「纯直连」之间二选一。
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum PunchMode {
    /// connect_with_fallback(现状语义:直连穷尽且中继资格在手走服务端
    /// 中继;请求路径)。
    WithFallback,
    /// punch_connect 纯直连,失败即失败——预热 direct-only 变体(spec
    /// V3-5 §3.1(4):warm 打不通即失败进冷却,不烧服务端中继)。
    DirectOnly,
}

/// 互拨段选择器(纯函数,单测 [`tests::punch_selector_pins_both_arms`] 钉
/// 两臂):allow_relay=true → WithFallback(请求路径行为不变),false →
/// DirectOnly(预热路径绝不烧服务端中继)。
fn punch_selector(allow_relay: bool) -> PunchMode {
    if allow_relay {
        PunchMode::WithFallback
    } else {
        PunchMode::DirectOnly
    }
}

/// 池条目:连接 + 保活锚。quinn 端点 drop 会终止端点驱动并连带关闭其上
/// 连接,端点必须随连接一并存活(只随条目入池/出池,永不单独读取)。
#[derive(Clone)]
struct PooledConn {
    _keep: quinn::Endpoint,
    conn: quinn::Connection,
}

/// 发起端连接池:per-device 单飞锁 + 连接缓存 + 本池 ctrl 传输调优。
pub(crate) struct CtrlPool {
    /// per-device 单飞锁(打洞全程串行:并发首查不双打洞;不同设备互不
    /// 阻塞——锁 Arc 在 std 锁内取出,await 期间绝不持 std 锁)。
    locks: Mutex<HashMap<String, Arc<tokio::sync::Mutex<()>>>>,
    /// per-device 连接缓存(端点随连接一并保活,见 [`PooledConn`])。
    conns: Mutex<HashMap<String, PooledConn>>,
    /// ctrl 直连调优(V3-5 值化):默认 [`ctrl_default_tuning`](15s 心跳),
    /// DaemonOpts.ctrl_tuning 可注入改值(测试);打洞端点与互拨 PunchConfig
    /// 均取此值,中继兜底段由 fallback 整份换 [`ctrl_relay_tuning`]。
    tuning: mtdss_core::transport::endpoint::TransportTuning,
}

impl CtrlPool {
    pub(crate) fn new(tuning: mtdss_core::transport::endpoint::TransportTuning) -> Self {
        Self {
            locks: Mutex::new(HashMap::new()),
            conns: Mutex::new(HashMap::new()),
            tuning,
        }
    }

    /// 本池 ctrl 直连调优(dial_session 端点/互拨与响应端 respond_session
    /// 同取;Copy 值语义,随取随用)。
    pub(crate) fn tuning(&self) -> mtdss_core::transport::endpoint::TransportTuning {
        self.tuning
    }

    /// 远端列表(本机短路直调,fs 调用落阻塞线程池;离线快错;池复用/
    /// 打洞见 [`CtrlPool::request`])。
    pub(crate) async fn list(
        &self,
        state: &Arc<DaemonState>,
        device: &str,
        path: &str,
    ) -> Result<mtdss_core::fs::ListOut, CtrlErr> {
        use mtdss_proto::ctrl::ctrl_request::Body as Req;
        use mtdss_proto::ctrl::ctrl_response::Body as Resp;
        // device == 本机(别名 "self" 或本机 device_id)→ core fs 直调
        // (短路,不建连接;别名不要求会话——未登录本地浏览面)
        if is_self(state, device) {
            let path = path.to_string();
            return blocking_fs(move || mtdss_core::fs::list_dir(&path))
                .await
                .map_err(local_fs_err);
        }
        let req = CtrlRequest {
            version: mtdss_proto::CTRL_VERSION,
            body: Some(Req::List(mtdss_proto::ctrl::ListRequest {
                path: path.to_string(),
            })),
        };
        let resp = self.request(state, device, req).await?;
        match resp.body {
            Some(Resp::List(r)) => Ok(mtdss_core::fs::ListOut {
                entries: r.entries.iter().map(proto_dir_entry).collect(),
                truncated: r.truncated,
            }),
            Some(Resp::Error(e)) => Err(CtrlErr::Peer {
                code: e.code,
                message: e.message,
            }),
            _ => Err(CtrlErr::Internal("意外的 ctrl 响应体".into())),
        }
    }

    /// 远端单条查询(本机短路直调;其余同 [`CtrlPool::list`])。
    pub(crate) async fn stat(
        &self,
        state: &Arc<DaemonState>,
        device: &str,
        path: &str,
    ) -> Result<mtdss_core::fs::DirEntryData, CtrlErr> {
        use mtdss_proto::ctrl::ctrl_request::Body as Req;
        use mtdss_proto::ctrl::ctrl_response::Body as Resp;
        if is_self(state, device) {
            let path = path.to_string();
            return blocking_fs(move || mtdss_core::fs::stat_path(&path))
                .await
                .map_err(local_fs_err);
        }
        let req = CtrlRequest {
            version: mtdss_proto::CTRL_VERSION,
            body: Some(Req::Stat(mtdss_proto::ctrl::StatRequest {
                path: path.to_string(),
            })),
        };
        let resp = self.request(state, device, req).await?;
        match resp.body {
            Some(Resp::Stat(r)) => {
                let e = r
                    .entry
                    .ok_or_else(|| CtrlErr::Internal("对端 stat 响应缺 entry".into()))?;
                Ok(proto_dir_entry(&e))
            }
            Some(Resp::Error(e)) => Err(CtrlErr::Peer {
                code: e.code,
                message: e.message,
            }),
            _ => Err(CtrlErr::Internal("意外的 ctrl 响应体".into())),
        }
    }

    /// 远端删除一批路径(本机短路直调,口径同 [`CtrlPool::list`];对端走
    /// ctrl v2 `delete`)。设施级回收不可用沿 FsError 批级上抛(CtrlErr::
    /// Peer,code=RECYCLE_UNAVAILABLE,操作端确认后 force 重发);逐条
    /// 失败留在响应失败表,批不半途弃废。
    pub(crate) async fn delete(
        &self,
        state: &Arc<DaemonState>,
        device: &str,
        paths: &[String],
        force: bool,
    ) -> Result<mtdss_proto::ctrl::DeleteResponse, CtrlErr> {
        use mtdss_proto::ctrl::ctrl_request::Body as Req;
        use mtdss_proto::ctrl::ctrl_response::Body as Resp;
        if is_self(state, device) {
            let paths = paths.to_vec();
            let (deleted, recycled, failed) =
                blocking_fs(move || mtdss_core::fs::delete_paths(&paths, force))
                    .await
                    .map_err(local_fs_err)?;
            return Ok(proto_delete_response(deleted, recycled, &failed));
        }
        let req = CtrlRequest {
            version: mtdss_proto::CTRL_VERSION,
            body: Some(Req::Delete(mtdss_proto::ctrl::DeleteRequest {
                paths: paths.to_vec(),
                force_permanent: force,
            })),
        };
        let resp = self.request(state, device, req).await?;
        match resp.body {
            Some(Resp::Delete(r)) => Ok(r),
            Some(Resp::Error(e)) => Err(CtrlErr::Peer {
                code: e.code,
                message: e.message,
            }),
            _ => Err(CtrlErr::Internal("意外的 ctrl 响应体".into())),
        }
    }

    /// 远端同机移动/改名一批路径(本机短路直调;对端走 ctrl v2 `move`。
    /// 跨设备在 RPC 参数层已拒,这里只承担同机面)。policy 为冲突策略
    /// (M4a-3;skip 命中整对跳过进响应 skipped,空策略 = legacy 且线上
    /// 不带 policy 字段——旧端互通面)。
    pub(crate) async fn move_(
        &self,
        state: &Arc<DaemonState>,
        device: &str,
        pairs: &[(String, String)],
        policy: &mtdss_core::transport::intake::TransferPolicy,
    ) -> Result<mtdss_proto::ctrl::MoveResponse, CtrlErr> {
        use mtdss_proto::ctrl::ctrl_request::Body as Req;
        use mtdss_proto::ctrl::ctrl_response::Body as Resp;
        if state.self_device_id().as_deref() == Some(device) {
            let pairs: Vec<(PathBuf, PathBuf)> = pairs
                .iter()
                .map(|(f, t)| (PathBuf::from(f), PathBuf::from(t)))
                .collect();
            let policy = policy.clone();
            let out = blocking_fs(move || mtdss_core::fs::move_entries_with(&pairs, &policy))
                .await
                .map_err(local_fs_err)?;
            return Ok(proto_move_response(&out));
        }
        let req = CtrlRequest {
            version: mtdss_proto::CTRL_VERSION,
            body: Some(Req::Move(mtdss_proto::ctrl::MoveRequest {
                pairs: pairs
                    .iter()
                    .map(|(from, to)| mtdss_proto::ctrl::MovePair {
                        from: from.clone(),
                        to: to.clone(),
                    })
                    .collect(),
                policy: policy_opt(policy),
            })),
        };
        let resp = self.request(state, device, req).await?;
        match resp.body {
            Some(Resp::Move(r)) => Ok(r),
            Some(Resp::Error(e)) => Err(CtrlErr::Peer {
                code: e.code,
                message: e.message,
            }),
            _ => Err(CtrlErr::Internal("意外的 ctrl 响应体".into())),
        }
    }

    /// 远端同机复制一批路径入目标目录(本机短路直调;对端走 ctrl v2
    /// `copy` → fs::copy_paths_with)。不经网络传输面(跨设备复制由
    /// Task 7/8 的 download/orchestrate 承担)。policy 为冲突策略(M4a-3;
    /// skip 命中整项跳过进响应 skipped,空策略 = legacy 且线上不带
    /// policy 字段——旧端互通面)。
    pub(crate) async fn copy(
        &self,
        state: &Arc<DaemonState>,
        device: &str,
        paths: &[String],
        dir: &str,
        policy: &mtdss_core::transport::intake::TransferPolicy,
    ) -> Result<mtdss_proto::ctrl::CopyResponse, CtrlErr> {
        use mtdss_proto::ctrl::ctrl_request::Body as Req;
        use mtdss_proto::ctrl::ctrl_response::Body as Resp;
        if state.self_device_id().as_deref() == Some(device) {
            let paths: Vec<PathBuf> = paths.iter().map(PathBuf::from).collect();
            let dir = PathBuf::from(dir);
            let policy = policy.clone();
            let out = blocking_fs(move || mtdss_core::fs::copy_paths_with(&paths, &dir, &policy))
                .await
                .map_err(local_fs_err)?;
            return Ok(proto_copy_response(&out));
        }
        let req = CtrlRequest {
            version: mtdss_proto::CTRL_VERSION,
            body: Some(Req::Copy(mtdss_proto::ctrl::CopyRequest {
                paths: paths.to_vec(),
                dir: dir.to_string(),
                policy: policy_opt(policy),
            })),
        };
        let resp = self.request(state, device, req).await?;
        match resp.body {
            Some(Resp::Copy(r)) => Ok(r),
            Some(Resp::Error(e)) => Err(CtrlErr::Peer {
                code: e.code,
                message: e.message,
            }),
            _ => Err(CtrlErr::Internal("意外的 ctrl 响应体".into())),
        }
    }

    /// 跨设备编排(Task 7 download 发起面):让**源设备**起一个回显 job_id
    /// 的 upload 驱动,把 paths 传给 to_device(操作端自己)。仅远端面(本
    /// 机源属 local_copy,api 层已拒,此处兜底报 Internal);源端拒绝/校验
    /// 失败不按错误上抛,原样带回 OrchestrateResponse(accepted=false +
    /// detail)供调用方落作业 failed。`old_rels` 非空 = 重发编排(v2-P2
    /// Task 8):源端以 partial 模式预检,顶层已消失项收进 missing、dropped
    /// 由源端推流前统一清接收端残留(spec §5.1);空 = 普通首发。
    #[allow(clippy::too_many_arguments)] // 编排面参数齐平 brief 钉死的请求形态(与 run_send_with 同款豁免)
    pub(crate) async fn orchestrate(
        &self,
        state: &Arc<DaemonState>,
        device: &str,
        job_id: &str,
        to_device: &str,
        paths: &[String],
        target_dir: &str,
        policy: &mtdss_core::transport::intake::TransferPolicy,
        old_rels: &[String],
        display_name: &str,
    ) -> Result<mtdss_proto::ctrl::OrchestrateResponse, CtrlErr> {
        use mtdss_proto::ctrl::ctrl_request::Body as Req;
        use mtdss_proto::ctrl::ctrl_response::Body as Resp;
        if state.self_device_id().as_deref() == Some(device) {
            return Err(CtrlErr::Internal("编排源不能是本机(本机复制请用 local_copy)".into()));
        }
        let req = CtrlRequest {
            version: mtdss_proto::CTRL_VERSION,
            body: Some(Req::Orchestrate(mtdss_proto::ctrl::OrchestrateRequest {
                job_id: job_id.to_string(),
                to_device: to_device.to_string(),
                paths: paths.to_vec(),
                target_dir: target_dir.to_string(),
                policy: Some(policy_proto(policy)),
                old_rels: old_rels.to_vec(),
                // V3-1 增量:任务代表名下发(发起端建卡算好;空 = 旧端形态,
                // 对端回退现状)。
                display_name: display_name.to_string(),
            })),
        };
        let resp = self.request(state, device, req).await?;
        match resp.body {
            Some(Resp::Orchestrate(r)) => Ok(r),
            Some(Resp::Error(e)) => Err(CtrlErr::Peer {
                code: e.code,
                message: e.message,
            }),
            _ => Err(CtrlErr::Internal("意外的 ctrl 响应体".into())),
        }
    }

    /// 远端作业取消(Task 7):download 取消链的第二跳——本地作业取消后
    /// best-effort 通知源端停驱动重试(收端断链只能杀当轮,预算内的重连
    /// 轮次靠它止步)。本机短路直调注册表(与 list/stat 同口径)。
    pub(crate) async fn cancel_job(
        &self,
        state: &Arc<DaemonState>,
        device: &str,
        job_id: &str,
    ) -> Result<bool, CtrlErr> {
        use mtdss_proto::ctrl::ctrl_request::Body as Req;
        use mtdss_proto::ctrl::ctrl_response::Body as Resp;
        if state.self_device_id().as_deref() == Some(device) {
            return Ok(state.jobs().cancel(job_id));
        }
        let req = CtrlRequest {
            version: mtdss_proto::CTRL_VERSION,
            body: Some(Req::CancelJob(mtdss_proto::ctrl::CancelJobRequest {
                job_id: job_id.to_string(),
            })),
        };
        let resp = self.request(state, device, req).await?;
        match resp.body {
            Some(Resp::CancelJob(r)) => Ok(r.cancelled),
            Some(Resp::Error(e)) => Err(CtrlErr::Peer {
                code: e.code,
                message: e.message,
            }),
            _ => Err(CtrlErr::Internal("意外的 ctrl 响应体".into())),
        }
    }

    /// 远端作业查询(Task 7):download 驱动周期探测源端作业态,轮失败后
    /// 据此区分「源端还在重试(继续等)」与「源端已终局(本地收口)」。
    /// 本机短路直调注册表;对端无此作业回 state="missing"。
    pub(crate) async fn job_query(
        &self,
        state: &Arc<DaemonState>,
        device: &str,
        job_id: &str,
    ) -> Result<mtdss_proto::ctrl::JobQueryResponse, CtrlErr> {
        use mtdss_proto::ctrl::ctrl_request::Body as Req;
        use mtdss_proto::ctrl::ctrl_response::Body as Resp;
        if state.self_device_id().as_deref() == Some(device) {
            let snap = state.jobs().snapshot();
            let rels = state.jobs().job_rels(job_id);
            return Ok(job_query_response(snap.iter().find(|j| j.id == job_id), rels));
        }
        let req = CtrlRequest {
            version: mtdss_proto::CTRL_VERSION,
            body: Some(Req::JobQuery(mtdss_proto::ctrl::JobQueryRequest {
                job_id: job_id.to_string(),
            })),
        };
        let resp = self.request(state, device, req).await?;
        match resp.body {
            Some(Resp::JobQuery(r)) => Ok(r),
            Some(Resp::Error(e)) => Err(CtrlErr::Peer {
                code: e.code,
                message: e.message,
            }),
            _ => Err(CtrlErr::Internal("意外的 ctrl 响应体".into())),
        }
    }

    /// 远端收件区残留清理(spec §5.3):让对端按 sha256(rel) 定位
    /// `.mtdss` 记录,身份吻合即删记录与 `.part`,回执 cleaned 数(幂等:
    /// 缺席 rel 静默跳过)。重新发起的 dropped 清理(Task 6,尽力而为)与
    /// transfer.clear(Task 7,Ok 才删本地记录)两个调用方共用。仅远端面
    /// ——本机残留清理由调用方按 §5.2 直调 `intake::cleanup_rels`
    /// (download 清除/本地复制类),不经此包装,故无本机短路臂。
    pub(crate) async fn cleanup_outbox(
        &self,
        state: &Arc<DaemonState>,
        to_device: &str,
        dir: &str,
        rels: &[String],
    ) -> Result<u32, CtrlErr> {
        use mtdss_proto::ctrl::ctrl_request::Body as Req;
        use mtdss_proto::ctrl::ctrl_response::Body as Resp;
        let req = CtrlRequest {
            version: mtdss_proto::CTRL_VERSION,
            body: Some(Req::CleanupOutbox(mtdss_proto::ctrl::CleanupOutboxRequest {
                dir: dir.to_string(),
                rels: rels.to_vec(),
            })),
        };
        let resp = self.request(state, to_device, req).await?;
        match resp.body {
            Some(Resp::CleanupOutbox(r)) => Ok(r.cleaned),
            Some(Resp::Error(e)) => Err(CtrlErr::Peer {
                code: e.code,
                message: e.message,
            }),
            _ => Err(CtrlErr::Internal("意外的 ctrl 响应体".into())),
        }
    }

    /// 登出拆卸:清空连接池与 per-device 单飞锁表(锁内即返,不等待在途
    /// 请求)。端点随池条目 drop,其上连接一并关闭——登出后旧连接不得
    /// 复用(会话态已清,打洞会话随登录代际失效);在途请求持有的单飞锁
    /// 是独立 Arc,清表不影响其解锁语义。
    pub(crate) fn reset(&self) {
        self.conns.lock().unwrap().clear();
        self.locks.lock().unwrap().clear();
    }

    /// 请求主路径:离线预检 → per-device 单飞 → 池命中(连接未死)复用 →
    /// 请求级失败(或死连接)驱逐后整程重试恰一次 → 成功连接入池。
    async fn request(
        &self,
        state: &Arc<DaemonState>,
        device: &str,
        req: CtrlRequest,
    ) -> Result<CtrlResponse, CtrlErr> {
        // 离线预检:目录快照 online=false / 无快照 / 条目缺失一律离线,
        // 先于任何打洞(快错,不悬等信号/打洞超时)。
        if !device_online(state, device) {
            return Err(CtrlErr::PeerOffline(format!("设备离线: {device}")));
        }
        // 单飞:std 锁内取锁 Arc 即放(锁不跨 await),打洞全程持 tokio 锁
        let lock = self.device_lock(device);
        let _guard = lock.lock().await;

        // 池命中且连接未死 → 复用;请求级失败或死连接 → 驱逐(下方整程重试)
        let cached: Option<PooledConn> = {
            let conns = self.conns.lock().unwrap();
            conns.get(device).cloned()
        };
        if let Some(pooled) = cached {
            if conn_alive(&pooled.conn).await {
                match send_request(&pooled.conn, &req, CTRL_SEND_TIMEOUT).await {
                    Ok(resp) => return Ok(resp),
                    Err(e) => {
                        tracing::warn!("ctrl 复用连接请求失败,驱逐后整程重试: {e:#}");
                    }
                }
            }
            self.conns.lock().unwrap().remove(device);
        }

        // 整程打洞(CtrlConnect/CtrlAccept/Candidates/互拨)→ 请求 → 入池
        // (allow_relay=true 维持现状语义:直连穷尽且资格在手走中继兜底;
        // 返回的中继标记暂不消费,Task 3 池化按标记分流,此处仅解构)
        let (ep, conn, _relay) = self.dial_session(state, device, true).await?;
        let resp = send_request(&conn, &req, CTRL_SEND_TIMEOUT)
            .await
            .map_err(|e| CtrlErr::Timeout(format!("{e:#}")))?;
        self.conns.lock().unwrap().insert(
            device.to_string(),
            PooledConn {
                _keep: ep,
                conn,
            },
        );
        Ok(resp)
    }

    /// per-device 单飞锁(缺失即建;Arc 生命周期随池,同设备恒同一把)。
    fn device_lock(&self, device: &str) -> Arc<tokio::sync::Mutex<()>> {
        let mut locks = self.locks.lock().unwrap();
        locks.entry(device.to_string()).or_default().clone()
    }

    /// 经枢纽建立会话(V3-5 打洞编舞收敛点):先订阅(对端信令不会早于
    /// 订阅窗口)→ CtrlConnect → 等 CtrlAccept → 建双角色端点 → 互发
    /// Candidates → 互拨段按 `allow_relay` 分流([`punch_selector`]):
    /// `true` 走 connect_with_fallback(直连穷尽且中继资格在手走服务端
    /// 中继,v2-P1c 第二刀),`false` 直调 punch_connect 纯直连(预热
    /// direct-only 变体,spec V3-5 §3.1(4):打不通即失败,不烧服务端
    /// 中继)。返回 (端点, 连接, 中继标记):直连恒 None,中继兜底命中
    /// Some(RelayPath);端点随连接保活(调用方入池;中继标记随 Task 3
    /// 池化消费)。
    async fn dial_session(
        &self,
        state: &Arc<DaemonState>,
        device: &str,
        allow_relay: bool,
    ) -> Result<(quinn::Endpoint, quinn::Connection, Option<fallback::RelayPath>), CtrlErr> {
        let hub = state.hub();
        let local = |e: anyhow::Error| CtrlErr::Internal(format!("{e:#}"));

        // 先订阅再发信令(Accept/Candidates 不会早于订阅窗口,漏帧即失败)
        let mut rx = hub
            .subscribe()
            .ok_or_else(|| CtrlErr::PeerOffline("信令未在线,无法发起控制通道".into()))?;
        // 中继事件并列订阅(兜底编排的分配推送落点;先订阅后打洞,推送不早
        // 于订阅窗口):None 同「信令未在线」——兜底无从感知。
        let mut relay_rx = hub
            .relay_events()
            .ok_or_else(|| CtrlErr::PeerOffline("信令未在线,无法发起控制通道".into()))?;
        let session = state
            .session()
            .ok_or_else(|| CtrlErr::PeerOffline("未登录".into()))?;
        let me = flow::load_identity(&state.home()).map_err(local)?;

        // 对端公钥:现拉目录(REST 为权威,刚上线的对端才在)+ TOFU 固定
        let devices = flow::fetch_devices(&session).await.map_err(local)?;
        let peer_pk = flow::peer_pk_from(&devices, device).map_err(local)?;
        flow::tofu_pin(&state.home(), device, &peer_pk).map_err(local)?;

        let sid = flow::rand_session_id();
        // CtrlConnect → 等 CtrlAccept:短窗口内重发同一 CtrlConnect(见
        // CTRL_ACCEPT_WINDOW 注),任一次等到即过。
        for attempt in 0..CTRL_ACCEPT_ATTEMPTS {
            if attempt > 0 {
                tokio::time::sleep(CTRL_ACCEPT_RETRY_PACE).await;
            }
            hub.relay(
                device,
                &seal(
                    &me,
                    &peer_pk,
                    &SignalBody::CtrlConnect {
                        session_id: sid.clone(),
                    },
                ),
            )
            .await
            .map_err(local)?;
            match tokio::time::timeout(
                CTRL_ACCEPT_WINDOW,
                wait_signal(&mut rx, device, |b| {
                    matches!(b, SignalBody::CtrlAccept { session_id } if *session_id == sid)
                }),
            )
            .await
            {
                Ok(Ok(_accept)) => break,
                Ok(Err(e)) if attempt + 1 == CTRL_ACCEPT_ATTEMPTS => {
                    return Err(CtrlErr::Timeout(format!("{e:#}")));
                }
                Ok(Err(e)) => {
                    tracing::debug!("ctrl: 第 {} 次等待 CtrlAccept 失败: {e:#}", attempt + 1)
                }
                Err(_) if attempt + 1 == CTRL_ACCEPT_ATTEMPTS => {
                    return Err(CtrlErr::Timeout(format!(
                        "等待 {device} 的 CtrlAccept 超时({CTRL_ACCEPT_ATTEMPTS} 次尝试)"
                    )));
                }
                Err(_) => tracing::debug!("ctrl: 第 {} 次等待 CtrlAccept 超窗,重发", attempt + 1),
            }
        }

        // Accept 后建双角色端点,回发本端 Candidates,再等对端 Candidates,互拨
        let (ep, mine, nat_symmetric, _stun_ok) =
            flow::setup_punch_endpoint(&me, device, &peer_pk, &hub.stun(), self.tuning)
                .map_err(local)?;
        hub.relay(
            device,
            &seal(
                &me,
                &peer_pk,
                &SignalBody::Candidates {
                    session_id: sid.clone(),
                    addrs: mine.iter().map(|a| a.to_string()).collect(),
                },
            ),
        )
        .await
        .map_err(local)?;
        let theirs = wait_signal(&mut rx, device, |b| {
            matches!(b, SignalBody::Candidates { session_id, .. } if *session_id == sid)
        })
        .await
        .map_err(|e| CtrlErr::Timeout(format!("{e:#}")))?;
        let theirs = match theirs {
            SignalBody::Candidates { addrs, .. } => flow::parse_candidates(&addrs),
            other => return Err(CtrlErr::Internal(format!("意外的信令体 {other:?}"))),
        };

        let mut allowed: HashMap<String, VerifyingKey> = HashMap::new();
        allowed.insert(device.to_string(), peer_pk);
        // 互拨段按 allow_relay 分流([`punch_selector`],V3-5 direct-only
        // 变体):true 维持 connect-with-fallback(与传输面 send_round 同形:
        // 两侧端点/白名单/候选交换流程不动,仅互拨段替换),false 直调
        // punch_connect 纯直连——打不通即失败,不烧服务端中继(spec V3-5
        // §3.1(4))。两臂直连败同归今日 punch_err 口径包装(NAT 标签/
        // 候选数,未开通账号行为不变);WithFallback 臂错误三臂按全局约束
        // 映射——RelayDenied(4/5)终局文案收口 fallback;RelayFailed
        // (拒 6 超并发/推送超窗/兜底互拨败)按可重试语义归 Punch。
        let (conn, relay) = match punch_selector(allow_relay) {
            PunchMode::WithFallback => {
                let outcome = fallback::connect_with_fallback(
                    hub,
                    &mut relay_rx,
                    &me,
                    device,
                    &peer_pk,
                    &ep,
                    &theirs,
                    ctrl_punch(self.tuning),
                    FallbackRole::Initiator,
                    RelayPurpose::Ctrl,
                    state.send_opts().fault_force_relay,
                )
                .await;
                match outcome {
                    Ok(o) => (o.conn, o.relay),
                    Err(FallbackErr::Direct(e)) => {
                        return Err(CtrlErr::Punch(
                            flow::punch_err(e, nat_symmetric, mine.len()).to_string(),
                        ));
                    }
                    Err(FallbackErr::RelayDenied { code }) => {
                        return Err(CtrlErr::Internal(fallback::denied_message(code)));
                    }
                    Err(FallbackErr::RelayFailed(m)) => return Err(CtrlErr::Punch(m)),
                }
            }
            PunchMode::DirectOnly => {
                // 纯直连互拨原语直调(与 fallback 直连段同参同调优),中继
                // 标记恒 None;force_relay 注错仅属兜底编排面,本臂不存在。
                let conn = punch::punch_connect(
                    &ep,
                    &me,
                    &allowed,
                    &peer_pk,
                    &theirs,
                    &ctrl_punch(self.tuning),
                )
                .await
                .map_err(|e| {
                    CtrlErr::Punch(flow::punch_err(e, nat_symmetric, mine.len()).to_string())
                })?;
                (conn, None)
            }
        };
        Ok((ep, conn, relay))
    }
}

/// 目录快照离线预检:无快照/条目缺失/online=false 一律视为离线。
/// 快照与公钥缓存同源刷新,缺失条目打洞也无从解析公钥,报离线最诚实。
fn device_online(state: &DaemonState, device: &str) -> bool {
    state
        .devices()
        .and_then(|list| {
            list.iter()
                .find(|d| d["device_id"].as_str() == Some(device))
                .cloned()
        })
        .map(|d| d["online"].as_bool().unwrap_or(false))
        .unwrap_or(false)
}

/// 连接活性:`closed()` future 就绪 = 对端已关闭/连接已死。一次性零等待
/// 轮询(poll_fn,不阻塞、不跨 await 点),池内条目使用前必查。
async fn conn_alive(conn: &quinn::Connection) -> bool {
    use std::future::Future as _;
    let mut closed = Box::pin(conn.closed());
    std::future::poll_fn(move |cx| match closed.as_mut().poll(cx) {
        std::task::Poll::Ready(_) => std::task::Poll::Ready(false),
        std::task::Poll::Pending => std::task::Poll::Ready(true),
    })
    .await
}

/// std-fs 阻塞调用统一收口:落 tokio 阻塞线程池(根卷枚举/整树复制/
/// 回收删除等毫秒~秒级调用直跑会占死异步 worker,饿死同 reactor 的
/// QUIC/信令任务——M3b 携带债清偿)。JoinError(运行时收尾边缘)归
/// Internal,不 panic。
async fn blocking_fs<T: Send + 'static>(
    f: impl FnOnce() -> Result<T, mtdss_core::fs::FsError> + Send + 'static,
) -> Result<T, mtdss_core::fs::FsError> {
    match tokio::task::spawn_blocking(f).await {
        Ok(r) => r,
        Err(e) => Err(mtdss_core::fs::FsError::Internal(format!(
            "阻塞任务收尾失败: {e}"
        ))),
    }
}

/// 本机短路的 core fs 错误映射:与对端 FsError 同语义(CtrlErr::Peer,
/// code 即 fs 稳定码,RPC 层同样加「远端: 」前缀);正式契约 Task 5 收口。
fn local_fs_err(e: mtdss_core::fs::FsError) -> CtrlErr {
    CtrlErr::Peer {
        code: e.code().into(),
        message: e.to_string(),
    }
}

/// usize → proto uint32(批量条目数,u32 上界内必然成立,防御性饱和)。
fn count_u32(n: usize) -> u32 {
    u32::try_from(n).unwrap_or(u32::MAX)
}

/// 数据侧 PathOpFailure → proto PathOpFailure(逐字段拷贝)。
fn proto_failure(f: &mtdss_core::fs::PathOpFailure) -> mtdss_proto::ctrl::PathOpFailure {
    mtdss_proto::ctrl::PathOpFailure {
        path: f.path.clone(),
        code: f.code.clone(),
        message: f.message.clone(),
    }
}

/// delete 结果 → proto DeleteResponse(deleted 计数 + 回收标志 + 逐条失败表)。
fn proto_delete_response(
    deleted: usize,
    recycled: bool,
    failed: &[mtdss_core::fs::PathOpFailure],
) -> mtdss_proto::ctrl::DeleteResponse {
    mtdss_proto::ctrl::DeleteResponse {
        deleted: count_u32(deleted),
        recycled,
        failed: failed.iter().map(proto_failure).collect(),
    }
}

/// move 结果 → proto MoveResponse(moved 计数 + 逐条失败表 + skip 跳过表)。
fn proto_move_response(out: &mtdss_core::fs::MoveOutcome) -> mtdss_proto::ctrl::MoveResponse {
    mtdss_proto::ctrl::MoveResponse {
        moved: count_u32(out.moved.len()),
        failed: out.failed.iter().map(proto_failure).collect(),
        skipped: out.skipped.clone(),
    }
}

/// copy 结果 → proto CopyResponse(copied 计数 + skip 跳过表;fs::copy_paths
/// 任一条失败整批 Err,成功即无失败表条目)。
fn proto_copy_response(out: &mtdss_core::fs::CopyOutcome) -> mtdss_proto::ctrl::CopyResponse {
    mtdss_proto::ctrl::CopyResponse {
        copied: count_u32(out.copied.len()),
        failed: out.failed.iter().map(proto_failure).collect(),
        skipped: out.skipped.clone(),
    }
}

/// proto DirEntry → 数据侧 DirEntryData(逐字段拷贝;children 缺省 None =
/// 旧对端不带子项数,互通面;包裹消息在场 → count)。
fn proto_dir_entry(e: &mtdss_proto::ctrl::DirEntry) -> mtdss_core::fs::DirEntryData {
    mtdss_core::fs::DirEntryData {
        name: e.name.clone(),
        path: e.path.clone(),
        size: e.size,
        mtime_ms: e.mtime_ms,
        is_dir: e.is_dir,
        children: e.children.map(|c| c.count),
    }
}

/// 引擎策略 → ctrl proto 策略(逐表拷贝;两型同形,无信令层归一问题)。
fn policy_proto(
    p: &mtdss_core::transport::intake::TransferPolicy,
) -> mtdss_proto::ctrl::TransferPolicy {
    mtdss_proto::ctrl::TransferPolicy {
        skip: p.skip.clone(),
        overwrite: p.overwrite.clone(),
    }
}

/// 空策略 → 线上 None(不带 policy 字段):与旧端请求逐字节同形,响应端
/// 缺省解码即 legacy(M4a-3 互通契约,ctrl_e2e absent-policy 用例钉死)。
fn policy_opt(
    p: &mtdss_core::transport::intake::TransferPolicy,
) -> Option<mtdss_proto::ctrl::TransferPolicy> {
    if p.skip.is_empty() && p.overwrite.is_empty() {
        None
    } else {
        Some(policy_proto(p))
    }
}

/// ctrl proto 策略 → 引擎策略(逐表拷贝)。
fn policy_engine(p: &mtdss_proto::ctrl::TransferPolicy) -> mtdss_core::transport::intake::TransferPolicy {
    mtdss_core::transport::intake::TransferPolicy {
        skip: p.skip.clone(),
        overwrite: p.overwrite.clone(),
    }
}

/// 注册表快照条目 → JobQueryResponse(缺作业 → state="missing",download
/// 驱动据此把「源端已无此作业」按失败收口;error 恒为字符串,成功空串)。
/// `rels` = 作业的预扫 rel 全集(v2-P2 Task 8 回填面:源端 upload 驱动随
/// 响应回给操作端,操作端轮询写回 store;download/remote_copy 的 meta.
/// rels 恒空,自然回空集)。
fn job_query_response(
    snap: Option<&crate::jobs::JobSnap>,
    rels: Vec<String>,
) -> mtdss_proto::ctrl::JobQueryResponse {
    match snap {
        Some(j) => mtdss_proto::ctrl::JobQueryResponse {
            state: j.state.clone(),
            files_done: j.files_done,
            files_total: j.files_total,
            bytes_done: j.bytes_done,
            bytes_total: j.bytes_total,
            error: j.error.clone().unwrap_or_default(),
            rels,
            // 截断账(R7 评审 I-2):源端照片相册的帽截断差额,操作端并入
            // 本端卡片「跳过 N 项」;旧端不识=0(线上缺省,兼容)。
            skipped: j.skipped,
        },
        None => mtdss_proto::ctrl::JobQueryResponse {
            state: "missing".into(),
            files_done: 0,
            files_total: 0,
            bytes_done: 0,
            bytes_total: 0,
            error: "任务不存在".into(),
            rels: Vec::new(),
            skipped: 0,
        },
    }
}

// ---------- 响应端常驻循环 ----------

/// 响应端等订阅的小步步距(防快败热旋;与常驻接收的轮间小歇同规格)。
const RESPONDER_PACE: Duration = Duration::from_millis(500);

/// 活跃 CtrlConnect 会话的 session id 集合(每代际一份):发起端对
/// CtrlAccept 短窗重发**同一** CtrlConnect,无丢失面的定向通道会逐份送达
/// ——同一 sid 必须恰起一个会话任务,重发噪声不得放大成并发打洞(负载下
/// 多会话同时建端点/收集候选会互相挤兑请求预算)。guard 随会话任务结束
/// (drop)释放该 sid。
type LiveSids = Arc<Mutex<HashSet<Vec<u8>>>>;

/// sid 占位守卫:被 spawn 的会话任务持有,任务收尾即从活跃集合释放。
struct SidGuard {
    live: LiveSids,
    sid: Vec<u8>,
}

impl Drop for SidGuard {
    fn drop(&mut self) {
        self.live.lock().unwrap().remove(&self.sid);
    }
}

/// 响应端常驻任务:登录后取枢纽的 CtrlConnect 定向通道(随代际安装即
/// 存在,取件晚于上线的窗口里来件在通道缓冲排队——广播无订阅者即丢,
/// 来件请求不可丢),每条 CtrlConnect(按 sid 去重,见 [`LiveSids`])spawn
/// 一个会话任务(CtrlAccept → punch → serve_conn,见 [`respond_session`]),
/// 多会话并发。通道关闭(代际更替)回外层重取下一代;登出回外层按登录态
/// 重进;未登录自挂起(通知驱动);收尾由 Daemon shutdown abort。
pub(crate) async fn responder_loop(state: Arc<DaemonState>) {
    let mut login_rx = state.login_watch();
    loop {
        // 未登录:通知驱动挂起(不轮询);发送端随 state 常驻,Err 不可达,
        // 防御性返回。
        while !state.logged_in() {
            if login_rx.changed().await.is_err() {
                return;
            }
        }
        // 等连接代际并取 CtrlConnect 定向通道:枢纽连接建立即可取;离线
        // 期间小步轮询等待,登出翻转即分流。
        let mut ctrl_rx = loop {
            if let Some(rx) = state.hub().take_ctrl_rx() {
                break rx;
            }
            tokio::select! {
                _ = tokio::time::sleep(RESPONDER_PACE) => {}
                r = login_rx.changed() => {
                    if r.is_err() {
                        return;
                    }
                }
            }
        };
        // 消费循环:CtrlConnect → spawn 会话(通道只投递 CtrlConnect,防御
        // 分支忽略其余体);同一 sid 去重恰一个会话;通道关闭(代际更替:
        // 断连/登出)回外层重取新代。
        let live: LiveSids = Arc::default();
        loop {
            match ctrl_rx.recv().await {
                Some((from, SignalBody::CtrlConnect { session_id })) => {
                    // 防御:本机请求走 CtrlPool 短路,不存在自拨
                    if Some(from.as_str()) == state.self_device_id().as_deref() {
                        continue;
                    }
                    // 去重:重发的同一 CtrlConnect 不再重复起会话
                    let claimed = live.lock().unwrap().insert(session_id.clone());
                    if !claimed {
                        continue;
                    }
                    let guard = SidGuard {
                        live: live.clone(),
                        sid: session_id.clone(),
                    };
                    let st = state.clone();
                    tokio::spawn(async move {
                        let _guard = guard;
                        respond_session(st, from, session_id).await;
                    });
                }
                Some(_) => {}
                None => break, // 代际更替(断连/登出):回外层重取新代
            }
        }
    }
}

/// 单会话响应任务:先订阅再发信令 → CtrlAccept(即刻回,不先建端点)→
/// 建双角色端点 → 本端 Candidates → 等对端 Candidates(session id 过滤)→
/// connect_with_fallback(响应方角色:直连穷尽且中继资格在手时等本对端
/// 分配推送,v2-P1c 第二刀)→ serve_conn(连接存活期服务请求)。任一环节
/// 失败只告警弃会话,不影响其余会话;会话任务无返回值,失败即静默收尾。
///
/// 编舞按 brief 钉的「等 CtrlAccept → 双方各自建端点 → 互发 Candidates」:
/// Accept 先行让发起端尽早解锁(建端点的候选收集耗时落在两侧 Candidates
/// 窗口,该窗口 10s,远比 Accept 窗口宽裕)。
///
/// 发起端对 CtrlAccept 短窗重发同一 CtrlConnect(`CTRL_ACCEPT_WINDOW`);
/// 响应端按 sid 去重(见 [`LiveSids`]),重发只补投递、不重复起会话——
/// CtrlConnect 的第一份到达即起会话,后续重发在响应端被丢弃。
async fn respond_session(state: Arc<DaemonState>, from: String, session_id: Vec<u8>) {
    let hub = state.hub();
    // 先订阅再发信令:对端 Candidates 只会在我方 CtrlAccept 之后到达,
    // 订阅先于 CtrlAccept 即不漏帧。
    let Some(mut rx) = hub.subscribe() else {
        tracing::warn!("ctrl 会话:信令未在线,弃会话");
        return;
    };
    // 中继事件并列订阅(响应方兜底的分配推送落点;None → 弃会话,发起端
    // 下一笔重试自愈)。
    let Some(mut relay_rx) = hub.relay_events() else {
        tracing::warn!("ctrl 会话:信令未在线(中继事件不可订阅),弃会话");
        return;
    };
    let Ok(me) = flow::load_identity(&state.home()) else {
        tracing::warn!("ctrl 会话:加载本机身份失败,弃会话");
        return;
    };
    // 发送方公钥:枢纽缓存(开箱时已经服务器目录解析 + TOFU 固定,常态
    // 必命中;未命中属防御分支,弃会话等重试)
    let Some(peer_pk) = hub.peer_pk(&from) else {
        tracing::warn!("ctrl 会话:来件方 {from} 不在公钥缓存,弃会话");
        return;
    };
    // Accept 先行:发起端等的就是它,刻不容缓
    if let Err(e) = hub
        .relay(
            &from,
            &seal(
                &me,
                &peer_pk,
                &SignalBody::CtrlAccept {
                    session_id: session_id.clone(),
                },
            ),
        )
        .await
    {
        tracing::warn!("ctrl 会话:回 CtrlAccept 失败,弃会话: {e:#}");
        return;
    }
    let (ep, mine, nat_symmetric, _stun_ok) = match flow::setup_punch_endpoint(
        &me,
        &from,
        &peer_pk,
        &hub.stun(),
        state.ctrl().tuning(),
    ) {
        Ok(v) => v,
        Err(e) => {
            tracing::warn!("ctrl 会话:建端点失败,弃会话: {e:#}");
            return;
        }
    };
    if let Err(e) = hub
        .relay(
            &from,
            &seal(
                &me,
                &peer_pk,
                &SignalBody::Candidates {
                    session_id: session_id.clone(),
                    addrs: mine.iter().map(|a| a.to_string()).collect(),
                },
            ),
        )
        .await
    {
        tracing::warn!("ctrl 会话:回 Candidates 失败,弃会话: {e:#}");
        return;
    }
    let theirs = match wait_signal(&mut rx, &from, |b| {
        matches!(b, SignalBody::Candidates { session_id: s, .. } if *s == session_id)
    })
    .await
    {
        Ok(SignalBody::Candidates { addrs, .. }) => flow::parse_candidates(&addrs),
        Ok(other) => {
            tracing::warn!("ctrl 会话:意外的信令体 {other:?},弃会话");
            return;
        }
        Err(e) => {
            tracing::warn!("ctrl 会话:等对端 Candidates 失败,弃会话: {e:#}");
            return;
        }
    };
    let mut allowed: HashMap<String, VerifyingKey> = HashMap::new();
    allowed.insert(from.clone(), peer_pk);
    // punch 段换 connect-with-fallback(响应方角色:直连穷尽且资格在手时在
    // 本会话开头的订阅上等本对端分配推送,与传输面 recv_handshake 同形)。
    // 三条失败臂均只告警弃会话(Direct 臂沿 punch_err 全链保 NAT 标签/
    // 候选数诊断口径);relay=Ok 臂日志带中继标记(relay_dial 的既有日志
    // 锚「中继兜底已建立」已覆盖兜底生效面,此处不重复)。
    match fallback::connect_with_fallback(
        hub,
        &mut relay_rx,
        &me,
        &from,
        &peer_pk,
        &ep,
        &theirs,
        ctrl_punch(state.ctrl().tuning()),
        FallbackRole::Responder,
        RelayPurpose::Ctrl,
        state.recv_opts().fault_force_relay,
    )
    .await
    {
        Ok(o) => {
            tracing::info!("ctrl 会话已打通(from={from}, relay={})", o.relay.is_some());
            // 连接存活期服务请求;对端退出/网络断即返回(任务收尾)。
            // handler 携 state(Task 7:orchestrate 响应分支要起本地 upload
            // 作业)——Arc 按流克隆,handler 闭包只要求 Send + Sync。
            let st = state.clone();
            serve_conn(o.conn, move |req: CtrlRequest| {
                let st = st.clone();
                async move { handle_ctrl_request(st, req).await }
            })
            .await;
        }
        Err(FallbackErr::Direct(e)) => {
            tracing::warn!(
                "ctrl 会话:直连打洞失败且无兜底,弃会话: {:#}",
                flow::punch_err(e, nat_symmetric, mine.len())
            );
        }
        Err(FallbackErr::RelayDenied { code }) => {
            tracing::warn!(
                "ctrl 会话:中继兜底被拒,弃会话: {}",
                fallback::denied_message(code)
            );
        }
        Err(FallbackErr::RelayFailed(m)) => {
            tracing::warn!("ctrl 会话:中继兜底失败,弃会话: {m}");
        }
    }
}

/// ctrl 请求分发(serve_conn 的 async handler):version 不符 →
/// UNSUPPORTED_VERSION(ok=false);List/Stat/Delete/Move/Copy → core fs,
/// fs 调用一律经 [`blocking_fs`] 落阻塞线程池(见其注);Orchestrate/
/// CancelJob/JobQuery(Task 7)→ 作业注册表(orchestrate 在**本机**起
/// 回显 job_id 的 upload 作业,拒绝不按错误上抛、原样带回 accepted=false
/// 与 detail);CleanupOutbox(spec §5.3)→ dir 同 list/stat 口径校验后
/// 跑 intake::cleanup_rels;Move/Copy 臂在 policy 转换后即校验策略上限
/// (与发起端 [`check_policy_limits`] 对称,超限 POLICY_LIMIT)。FsError
/// 原码透传,截断标志原样携带,批量逐条失败表随响应携带。请求读不出/
/// 响应写不出由 serve_conn 逐流兜底。
/// ctrl 请求的本机路径授权守卫(v2-P5 T2):按臂提取路径参数逐个过
/// [`RootGuard::check`];未配置直通(内部 check 即 Ok)。首个越界路径的
/// 文案即拒因。Orchestrate/CancelJob/JobQuery 无本机路径面(前者在 jobs
/// 面收口),直通。
fn guard_local_paths(
    state: &DaemonState,
    body: Option<&mtdss_proto::ctrl::ctrl_request::Body>,
) -> Result<(), String> {
    use mtdss_proto::ctrl::ctrl_request::Body as Req;
    let Some(body) = body else {
        return Ok(());
    };
    let guard = state.guard();
    // 照片虚拟路径判定(R7):`/照片图库` 前缀(词法规范化后)。
    let photo_path = |p: &str| crate::photo::photo_vpath(p).is_some();
    // 变更面对照片虚拟路径一律拒(只读来源;与 photo_serve 开关无关——
    // 虚拟路径本就不是合法 fs 面,拒绝先于授权守卫给出明确文案)。
    let reject_photo_write = |p: &str| {
        Err(format!("照片图库为只读来源,不支持该操作: {p}"))
    };
    // 服务桥 vpath 判定(V3-4 §4.1):与守卫桥臂同一口径(bridge_route,
    // 词法规范化 + 整组件前缀)。变更面(移/复制/收件清理)一律拒——
    // bridge vpath 是 App 应答式只读来源,非本机路径面;**delete 是唯一
    // 例外**(分发处路由 fs.serve_delete,整树语义在 App 侧 SAF)。
    let serve_path = |p: &str| guard.bridge_route(p).is_some();
    let reject_serve_write = |p: &str| {
        Err(format!("服务桥目录为只读来源,不支持该操作: {p}"))
    };
    match body {
        // List 的空路径 = 根列表(T13b 修#7):授权形态下不按集外拒,由
        // 分发处合成授权根虚拟列表(见 handle_ctrl_request List 臂);
        // 未配置形态本就直通(分发处走既有 fs::root_listing 卷列表)。
        Req::List(r) if r.path.is_empty() => Ok(()),
        // List/Stat 的照片虚拟路径(R7):不经授权守卫(桥可用性即授权,
        // App 只在照片权限授予时应答);photo_serve 关(桌面/Android)落
        // 既有守卫按集外拒(行为逐字节不变)。
        Req::List(r) if photo_path(&r.path) => match state.photo() {
            Some(_) => Ok(()),
            None => guard.check(Path::new(&r.path)),
        },
        Req::Stat(r) if photo_path(&r.path) => match state.photo() {
            Some(_) => Ok(()),
            None => guard.check(Path::new(&r.path)),
        },
        // List/Stat 的服务桥 vpath(V3-4 §4.1):守卫桥臂本就放行(注册即
        // 授权),此处直通——分发处路由服务桥(桥不可用按 NOT_FOUND 诚实
        // 报错,不挂死)。
        Req::List(r) if serve_path(&r.path) => Ok(()),
        Req::Stat(r) if serve_path(&r.path) => Ok(()),
        Req::List(r) => guard.check(Path::new(&r.path)),
        Req::Stat(r) => guard.check(Path::new(&r.path)),
        Req::Delete(r) => {
            // delete 是 bridge vpath 的唯一变更面例外:守卫照常收口(桥臂
            // 放行注册 vpath,fs 臂拒集外),分发处按 serve_path 分流。
            for p in &r.paths {
                if photo_path(p) {
                    reject_photo_write(p)?;
                }
                guard.check(Path::new(p))?;
            }
            Ok(())
        }
        Req::Move(r) => {
            for pair in &r.pairs {
                for p in [&pair.from, &pair.to] {
                    if photo_path(p) {
                        reject_photo_write(p)?;
                    }
                    if serve_path(p) {
                        reject_serve_write(p)?;
                    }
                    guard.check(Path::new(p))?;
                }
            }
            Ok(())
        }
        Req::Copy(r) => {
            for p in &r.paths {
                if photo_path(p) {
                    reject_photo_write(p)?;
                }
                if serve_path(p) {
                    reject_serve_write(p)?;
                }
                guard.check(Path::new(p))?;
            }
            if photo_path(&r.dir) {
                reject_photo_write(&r.dir)?;
            }
            if serve_path(&r.dir) {
                reject_serve_write(&r.dir)?;
            }
            guard.check(Path::new(&r.dir))
        }
        Req::CleanupOutbox(r) => {
            if photo_path(&r.dir) {
                reject_photo_write(&r.dir)?;
            }
            if serve_path(&r.dir) {
                reject_serve_write(&r.dir)?;
            }
            guard.check(Path::new(&r.dir))
        }
        Req::Orchestrate(_)
        | Req::CancelJob(_)
        | Req::JobQuery(_) => Ok(()),
    }
}

async fn handle_ctrl_request(state: Arc<DaemonState>, req: CtrlRequest) -> CtrlResponse {
    use mtdss_proto::ctrl::ctrl_request::Body as Req;
    use mtdss_proto::ctrl::ctrl_response::Body as Resp;
    if req.version != mtdss_proto::CTRL_VERSION {
        return err_response(
            "UNSUPPORTED_VERSION",
            format!(
                "unsupported ctrl version {} (expected {})",
                req.version,
                mtdss_proto::CTRL_VERSION
            ),
        );
    }
    // 授权根守卫(v2-P5 T2,spec §3「授权目录集」):ctrl 各臂解析的
    // **本机**路径(浏览/读/删/移/复制/收件区清理)必须落在授权集内,
    // 集外与 `..` 逃逸一律拒;未配置(桌面默认)直通,行为逐字节不变。
    // 拒绝按 INVALID_PATH 原码回(与既有路径面同码,老发起端零适配);
    // Orchestrate 例外——其源读取在 jobs 面(start_upload_internal)收口,
    // 走 accepted=false 业务拒绝形态,不在此重复报错路径。
    if let Err(msg) = guard_local_paths(&state, req.body.as_ref()) {
        return err_response("INVALID_PATH", msg);
    }
    let result: Result<Resp, mtdss_core::fs::FsError> = match req.body {
        Some(Req::List(r)) if r.path.is_empty() && state.guard().enabled() => {
            // 授权根虚拟列表(2026-11-01 T13b 修#7):授权形态下对端列根
            // 不再 INVALID_PATH 拒——把授权根自身列成目录条目,对端得以
            // 发现收件箱/信任目录并下钻(集内子路径照旧过守卫)。合成条
            // 目路径 = 规范化授权根,`stat` 同路径直达(守卫放行根自身)。
            // 未配置(桌面默认)不进此臂,落到既有 fs::root_listing 卷列
            // 表,行为逐字节不变。合成面逐根 stat + 子项计数(R2 修:
            // 评审 Minor 1——与其他 fs 调用同口径经 blocking_fs 落阻塞线
            // 程池,网络型根(iCloud Drive 等)的 stat/read_dir 可达秒级,
            // 直跑会占死异步 worker,T5b 卷列表 child_count 同款教训)。
            let guard = state.guard().clone();
            let mut listing = match blocking_fs(move || Ok(guard.roots_listing())).await {
                Ok(Some(out)) => out,
                // enabled 形态下 roots_listing 恒 Some;None 防御臂沿既有
                // 回退(根路径 list_dir 语义),行为不变。
                Ok(None) => {
                    let out = blocking_fs(move || mtdss_core::fs::list_dir(&r.path)).await;
                    match out {
                        Ok(out) => out,
                        Err(e) => return err_response(e.code(), e.to_string()),
                    }
                }
                Err(e) => return err_response(e.code(), e.to_string()),
            };
            // 照片图库虚拟根(R7 Ruling P):桥在场即探一次(0x06 列根 =
            // 相册列表,顺带落缓存)——可用 → 根条目(children = 相册数)
            // 与授权根并列;不可用(未授权/超时)→ None,根隐藏(下次访问
            // 自然重探)。
            if let Some(photo) = state.photo() {
                if let Some(entry) = photo.root_listing_entry().await {
                    listing.entries.push(entry);
                    // 目录优先 + 名称字节升序(core sort_entries 同口径;
                    // 与授权根合列后重排一次)
                    listing.entries.sort_by(|a, b| {
                        let dir = b.is_dir.cmp(&a.is_dir);
                        dir.then_with(|| a.name.as_bytes().cmp(b.name.as_bytes()))
                    });
                }
            }
            // 服务桥根可见性(V3-4 §4.1):roots_listing 已合成 visible 桥根
            // 条目(children 惰性 None);桥不可达(App 未应答/超时,桥级
            // 负缓存 TTL 内不重探)→ 桥根隐藏;App 拒答(ok:false)→ 仅该
            // 根隐藏(桥仍在,其余根照常)。photo 根探针同款哲学——对端根
            // 列表不出现死桥根,重试自然重探(每次访问都是新请求)。
            if let Some(serve) = state.serve() {
                let guard = state.guard();
                let mut i = 0;
                while i < listing.entries.len() {
                    let path = listing.entries[i].path.clone();
                    match guard.bridge_route(&path) {
                        Some(hit) => {
                            if serve.root_probe(&hit.root).await {
                                i += 1;
                            } else {
                                listing.entries.remove(i);
                            }
                        }
                        None => i += 1,
                    }
                }
            }
            Ok(Resp::List(mtdss_proto::ctrl::ListResponse {
                entries: listing.entries.iter().map(entry_proto).collect(),
                truncated: listing.truncated,
            }))
        }
        // 照片图库虚拟根下钻列表(R7):`/照片图库[/相册]` → 0x06 桥列表,
        // 条目路径合成完整虚拟路径;桥不可用 → NOT_FOUND(文案携因,对端
        // 原码透传)。
        Some(Req::List(r)) if crate::photo::photo_vpath(&r.path).is_some() => {
            let Some(photo) = state.photo() else {
                return err_response("INVALID_PATH", "照片桥未启用".into());
            };
            let sub = crate::photo::photo_vpath(&r.path).unwrap_or_default();
            match photo.list(&sub).await {
                Ok((entries, truncated)) => {
                    let mut out: Vec<mtdss_core::fs::DirEntryData> = entries
                        .iter()
                        .map(|e| mtdss_core::fs::DirEntryData {
                            name: e.name.clone(),
                            path: crate::photo::join_vpath(&sub, &e.name),
                            size: e.size,
                            mtime_ms: e.mtime_ms,
                            is_dir: e.is_dir,
                            children: e.children,
                        })
                        .collect();
                    // 目录优先 + 名称字节升序(fs sort_entries 同口径)
                    out.sort_by(|a, b| {
                        let dir = b.is_dir.cmp(&a.is_dir);
                        dir.then_with(|| a.name.as_bytes().cmp(b.name.as_bytes()))
                    });
                    Ok(Resp::List(mtdss_proto::ctrl::ListResponse {
                        entries: out.iter().map(entry_proto).collect(),
                        truncated,
                    }))
                }
                Err(m) => Err(mtdss_core::fs::FsError::NotFound(format!(
                    "照片图库不可用: {m}"
                ))),
            }
        }
        // 照片图库单查(R7):缓存命中即回、miss 重列父目录(photo::stat);
        // 根自身 = 合成条目(相册数 children)。
        Some(Req::Stat(r)) if crate::photo::photo_vpath(&r.path).is_some() => {
            let Some(photo) = state.photo() else {
                return err_response("INVALID_PATH", "照片桥未启用".into());
            };
            match photo.stat(&r.path).await {
                Ok(d) => Ok(Resp::Stat(mtdss_proto::ctrl::StatResponse {
                    entry: Some(entry_proto(&d)),
                })),
                Err(e) => Err(e),
            }
        }
        // 列表(V3-4 §4.1 服务桥分区,守卫**单快照**路由——Delete/根列表
        // 臂同款):bridge vpath → 0x08 桥列表,条目路径合成完整 vpath
        // (可直接再下钻/作下载源),缓存不设——每次实问;桥不可用/拒答 →
        // NOT_FOUND(文案携因,对端原码透传,不挂死)。非桥路径落既有 fs
        // 管线,行为不变。**不得**用「match 守卫判桥 + 臂体二次读守卫」的
        // 双读形:roots.set 热换竞窗下二次读可换出 None,expect 即 panic
        // (分发任务死 = 对端挂死,违「不挂死」纪律;评审 Important 修)。
        Some(Req::List(r)) => {
            let guard = state.guard();
            if let Some(hit) = guard.bridge_route(&r.path) {
                let Some(serve) = state.serve() else {
                    return err_response("INVALID_PATH", "服务桥未启用(无 bridge 根注册)".into());
                };
                match serve.list(&hit.root, &hit.sub).await {
                    Ok((entries, truncated)) => {
                        let mut out: Vec<mtdss_core::fs::DirEntryData> = entries
                            .iter()
                            .map(|e| mtdss_core::fs::DirEntryData {
                                name: e.name.clone(),
                                path: crate::serve::join_vpath(&hit.root, &hit.sub, &e.name),
                                size: e.size,
                                mtime_ms: e.mtime_ms,
                                is_dir: e.is_dir,
                                children: e.children,
                            })
                            .collect();
                        // 目录优先 + 名称字节升序(fs sort_entries 同口径)
                        out.sort_by(|a, b| {
                            let dir = b.is_dir.cmp(&a.is_dir);
                            dir.then_with(|| a.name.as_bytes().cmp(b.name.as_bytes()))
                        });
                        Ok(Resp::List(mtdss_proto::ctrl::ListResponse {
                            entries: out.iter().map(entry_proto).collect(),
                            truncated,
                        }))
                    }
                    Err(m) => Err(mtdss_core::fs::FsError::NotFound(format!(
                        "服务目录不可用: {m}"
                    ))),
                }
            } else {
                blocking_fs(move || mtdss_core::fs::list_dir(&r.path))
                    .await
                    .map(|out| {
                        Resp::List(mtdss_proto::ctrl::ListResponse {
                            entries: out.entries.iter().map(entry_proto).collect(),
                            truncated: out.truncated,
                        })
                    })
            }
        }
        // 单查(V3-4 §4.1,单快照路由同上;缓存不设):子路径 = 列父目录
        // 找条目;根自身 = 探活成功即合成(children 惰性 None,与根列表条
        // 目同形)。非桥路径落既有 fs 管线。
        Some(Req::Stat(r)) => {
            let guard = state.guard();
            if let Some(hit) = guard.bridge_route(&r.path) {
                let Some(serve) = state.serve() else {
                    return err_response("INVALID_PATH", "服务桥未启用(无 bridge 根注册)".into());
                };
                match serve.stat(&hit.root, &hit.sub, &hit.name).await {
                    Ok(d) => Ok(Resp::Stat(mtdss_proto::ctrl::StatResponse {
                        entry: Some(entry_proto(&d)),
                    })),
                    Err(e) => Err(e),
                }
            } else {
                blocking_fs(move || mtdss_core::fs::stat_path(&r.path))
                    .await
                    .map(|d| {
                        Resp::Stat(mtdss_proto::ctrl::StatResponse {
                            entry: Some(entry_proto(&d)),
                        })
                    })
            }
        }
        // 删除(V3-4 §4.1 服务桥分区):bridge vpath → fs.serve_delete(App
        // 侧 SAF **递归整树删**,一次调用;真路径照旧 fs::delete_paths)。
        // 批不半途弃废(逐条失败表随响应);桥删恒永久删(SAF 无回收设
        // 施,recycled 只由 fs 面贡献)。
        Some(Req::Delete(r)) => {
            let mut bridge: Vec<(String, crate::roots::BridgeHit)> = Vec::new();
            let mut fs_paths: Vec<String> = Vec::new();
            {
                let guard = state.guard();
                for p in &r.paths {
                    match guard.bridge_route(p) {
                        Some(hit) => bridge.push((p.clone(), hit)),
                        None => fs_paths.push(p.clone()),
                    }
                }
            }
            let mut deleted: usize = 0;
            let mut recycled = false;
            let mut failed: Vec<mtdss_core::fs::PathOpFailure> = Vec::new();
            if !bridge.is_empty() {
                let Some(serve) = state.serve() else {
                    return err_response("INVALID_PATH", "服务桥未启用(无 bridge 根注册)".into());
                };
                for (raw, hit) in &bridge {
                    match serve.delete(&hit.root, &hit.sub).await {
                        Ok(true) => deleted += 1,
                        Ok(false) => failed.push(mtdss_core::fs::PathOpFailure {
                            path: raw.clone(),
                            code: "NOT_FOUND".into(),
                            message: "服务目录不存在或已删除".into(),
                        }),
                        Err(crate::serve::ServeErr::Refused(m)) => {
                            failed.push(mtdss_core::fs::PathOpFailure {
                                path: raw.clone(),
                                code: "PERMISSION_DENIED".into(),
                                message: format!("App 删除被拒: {m}"),
                            })
                        }
                        Err(e) => failed.push(mtdss_core::fs::PathOpFailure {
                            path: raw.clone(),
                            code: "INTERNAL".into(),
                            message: e.to_string(),
                        }),
                    }
                }
            }
            if !fs_paths.is_empty() {
                let force = r.force_permanent;
                match blocking_fs(move || mtdss_core::fs::delete_paths(&fs_paths, force)).await {
                    Ok((d, rec, f)) => {
                        deleted += d;
                        recycled |= rec;
                        failed.extend(f);
                    }
                    // 设施级批失败(回收不可用等)沿既有整批错误口径上抛
                    Err(e) => return err_response(e.code(), e.to_string()),
                }
            }
            Ok(Resp::Delete(proto_delete_response(deleted, recycled, &failed)))
        }
        Some(Req::Move(r)) => {
            // policy 缺省(旧端请求)= 空策略 = legacy(move_entries 语义)
            let policy = r.policy.as_ref().map(policy_engine).unwrap_or_default();
            // 策略上限与发起端对称(移交债③):远端来件同样不得超限
            // (合计条数/单条字节),超限 POLICY_LIMIT,fs 变更零发生。
            if let Err(m) = check_policy_limits(&policy) {
                return err_response("POLICY_LIMIT", m);
            }
            blocking_fs(move || {
                let pairs: Vec<(PathBuf, PathBuf)> = r
                    .pairs
                    .into_iter()
                    .map(|p| (PathBuf::from(p.from), PathBuf::from(p.to)))
                    .collect();
                mtdss_core::fs::move_entries_with(&pairs, &policy)
            })
            .await
            .map(|out| Resp::Move(proto_move_response(&out)))
        }
        Some(Req::Copy(r)) => {
            // policy 缺省(旧端请求)= 空策略 = legacy(copy_paths 语义)
            let policy = r.policy.as_ref().map(policy_engine).unwrap_or_default();
            // 策略上限与发起端对称(移交债③),同 Move 臂。
            if let Err(m) = check_policy_limits(&policy) {
                return err_response("POLICY_LIMIT", m);
            }
            blocking_fs(move || {
                let paths: Vec<PathBuf> = r.paths.iter().map(PathBuf::from).collect();
                let dir = PathBuf::from(&r.dir);
                mtdss_core::fs::copy_paths_with(&paths, &dir, &policy)
            })
            .await
            .map(|out| Resp::Copy(proto_copy_response(&out)))
        }
        // 编排(Task 7):源端接受 = 起回显 job_id 的 upload 作业(ConnectRequest
        // 随之带 id 回操作端)。拒绝(路径/目录/策略/未登录/id 撞号)不是
        // 通道错误:accepted=false + detail 原样带回,操作端落作业 failed。
        // old_rels 非空 = 重发编排(v2-P2 Task 8):partial 预检 + dropped
        // 由源端统一清接收端残留,经 accept_orchestration 透传。
        Some(Req::Orchestrate(r)) => {
            let policy = r.policy.as_ref().map(policy_engine).unwrap_or_default();
            match state
                .jobs()
                .accept_orchestration(&state, &r.job_id, &r.to_device, &r.paths, &r.target_dir, policy, &r.old_rels, &r.display_name)
                .await
            {
                Ok(()) => Ok(Resp::Orchestrate(mtdss_proto::ctrl::OrchestrateResponse {
                    accepted: true,
                    detail: String::new(),
                })),
                Err(m) => Ok(Resp::Orchestrate(mtdss_proto::ctrl::OrchestrateResponse {
                    accepted: false,
                    detail: m,
                })),
            }
        }
        Some(Req::CancelJob(r)) => Ok(Resp::CancelJob(mtdss_proto::ctrl::CancelJobResponse {
            cancelled: state.jobs().cancel(&r.job_id),
        })),
        Some(Req::JobQuery(r)) => {
            let snap = state.jobs().snapshot();
            let rels = state.jobs().job_rels(&r.job_id);
            Ok(Resp::JobQuery(job_query_response(
                snap.iter().find(|j| j.id == r.job_id),
                rels,
            )))
        }
        // 残留清理(spec §5.3):dir 绝对形态校验同 list/stat 口径(NUL/
        // 相对形态 → INVALID_PATH 原码透传;Windows 裸盘符规范化)——规范化
        // 后才落 `.mtdss` 拼接,杜绝 `C:` + join 的盘符相对歧义;执行面是
        // intake::cleanup_rels(身份吻合幂等清,只动隐藏区)。
        Some(Req::CleanupOutbox(r)) => blocking_fs(move || {
            let dir = mtdss_core::fs::normalize(&r.dir)?;
            Ok(Resp::CleanupOutbox(mtdss_proto::ctrl::CleanupOutboxResponse {
                cleaned: mtdss_core::transport::intake::cleanup_rels(
                    &mtdss_core::transport::intake::FsDestIo::new(&dir),
                    &r.rels,
                ),
            }))
        })
        .await,
        None => Err(mtdss_core::fs::FsError::Internal("ctrl 请求缺 body".into())),
    };
    match result {
        Ok(body) => CtrlResponse {
            version: mtdss_proto::CTRL_VERSION,
            ok: true,
            body: Some(body),
        },
        Err(e) => err_response(e.code(), e.to_string()),
    }
}

/// 错误响应(ok=false,CtrlError{code,message};version 回本端版本,
/// 发起端 send_request 按版本比对即可见新端/旧端差异)。
fn err_response(code: &str, message: String) -> CtrlResponse {
    CtrlResponse {
        version: mtdss_proto::CTRL_VERSION,
        ok: false,
        body: Some(mtdss_proto::ctrl::ctrl_response::Body::Error(
            mtdss_proto::ctrl::CtrlError {
                code: code.to_string(),
                message,
            },
        )),
    }
}

/// 数据侧 DirEntryData → proto DirEntry(逐字段拷贝;children 仅目录项
/// 在场,文件/未知缺省 None 线上即缺省;包裹消息携 count)。
fn entry_proto(d: &mtdss_core::fs::DirEntryData) -> mtdss_proto::ctrl::DirEntry {
    mtdss_proto::ctrl::DirEntry {
        name: d.name.clone(),
        path: d.path.clone(),
        size: d.size,
        mtime_ms: d.mtime_ms,
        is_dir: d.is_dir,
        children: d.children.map(|count| mtdss_proto::ctrl::DirChildren { count }),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    // ---------- ctrl 调优值(V3-5 Task 1)----------
    //
    // 直连心跳默认开 / 中继路保持无心跳:值是长连接保活行为与服务器配额
    // 防线共同的锚,改值必须显式过本测试(暖池语义见模块头)。

    /// 直连调优 15s 心跳 + 60s 空闲上界 + 直连 MTU;中继调优绝不心跳
    /// (配额)+ 中继 MTU 1400。
    #[test]
    fn direct_tuning_heartbeat_on_relay_off() {
        let d = ctrl_default_tuning();
        assert_eq!(d.keep_alive_interval, Some(Duration::from_secs(15)));
        assert_eq!(d.max_idle_timeout, Duration::from_secs(60));
        assert_eq!(d.max_mtu, 1452);
        let r = ctrl_relay_tuning();
        assert_eq!(r.keep_alive_interval, None, "中继连接绝不心跳保活(配额)");
        assert_eq!(r.max_mtu, 1400);
    }

    /// 互拨段分流两臂(V3-5 Task 2):`allow_relay=true` 走
    /// connect-with-fallback(现状,请求路径——直连穷尽且资格在手走服务端
    /// 中继);`false` 纯直连(预热路径——打不通即失败,绝不烧服务端中继)。
    /// dial_session 编舞进程内不可直测(打洞栈不可达),以本纯函数钉分流
    /// 判定;请求路径行为回归由既有 ctrl_e2e 套件钉。
    #[test]
    fn punch_selector_pins_both_arms() {
        assert_eq!(punch_selector(true), PunchMode::WithFallback);
        assert_eq!(punch_selector(false), PunchMode::DirectOnly);
    }

    /// 测试态:独立临时 home(版本不符/缺 body/job 分支不触盘,home 生命
    /// 周期到函数尾即弃)。
    fn test_state() -> Arc<DaemonState> {
        let home = tempfile::tempdir().unwrap();
        let (events, _) = tokio::sync::broadcast::channel(16);
        // 更新服务(Task 11):缺省失败安全配置(.invalid/无钥),ctrl 测试
        // 不触更新面;非 Windows 为单元占位。
        #[cfg(windows)]
        let update = std::sync::Arc::new(crate::update::UpdateService::new(
            crate::update::UpdateConfig::default(),
            events.clone(),
            tokio::sync::watch::channel(false).0,
        ));
        #[cfg(not(windows))]
        let update = ();
        Arc::new(
            DaemonState::load(
                home.path(),
                home.path().join("inbox"),
                None,
                crate::inbox_dest::staging_dir(home.path()),
                // 测试台默认无发布面(桌面;哨兵回退语义)
                false,
                // 测试态无照片桥(R7;照片臂整体不进,行为不变)
                false,
                Default::default(),
                Default::default(),
                ctrl_default_tuning(),
                events,
                update,
            )
            .unwrap(),
        )
    }

    /// 手造 v1 请求仍得结构化 UNSUPPORTED_VERSION(版本不符不静默/不悬挂;
    /// v2 升版后老发起端语义保持——错误响应 version 回本端 v2,发起端
    /// send_request 按版本比对即可见新端)。e2e 无法在测试进程内手造
    /// ctrl 连接(打洞栈不可达),分发入口直测即钉住该行为。
    #[tokio::test]
    async fn version_mismatch_still_reports_unsupported() {
        let resp = handle_ctrl_request(
            test_state(),
            CtrlRequest {
                version: 1,
                body: Some(mtdss_proto::ctrl::ctrl_request::Body::List(
                    mtdss_proto::ctrl::ListRequest {
                        path: "C:\\".into(),
                    },
                )),
            },
        )
        .await;
        assert!(!resp.ok, "版本不符必须 ok=false: {resp:?}");
        assert_eq!(resp.version, mtdss_proto::CTRL_VERSION);
        match resp.body {
            Some(mtdss_proto::ctrl::ctrl_response::Body::Error(e)) => {
                assert_eq!(e.code, "UNSUPPORTED_VERSION");
                assert!(e.message.contains("1"), "message 携带来件版本: {e:?}");
            }
            other => panic!("expected error body, got {other:?}"),
        }
    }

    /// 缺 body 的防御分支:INTERNAL 错误响应(ok=false)。
    #[tokio::test]
    async fn missing_body_is_internal_error() {
        let resp = handle_ctrl_request(
            test_state(),
            CtrlRequest {
                version: mtdss_proto::CTRL_VERSION,
                body: None,
            },
        )
        .await;
        assert!(!resp.ok);
        match resp.body {
            Some(mtdss_proto::ctrl::ctrl_response::Body::Error(e)) => {
                assert_eq!(e.code, "INTERNAL");
            }
            other => panic!("expected error body, got {other:?}"),
        }
    }

    // ---------- 授权根守卫(v2-P5 T2,spec §3)----------
    //
    // 浏览/读/操作面的收口矩阵:授权根 [A, B] 注入后,A 内(含深层子路径)
    // 放行;集外 C 与 `..` 逃逸按 INVALID_PATH 拒;空路径(根列表)在授权
    // 形态合成虚拟授权根列表(2026-11-01 T13b 修#7),未配置走既有卷列
    // 表。未配置(桌面默认)对任意本机路径直通。分发入口直测(e2e 无法
    // 在测试进程内手造 ctrl 连接,同 version_mismatch 先例)。

    /// 授权根测试态:独立临时 home,授权根 = home 下 docs/trusted 两个
    /// 真实目录(secret 为集外对照);返回 (state, TempDir 保活)。
    fn test_state_with_roots() -> (Arc<DaemonState>, tempfile::TempDir) {
        let home = tempfile::tempdir().unwrap();
        for d in ["docs", "trusted", "secret"] {
            std::fs::create_dir_all(home.path().join(d)).unwrap();
        }
        let (events, _) = tokio::sync::broadcast::channel(16);
        #[cfg(windows)]
        let update = std::sync::Arc::new(crate::update::UpdateService::new(
            crate::update::UpdateConfig::default(),
            events.clone(),
            tokio::sync::watch::channel(false).0,
        ));
        #[cfg(not(windows))]
        let update = ();
        let roots = vec![
            home.path().join("docs"),
            home.path().join("trusted"),
        ];
        let state = DaemonState::load(
            home.path(),
            home.path().join("inbox"),
            Some(roots),
            crate::inbox_dest::staging_dir(home.path()),
            // 测试台默认无发布面(桌面;哨兵回退语义)
            false,
            // 测试态无照片桥(R7;照片臂整体不进,行为不变)
            false,
            Default::default(),
            Default::default(),
            ctrl_default_tuning(),
            events,
            update,
        )
        .unwrap();
        (Arc::new(state), home)
    }

    /// 授权根内 List 放行:根自身与深层子路径都可达(列表真实成功)。
    #[tokio::test]
    async fn roots_guard_allows_list_inside_roots() {
        let (state, home) = test_state_with_roots();
        let docs = home.path().join("docs");
        std::fs::write(docs.join("f.txt"), b"x").unwrap();
        let sub = docs.join("sub");
        std::fs::create_dir_all(&sub).unwrap();
        for path in [docs.clone(), sub] {
            let resp = handle_ctrl_request(
                state.clone(),
                CtrlRequest {
                    version: mtdss_proto::CTRL_VERSION,
                    body: Some(mtdss_proto::ctrl::ctrl_request::Body::List(
                        mtdss_proto::ctrl::ListRequest {
                            path: path.to_string_lossy().into_owned(),
                        },
                    )),
                },
            )
            .await;
            assert!(resp.ok, "授权根内浏览应放行: {path:?} → {resp:?}");
        }
    }

    /// 集外路径与 `..` 逃逸拒绝:INVALID_PATH 原码 + 授权文案。空路径
    /// (根列表)例外——授权形态合成虚拟根列表,见
    /// `roots_guard_root_listing_synthesizes_authorized_roots`。
    #[tokio::test]
    async fn roots_guard_rejects_outside_and_escape() {
        let (state, home) = test_state_with_roots();
        let docs = home.path().join("docs");
        let cases: Vec<String> = vec![
            // 集外对照目录(home 的子目录,但不在授权集内)
            home.path().join("secret").to_string_lossy().into_owned(),
            // home 自身:是授权根的父目录,不因「含授权根」而放行
            home.path().to_string_lossy().into_owned(),
            // `..` 逃逸:形似集内,词法规范化后落集外
            docs.join("../../../etc").to_string_lossy().into_owned(),
        ];
        for path in cases {
            let resp = handle_ctrl_request(
                state.clone(),
                CtrlRequest {
                    version: mtdss_proto::CTRL_VERSION,
                    body: Some(mtdss_proto::ctrl::ctrl_request::Body::List(
                        mtdss_proto::ctrl::ListRequest { path: path.clone() },
                    )),
                },
            )
            .await;
            assert!(!resp.ok, "路径 {path:?} 应被守卫拒绝");
            match resp.body {
                Some(mtdss_proto::ctrl::ctrl_response::Body::Error(e)) => {
                    assert_eq!(e.code, "INVALID_PATH", "路径 {path:?} 原码不符: {e:?}");
                    assert!(
                        e.message.contains("授权"),
                        "路径 {path:?} 文案应点名授权集: {e:?}"
                    );
                }
                other => panic!("路径 {path:?} 应得错误体, got {other:?}"),
            }
        }
    }

    /// 根列表(2026-11-01 T13b 修#7):授权形态下对端列根("")= 授权根
    /// 虚拟列表——恰为授权根集的目录条目(name = 末段真实目录名、path =
    /// 规范化根、children 按 T5b 计);根条目 `stat` 同路径可达;集外拒绝
    /// 语义不变(见 reject 用例);未配置(桌面默认)走既有卷列表,行为
    /// 逐字节不变。
    #[tokio::test]
    async fn roots_guard_root_listing_synthesizes_authorized_roots() {
        let (state, home) = test_state_with_roots();
        let docs = home.path().join("docs");
        let trusted = home.path().join("trusted");
        // 根内放真实子项:docs = 1 文件 + 1 目录;trusted 空 → children 0
        std::fs::write(docs.join("f.txt"), b"x").unwrap();
        std::fs::create_dir_all(docs.join("sub")).unwrap();

        use mtdss_proto::ctrl::ctrl_response::Body as Resp;
        let resp = handle_ctrl_request(
            state.clone(),
            CtrlRequest {
                version: mtdss_proto::CTRL_VERSION,
                body: Some(mtdss_proto::ctrl::ctrl_request::Body::List(
                    mtdss_proto::ctrl::ListRequest { path: String::new() },
                )),
            },
        )
        .await;
        assert!(resp.ok, "授权形态列根应放行(虚拟根列表): {resp:?}");
        match resp.body {
            Some(Resp::List(r)) => {
                assert!(!r.truncated);
                let got: Vec<(String, String, bool, Option<u32>)> = r
                    .entries
                    .iter()
                    .map(|e| {
                        (
                            e.name.clone(),
                            e.path.clone(),
                            e.is_dir,
                            e.children.as_ref().map(|c| c.count),
                        )
                    })
                    .collect();
                let docs_s = docs.to_string_lossy().into_owned();
                let trusted_s = trusted.to_string_lossy().into_owned();
                assert_eq!(
                    got,
                    vec![
                        ("docs".into(), docs_s.clone(), true, Some(2)),
                        ("trusted".into(), trusted_s.clone(), true, Some(0)),
                    ],
                    "根列表 = 授权根自身(名称字节升序,T5b 子项数)"
                );
            }
            other => panic!("列根应得 List 体, got {other:?}"),
        }

        // 合成根条目的 path 可直接 stat(守卫放行根自身)
        let resp = handle_ctrl_request(
            state.clone(),
            CtrlRequest {
                version: mtdss_proto::CTRL_VERSION,
                body: Some(mtdss_proto::ctrl::ctrl_request::Body::Stat(
                    mtdss_proto::ctrl::StatRequest {
                        path: docs.to_string_lossy().into_owned(),
                    },
                )),
            },
        )
        .await;
        assert!(resp.ok, "根条目 stat 应放行: {resp:?}");

        // 未配置(桌面默认)列根 = 既有卷列表语义,不进虚拟根臂
        let home2 = tempfile::tempdir().unwrap();
        let (events, _) = tokio::sync::broadcast::channel(16);
        #[cfg(windows)]
        let update = std::sync::Arc::new(crate::update::UpdateService::new(
            crate::update::UpdateConfig::default(),
            events.clone(),
            tokio::sync::watch::channel(false).0,
        ));
        #[cfg(not(windows))]
        let update = ();
        let plain = DaemonState::load(
            home2.path(),
            home2.path().join("inbox"),
            None,
            crate::inbox_dest::staging_dir(home2.path()),
            false,
            // 测试态无照片桥(R7)
            false,
            Default::default(),
            Default::default(),
            ctrl_default_tuning(),
            events,
            update,
        )
        .unwrap();
        let resp = handle_ctrl_request(
            Arc::new(plain),
            CtrlRequest {
                version: mtdss_proto::CTRL_VERSION,
                body: Some(mtdss_proto::ctrl::ctrl_request::Body::List(
                    mtdss_proto::ctrl::ListRequest { path: String::new() },
                )),
            },
        )
        .await;
        assert!(resp.ok, "未配置列根应走既有卷列表: {resp:?}");
        match resp.body {
            Some(Resp::List(r)) => {
                let names: Vec<String> = r.entries.iter().map(|e| e.name.clone()).collect();
                #[cfg(unix)]
                assert_eq!(names, vec!["/"], "unix 卷列表语义不变");
                #[cfg(windows)]
                assert!(!names.is_empty(), "windows 卷列表非空即可(盘符探测)");
            }
            other => panic!("未配置列根应得 List 体, got {other:?}"),
        }
    }

    /// roots.set 热生效(state 级,V3-4 §4.1):`set_roots` 整根换守卫
    /// ——构造注入集(docs/trusted)被新集(secret)整体替换,旧根立即
    /// 拒、新根立即放(热生效,无重启窗);空集回直通(列根回既有卷列
    /// 表,任意路径放行)。对端视图(handle_ctrl_request 直测,同
    /// roots_guard_* 先例)。
    #[tokio::test]
    async fn roots_set_hot_swap_via_state() {
        use crate::roots::{RootSource, RootSpec};
        let (state, home) = test_state_with_roots();
        let list_req = |state: Arc<DaemonState>, path: String| async move {
            handle_ctrl_request(
                state,
                CtrlRequest {
                    version: mtdss_proto::CTRL_VERSION,
                    body: Some(mtdss_proto::ctrl::ctrl_request::Body::List(
                        mtdss_proto::ctrl::ListRequest { path },
                    )),
                },
            )
            .await
        };
        // 换新集:单根 secret
        state.set_roots(vec![RootSpec {
            name: "secret".into(),
            path: home.path().join("secret").to_string_lossy().into_owned(),
            source: RootSource::Fs,
            visible: true,
        }]);
        let docs = home.path().join("docs").to_string_lossy().into_owned();
        let resp = list_req(state.clone(), docs).await;
        assert!(!resp.ok, "换守卫后旧根应立即拒: {resp:?}");
        match resp.body {
            Some(mtdss_proto::ctrl::ctrl_response::Body::Error(e)) => {
                assert_eq!(e.code, "INVALID_PATH");
            }
            other => panic!("旧根应得错误体, got {other:?}"),
        }
        let resp = list_req(state.clone(), String::new()).await;
        assert!(resp.ok, "新集列根应放行: {resp:?}");
        match resp.body {
            Some(mtdss_proto::ctrl::ctrl_response::Body::List(r)) => {
                let paths: Vec<String> = r.entries.iter().map(|e| e.path.clone()).collect();
                assert_eq!(paths.len(), 1, "新集恰一根: {paths:?}");
                assert!(paths[0].contains("secret"), "新集条目应为新根: {paths:?}");
            }
            other => panic!("列根应得 List 体, got {other:?}"),
        }
        // 空集 → 直通:列根回卷列表,旧根放行(守卫不在场)
        state.set_roots(vec![]);
        let docs = home.path().join("docs").to_string_lossy().into_owned();
        let resp = list_req(state.clone(), docs).await;
        assert!(resp.ok, "空集直通后任意路径放行: {resp:?}");
        let resp = list_req(state, String::new()).await;
        match resp.body {
            Some(mtdss_proto::ctrl::ctrl_response::Body::List(r)) => {
                #[cfg(unix)]
                assert_eq!(r.entries.len(), 1, "unix 卷列表单根 /");
                #[cfg(windows)]
                assert!(!r.entries.is_empty(), "windows 卷列表非空即可(盘符探测)");
            }
            other => panic!("空集列根应得 List 体, got {other:?}"),
        }
    }

    /// 操作面同口径:Delete 集外 / Move 对端越集 / Copy 落盘目录越集 /
    /// CleanupOutbox 越集,一律 INVALID_PATH;集内 Delete 放行(桌面
    /// cut 收尾语义在授权形态下仍可达)。
    #[tokio::test]
    async fn roots_guard_covers_mutating_arms() {
        let (state, home) = test_state_with_roots();
        let docs = home.path().join("docs");
        let secret = home.path().join("secret");
        let docs_s = docs.to_string_lossy().into_owned();
        let secret_s = secret.to_string_lossy().into_owned();
        let req = |body: mtdss_proto::ctrl::ctrl_request::Body| CtrlRequest {
            version: mtdss_proto::CTRL_VERSION,
            body: Some(body),
        };
        // Delete:集内放行(列表面成功;目标不存在是逐条 NOT_FOUND,不
        // 触守卫拒),集外 INVALID_PATH
        let resp = handle_ctrl_request(
            state.clone(),
            req(mtdss_proto::ctrl::ctrl_request::Body::Delete(
                mtdss_proto::ctrl::DeleteRequest {
                    paths: vec![docs_s.clone()],
                    force_permanent: false,
                },
            )),
        )
        .await;
        assert!(resp.ok, "集内 Delete 不应被守卫拦截: {resp:?}");
        let resp = handle_ctrl_request(
            state.clone(),
            req(mtdss_proto::ctrl::ctrl_request::Body::Delete(
                mtdss_proto::ctrl::DeleteRequest {
                    paths: vec![secret_s.clone()],
                    force_permanent: false,
                },
            )),
        )
        .await;
        assert!(!resp.ok, "集外 Delete 应被守卫拒绝");
        // Move:from 集内、to 集外 → 拒(to 也在守卫面)
        let resp = handle_ctrl_request(
            state.clone(),
            req(mtdss_proto::ctrl::ctrl_request::Body::Move(
                mtdss_proto::ctrl::MoveRequest {
                    pairs: vec![mtdss_proto::ctrl::MovePair {
                        from: docs_s.clone(),
                        to: secret_s.clone(),
                    }],
                    policy: None,
                },
            )),
        )
        .await;
        assert!(!resp.ok, "Move 越集应被守卫拒绝");
        // Copy:源集内、落盘目录集外 → 拒
        let resp = handle_ctrl_request(
            state.clone(),
            req(mtdss_proto::ctrl::ctrl_request::Body::Copy(
                mtdss_proto::ctrl::CopyRequest {
                    paths: vec![docs_s.clone()],
                    dir: secret_s.clone(),
                    policy: None,
                },
            )),
        )
        .await;
        assert!(!resp.ok, "Copy 落盘越集应被守卫拒绝");
        // CleanupOutbox:dir 集外 → 拒
        let resp = handle_ctrl_request(
            state.clone(),
            req(mtdss_proto::ctrl::ctrl_request::Body::CleanupOutbox(
                mtdss_proto::ctrl::CleanupOutboxRequest {
                    dir: secret_s,
                    rels: vec![],
                },
            )),
        )
        .await;
        assert!(!resp.ok, "CleanupOutbox 越集应被守卫拒绝");
    }

    /// 未配置(桌面默认)= 守卫直通:对任意本机路径的浏览保持既有行为
    /// (真实列目录成功,而非 INVALID_PATH)。
    #[tokio::test]
    async fn roots_unconfigured_keeps_desktop_passthrough() {
        let state = test_state();
        let home = tempfile::tempdir().unwrap();
        std::fs::write(home.path().join("f.txt"), b"x").unwrap();
        let resp = handle_ctrl_request(
            state,
            CtrlRequest {
                version: mtdss_proto::CTRL_VERSION,
                body: Some(mtdss_proto::ctrl::ctrl_request::Body::List(
                    mtdss_proto::ctrl::ListRequest {
                        path: home.path().to_string_lossy().into_owned(),
                    },
                )),
            },
        )
        .await;
        assert!(resp.ok, "未配置守卫时浏览应直通: {resp:?}");
    }

    /// 编排拒绝(Task 7):缺 job_id 的 orchestrate 回 accepted=false +
    /// detail(ok=true,拒绝是业务语义不是通道错误),且不在注册表留作业。
    #[tokio::test]
    async fn orchestrate_missing_job_id_rejected() {
        let state = test_state();
        let resp = handle_ctrl_request(
            state.clone(),
            CtrlRequest {
                version: mtdss_proto::CTRL_VERSION,
                body: Some(mtdss_proto::ctrl::ctrl_request::Body::Orchestrate(
                    mtdss_proto::ctrl::OrchestrateRequest {
                        job_id: String::new(),
                        to_device: "peer".into(),
                        paths: vec!["C:\\x".into()],
                        target_dir: "C:\\dst".into(),
                        policy: None,
                        old_rels: Vec::new(),
                        display_name: String::new(),
                    },
                )),
            },
        )
        .await;
        assert!(resp.ok);
        match resp.body {
            Some(mtdss_proto::ctrl::ctrl_response::Body::Orchestrate(r)) => {
                assert!(!r.accepted, "缺 job_id 必须拒绝: {r:?}");
                assert!(r.detail.contains("job_id"), "detail 携带原因: {r:?}");
            }
            other => panic!("expected orchestrate body, got {other:?}"),
        }
        assert!(state.jobs().snapshot().is_empty(), "拒绝不得留作业");
    }

    /// 编排代表名收口(V3-1 §6,ctrl responder 消费面):OrchestrateRequest
    /// 携名(非空)→ 源端回显卡采纳线名;空名(旧版对端的线上形态,prost
    /// 缺省解码即空)→ 回退现状 = 按代表名规则自算,受理不炸。
    #[tokio::test]
    async fn orchestrate_display_name_adopted_or_legacy_fallback() {
        let home = tempfile::tempdir().unwrap();
        let state = {
            // 编排回显走 start_upload 本体(须已登录):测试态补一个会话
            let st = test_state();
            st.login(crate::session::Session {
                server: "http://test".into(),
                jwt: "jwt".into(),
                refresh: "refresh".into(),
                device_id: "srcB".into(),
                ca: None,
            })
            .unwrap();
            st
        };
        let (_keep, f) = {
            let f = home.path().join("报告.pdf");
            std::fs::write(&f, b"data").unwrap();
            (home, f)
        };
        let f_str = f.to_string_lossy().into_owned();
        let dir = if cfg!(windows) { "C:\\dst".to_string() } else { "/dst".to_string() };
        let mk = |job_id: &str, display_name: &str| CtrlRequest {
            version: mtdss_proto::CTRL_VERSION,
            body: Some(mtdss_proto::ctrl::ctrl_request::Body::Orchestrate(
                mtdss_proto::ctrl::OrchestrateRequest {
                    job_id: job_id.into(),
                    to_device: "opA".into(),
                    paths: vec![f_str.clone()],
                    target_dir: dir.clone(),
                    policy: None,
                    old_rels: Vec::new(),
                    display_name: display_name.into(),
                },
            )),
        };

        // 新端携名:accepted + 回显卡同名
        let resp = handle_ctrl_request(state.clone(), mk("opA:1", "报告.pdf 等 3 项")).await;
        assert!(resp.ok);
        match resp.body {
            Some(mtdss_proto::ctrl::ctrl_response::Body::Orchestrate(r)) => {
                assert!(r.accepted, "携名编排必须受理: {r:?}");
            }
            other => panic!("expected orchestrate body, got {other:?}"),
        }
        let snap = state.jobs().snapshot().into_iter().find(|s| s.id == "opA:1").unwrap();
        assert_eq!(snap.name, "报告.pdf 等 3 项", "回显卡必须采纳线名: {}", snap.name);

        // 旧端空名:受理不炸,回显卡回退现状(规则自算)
        let resp = handle_ctrl_request(state, mk("opC:1", "")).await;
        assert!(resp.ok);
        match resp.body {
            Some(mtdss_proto::ctrl::ctrl_response::Body::Orchestrate(r)) => {
                assert!(r.accepted, "旧端空名编排必须照常受理: {r:?}");
            }
            other => panic!("expected orchestrate body, got {other:?}"),
        }
    }

    /// 作业查询/取消(Task 7):missing 作业回 state="missing" + error 文案;
    /// cancel 不存在的作业回 cancelled=false。两分支都不经网络。
    #[tokio::test]
    async fn job_query_and_cancel_missing() {
        let state = test_state();
        let resp = handle_ctrl_request(
            state.clone(),
            CtrlRequest {
                version: mtdss_proto::CTRL_VERSION,
                body: Some(mtdss_proto::ctrl::ctrl_request::Body::JobQuery(
                    mtdss_proto::ctrl::JobQueryRequest {
                        job_id: "9".into(),
                    },
                )),
            },
        )
        .await;
        assert!(resp.ok);
        match resp.body {
            Some(mtdss_proto::ctrl::ctrl_response::Body::JobQuery(r)) => {
                assert_eq!(r.state, "missing");
                assert!(!r.error.is_empty());
            }
            other => panic!("expected job_query body, got {other:?}"),
        }
        let resp = handle_ctrl_request(
            state,
            CtrlRequest {
                version: mtdss_proto::CTRL_VERSION,
                body: Some(mtdss_proto::ctrl::ctrl_request::Body::CancelJob(
                    mtdss_proto::ctrl::CancelJobRequest {
                        job_id: "9".into(),
                    },
                )),
            },
        )
        .await;
        assert!(resp.ok);
        match resp.body {
            Some(mtdss_proto::ctrl::ctrl_response::Body::CancelJob(r)) => {
                assert!(!r.cancelled);
            }
            other => panic!("expected cancel_job body, got {other:?}"),
        }
    }

    /// 作业查询回填 rels(v2-P2 Task 8,spec §4.1):JobQueryResponse.rels
    /// = 源端作业的预扫 rel 全集(upload 含编排回显的 meta.rels;download/
    /// remote_copy 恒空集),操作端轮询据此写回 store 作重发原料。
    #[tokio::test]
    async fn job_query_returns_source_job_rels() {
        // 已登录态(start_upload 前置)——独立 home,与 test_state 同构
        let home = tempfile::tempdir().unwrap();
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
            None,
            crate::inbox_dest::staging_dir(home.path()),
            // 测试台默认无发布面(桌面;哨兵回退语义)
            false,
            // 测试态无照片桥(R7;照片臂整体不进,行为不变)
            false,
            Default::default(),
            Default::default(),
            ctrl_default_tuning(),
            events,
            update,
        )
        .unwrap();
        state
            .login(crate::session::Session {
                server: "http://test".into(),
                jwt: "jwt".into(),
                refresh: "refresh".into(),
                device_id: "srcB".into(),
                ca: None,
            })
            .unwrap();
        let state = Arc::new(state);

        // upload 作业(预扫 rel = ["a.bin"])在场
        let f = home.path().join("a.bin");
        std::fs::write(&f, b"data").unwrap();
        state
            .jobs()
            .start_upload(&state, vec![f], "peerA", "C:\\dst", Default::default(), false)
            .await
            .unwrap();

        let resp = handle_ctrl_request(
            state,
            CtrlRequest {
                version: mtdss_proto::CTRL_VERSION,
                body: Some(mtdss_proto::ctrl::ctrl_request::Body::JobQuery(
                    mtdss_proto::ctrl::JobQueryRequest { job_id: "1".into() },
                )),
            },
        )
        .await;
        assert!(resp.ok);
        match resp.body {
            Some(mtdss_proto::ctrl::ctrl_response::Body::JobQuery(r)) => {
                assert_eq!(r.rels, vec!["a.bin".to_string()], "回填 rels = 预扫全集: {r:?}");
            }
            other => panic!("expected job_query body, got {other:?}"),
        }
    }

    /// 响应端策略上限(移交债③,与发起端 check_policy_limits 对称):
    /// 远端来件 Move/Copy 携带超限策略(skip 2049 条 > 上限 2048)→
    /// ok=false + code=POLICY_LIMIT,且任何 fs 变都不发生(源件在位、
    /// 目标目录零改动);上限边界 2048 条放行,move 照常落盘。
    #[tokio::test]
    async fn responder_rejects_oversized_policy() {
        let tmp = tempfile::tempdir().unwrap();
        let src = tmp.path().join("src");
        let dst = tmp.path().join("dst");
        std::fs::create_dir(&src).unwrap();
        std::fs::create_dir(&dst).unwrap();
        std::fs::write(src.join("a.txt"), b"hello").unwrap();
        let oversized = |n: usize| mtdss_proto::ctrl::TransferPolicy {
            skip: (0..n).map(|i| format!("no/such/entry{i}")).collect(),
            overwrite: vec![],
        };
        let move_req = |policy: Option<mtdss_proto::ctrl::TransferPolicy>| {
            mtdss_proto::ctrl::ctrl_request::Body::Move(mtdss_proto::ctrl::MoveRequest {
                pairs: vec![mtdss_proto::ctrl::MovePair {
                    from: src.join("a.txt").to_string_lossy().into_owned(),
                    to: dst.join("a.txt").to_string_lossy().into_owned(),
                }],
                policy,
            })
        };
        let assert_policy_limit = |resp: CtrlResponse| {
            assert!(!resp.ok, "超限策略必须 ok=false: {resp:?}");
            match resp.body {
                Some(mtdss_proto::ctrl::ctrl_response::Body::Error(e)) => {
                    assert_eq!(e.code, "POLICY_LIMIT", "code 必须 POLICY_LIMIT: {e:?}");
                }
                other => panic!("expected error body, got {other:?}"),
            }
        };

        // Move 臂:超限 2049 条 → POLICY_LIMIT,零 fs 变更。
        let resp = handle_ctrl_request(
            test_state(),
            CtrlRequest {
                version: mtdss_proto::CTRL_VERSION,
                body: Some(move_req(Some(oversized(
                    crate::jobs::POLICY_MAX_ENTRIES + 1,
                )))),
            },
        )
        .await;
        assert_policy_limit(resp);
        assert!(src.join("a.txt").exists(), "源件不得被移动");
        assert_eq!(
            std::fs::read_dir(&dst).unwrap().count(),
            0,
            "目标目录零改动"
        );

        // Copy 臂同规格:超限同样 POLICY_LIMIT,不落盘。
        let resp = handle_ctrl_request(
            test_state(),
            CtrlRequest {
                version: mtdss_proto::CTRL_VERSION,
                body: Some(mtdss_proto::ctrl::ctrl_request::Body::Copy(
                    mtdss_proto::ctrl::CopyRequest {
                        paths: vec![src.join("a.txt").to_string_lossy().into_owned()],
                        dir: dst.to_string_lossy().into_owned(),
                        policy: Some(oversized(crate::jobs::POLICY_MAX_ENTRIES + 1)),
                    },
                )),
            },
        )
        .await;
        assert_policy_limit(resp);
        assert_eq!(
            std::fs::read_dir(&dst).unwrap().count(),
            0,
            "目标目录零改动"
        );

        // 上限边界(恰 2048 条):放行,move 照常落盘。
        let resp = handle_ctrl_request(
            test_state(),
            CtrlRequest {
                version: mtdss_proto::CTRL_VERSION,
                body: Some(move_req(Some(oversized(
                    crate::jobs::POLICY_MAX_ENTRIES,
                )))),
            },
        )
        .await;
        assert!(resp.ok, "边界 {} 条必须放行: {resp:?}", crate::jobs::POLICY_MAX_ENTRIES);
        assert!(dst.join("a.txt").exists(), "边界放行后 move 应已落盘");
    }

    // ---------- 服务桥路由(V3-4 §4.1 安卓服务桥)----------
    //
    // 分发入口直测(同 roots_guard_* 先例):测试台 events 通道零订阅者 →
    // 桥快失败「应用未连接」(photo fast-fail 同款),不可达降级面因此即
    // 时可测,不等应答窗。有应答器的全链(浏览/下载/删除)在 serve_e2e
    // (unix,真实 IPC + fdpass)。

    /// 服务桥测试态:独立 home,roots.set 注册双桥根(可见 收件箱 + 隐藏
    /// 暂存区)+ 一个 fs 根 docs(Copy 落盘目录触桥用例的合法源对照)+
    /// 注入小超时窗(桥在 roots.set 构造期读定)。返回 (state, TempDir
    /// 保活)。
    fn test_state_with_serve_roots() -> (Arc<DaemonState>, tempfile::TempDir) {
        std::env::set_var("MTDSS_TEST_SERVE_TIMEOUT_MS", "300");
        let home = tempfile::tempdir().unwrap();
        std::fs::create_dir_all(home.path().join("docs")).unwrap();
        let (events, _) = tokio::sync::broadcast::channel(16);
        #[cfg(windows)]
        let update = std::sync::Arc::new(crate::update::UpdateService::new(
            crate::update::UpdateConfig::default(),
            events.clone(),
            tokio::sync::watch::channel(false).0,
        ));
        #[cfg(not(windows))]
        let update = ();
        let state = Arc::new(
            DaemonState::load(
                home.path(),
                home.path().join("inbox"),
                None,
                crate::inbox_dest::staging_dir(home.path()),
                false,
                // 测试台无照片桥(R7;photo 臂整体不进)
                false,
                Default::default(),
                Default::default(),
                ctrl_default_tuning(),
                events,
                update,
            )
            .unwrap(),
        );
        state.set_roots(vec![
            crate::roots::RootSpec {
                name: "收件箱".into(),
                path: "/收件箱".into(),
                source: crate::roots::RootSource::Bridge,
                visible: true,
            },
            crate::roots::RootSpec {
                name: "暂存".into(),
                path: "/暂存区".into(),
                source: crate::roots::RootSource::Bridge,
                visible: false,
            },
            // fs 根对照(Copy 第二用例的合法源;亦证 fs/bridge 混合集下
            // fs 面行为不变)
            crate::roots::RootSpec {
                name: String::new(),
                path: home.path().join("docs").to_string_lossy().into_owned(),
                source: crate::roots::RootSource::Fs,
                visible: true,
            },
        ]);
        (state, home)
    }

    /// 不可达降级(简报矩阵):无应答 → List 桥 vpath 报「不可用」(非
    /// INVALID_PATH、非裸 fs NOT_FOUND);根列表不含桥根(快失败 + 桥级
    /// 负缓存);隐藏桥根不出列表、List 其 vpath 照常路由(可授权而不可
    /// 见);stat 同口径。
    #[tokio::test]
    async fn serve_unreachable_degrades_honestly() {
        let (state, _home) = test_state_with_serve_roots();
        let list_req = |path: &str| {
            handle_ctrl_request(
                state.clone(),
                CtrlRequest {
                    version: mtdss_proto::CTRL_VERSION,
                    body: Some(mtdss_proto::ctrl::ctrl_request::Body::List(
                        mtdss_proto::ctrl::ListRequest {
                            path: path.to_string(),
                        },
                    )),
                },
            )
        };
        // List 桥 vpath:NOT_FOUND + 「服务目录不可用」(诚实报错,不挂死)
        let resp = list_req("/收件箱").await;
        assert!(!resp.ok, "桥不可达 List 应报错: {resp:?}");
        match resp.body {
            Some(mtdss_proto::ctrl::ctrl_response::Body::Error(e)) => {
                assert_eq!(e.code, "NOT_FOUND", "原码 NOT_FOUND: {e:?}");
                assert!(
                    e.message.contains("服务目录不可用"),
                    "文案点名桥不可用: {e:?}"
                );
                assert!(e.message.contains("应用未连接"), "快失败文案: {e:?}");
            }
            other => panic!("应得错误体, got {other:?}"),
        }
        // 深层 vpath / 隐藏根 vpath 同口径路由(隐藏根可授权而不可见)
        for path in ["/收件箱/子目录/x.pdf", "/暂存区/y.pdf"] {
            let resp = list_req(path).await;
            assert!(!resp.ok, "{path} 应路由服务桥并诚实报错: {resp:?}");
            match resp.body {
                Some(mtdss_proto::ctrl::ctrl_response::Body::Error(e)) => {
                    assert!(e.message.contains("服务目录不可用"), "{path}: {e:?}");
                }
                other => panic!("{path} 应得错误体, got {other:?}"),
            }
        }
        // stat:子路径与根自身同口径(不可用 → NOT_FOUND)
        for path in ["/收件箱", "/收件箱/子目录"] {
            let resp = handle_ctrl_request(
                state.clone(),
                CtrlRequest {
                    version: mtdss_proto::CTRL_VERSION,
                    body: Some(mtdss_proto::ctrl::ctrl_request::Body::Stat(
                        mtdss_proto::ctrl::StatRequest {
                            path: path.to_string(),
                        },
                    )),
                },
            )
            .await;
            assert!(!resp.ok, "{path} 桥不可达 stat 应报错");
        }
        // 根列表:桥根(含隐藏)不出列——可见根被桥探针隐藏,隐藏根本就
        // 不在 roots_listing
        let resp = list_req("").await;
        assert!(resp.ok, "列根应放行: {resp:?}");
        match resp.body {
            Some(mtdss_proto::ctrl::ctrl_response::Body::List(r)) => {
                let paths: Vec<&str> = r.entries.iter().map(|e| e.path.as_str()).collect();
                assert!(
                    !paths.iter().any(|p| p.contains("收件箱") || p.contains("暂存")),
                    "桥不可达 + 隐藏:列根不得出现桥根: {paths:?}"
                );
            }
            other => panic!("列根应得 List 体, got {other:?}"),
        }
    }

    /// 越权与只读矩阵(简报矩阵):`..` 逃逸/字节前缀/集外 vpath →
    /// INVALID_PATH(守卫原口径);Move/Copy/CleanupOutbox 触桥 vpath →
    /// 只读拒;Delete 桥 vpath 不被守卫拦(路由服务桥,不可达时逐条失败
    /// 表诚实报错——批不半途弃废)。
    #[tokio::test]
    async fn serve_escape_and_readonly_matrix() {
        let (state, _home) = test_state_with_serve_roots();
        let req = |body: mtdss_proto::ctrl::ctrl_request::Body| {
            handle_ctrl_request(
                state.clone(),
                CtrlRequest {
                    version: mtdss_proto::CTRL_VERSION,
                    body: Some(body),
                },
            )
        };
        let list = |path: &str| {
            req(mtdss_proto::ctrl::ctrl_request::Body::List(
                mtdss_proto::ctrl::ListRequest {
                    path: path.to_string(),
                },
            ))
        };
        // `..` 逃逸:规范化后出 vpath → 守卫拒(INVALID_PATH,非桥文案)
        for path in ["/收件箱/../evil", "/收件箱/../../etc"] {
            let resp = list(path).await;
            assert!(!resp.ok, "{path} 逃逸应拒: {resp:?}");
            match resp.body {
                Some(mtdss_proto::ctrl::ctrl_response::Body::Error(e)) => {
                    assert_eq!(e.code, "INVALID_PATH", "{path}: {e:?}");
                }
                other => panic!("{path} 应得错误体, got {other:?}"),
            }
        }
        // 字节前缀坑:整组件判定
        let resp = list("/收件箱-x").await;
        assert!(!resp.ok, "字节前缀应拒");
        match resp.body {
            Some(mtdss_proto::ctrl::ctrl_response::Body::Error(e)) => {
                assert_eq!(e.code, "INVALID_PATH", "{e:?}");
            }
            other => panic!("应得错误体, got {other:?}"),
        }
        // 集外 vpath:守卫拒
        let resp = list("/其他根").await;
        assert!(!resp.ok);

        // Move:from/to 任一触桥 → 只读拒
        let resp = req(mtdss_proto::ctrl::ctrl_request::Body::Move(
            mtdss_proto::ctrl::MoveRequest {
                pairs: vec![mtdss_proto::ctrl::MovePair {
                    from: "/收件箱/a.pdf".into(),
                    to: "/收件箱/b.pdf".into(),
                }],
                policy: None,
            },
        ))
        .await;
        assert!(!resp.ok, "Move 桥 vpath 应拒");
        match resp.body {
            Some(mtdss_proto::ctrl::ctrl_response::Body::Error(e)) => {
                assert_eq!(e.code, "INVALID_PATH");
                assert!(e.message.contains("只读"), "文案点名只读: {e:?}");
            }
            other => panic!("应得错误体, got {other:?}"),
        }
        // Copy:源与落盘目录触桥均拒(第二用例源取 fs 根内真实路径,钉
        // 「落盘目录触桥」独立拒因)
        let (state2, home2) = test_state_with_serve_roots();
        let docs_src = home2.path().join("docs").to_string_lossy().into_owned();
        for (paths, dir) in [
            (vec!["/收件箱/a.pdf".to_string()], "/tmp-dst".to_string()),
            (vec![docs_src], "/收件箱".to_string()),
        ] {
            let resp = handle_ctrl_request(
                state2.clone(),
                CtrlRequest {
                    version: mtdss_proto::CTRL_VERSION,
                    body: Some(mtdss_proto::ctrl::ctrl_request::Body::Copy(
                        mtdss_proto::ctrl::CopyRequest {
                            paths,
                            dir,
                            policy: None,
                        },
                    )),
                },
            )
            .await;
            assert!(!resp.ok, "Copy 触桥应拒");
            match resp.body {
                Some(mtdss_proto::ctrl::ctrl_response::Body::Error(e)) => {
                    assert!(e.message.contains("只读"), "{e:?}");
                }
                other => panic!("应得错误体, got {other:?}"),
            }
        }
        drop(state2);
        // CleanupOutbox:dir 触桥拒
        let resp = req(mtdss_proto::ctrl::ctrl_request::Body::CleanupOutbox(
            mtdss_proto::ctrl::CleanupOutboxRequest {
                dir: "/收件箱".into(),
                rels: vec![],
            },
        ))
        .await;
        assert!(!resp.ok, "CleanupOutbox 触桥应拒");

        // Delete:桥 vpath 不被守卫拦(唯一变更面例外);不可达 → ok=true
        // 批语义 + 逐条失败表(INTERNAL,文案携因),不挂死
        let resp = req(mtdss_proto::ctrl::ctrl_request::Body::Delete(
            mtdss_proto::ctrl::DeleteRequest {
                paths: vec!["/收件箱/a.pdf".into()],
                force_permanent: true,
            },
        ))
        .await;
        assert!(resp.ok, "Delete 批语义应 ok(逐条失败表承载): {resp:?}");
        match resp.body {
            Some(mtdss_proto::ctrl::ctrl_response::Body::Delete(r)) => {
                assert_eq!(r.deleted, 0);
                assert!(!r.recycled, "桥删无回收面");
                assert_eq!(r.failed.len(), 1, "不可达逐条诚实失败: {r:?}");
                assert_eq!(r.failed[0].code, "INTERNAL");
                assert!(
                    r.failed[0].message.contains("应用未连接"),
                    "失败文案携桥因: {r:?}"
                );
            }
            other => panic!("应得 Delete 体, got {other:?}"),
        }
    }

    /// roots.set 热换竞窗(评审 Important 修的回归钉):List/Stat/Delete 的
    /// 桥路由在臂内**单守卫快照**一次取定——请求间换根(App 撤销/重授 SAF
    /// 根)后:原 vpath 立即按新守卫口径收口(换 fs 集授权拒、换回 bridge
    /// 恢复桥路由),分发全程不 panic、不挂死。请求内双读竞态本身随单快照
    /// 结构消除(代码结构即钉:分发路径零 `.expect` 于 bridge_route)。
    #[tokio::test]
    async fn serve_routes_survive_roots_hot_swap() {
        let (state, home) = test_state_with_serve_roots();
        let req = |body: mtdss_proto::ctrl::ctrl_request::Body| {
            handle_ctrl_request(
                state.clone(),
                CtrlRequest {
                    version: mtdss_proto::CTRL_VERSION,
                    body: Some(body),
                },
            )
        };
        let list = |path: &str| {
            req(mtdss_proto::ctrl::ctrl_request::Body::List(
                mtdss_proto::ctrl::ListRequest {
                    path: path.to_string(),
                },
            ))
        };
        let stat = |path: &str| {
            req(mtdss_proto::ctrl::ctrl_request::Body::Stat(
                mtdss_proto::ctrl::StatRequest {
                    path: path.to_string(),
                },
            ))
        };
        // ① bridge 根在册:List/Stat 桥 vpath → 桥路由(不可达诚实报错)
        for f in [list("/收件箱"), stat("/收件箱")] {
            let resp = f.await;
            assert!(!resp.ok, "桥在册应路由服务桥(无应答器 → 报错): {resp:?}");
            match resp.body {
                Some(mtdss_proto::ctrl::ctrl_response::Body::Error(e)) => {
                    assert!(e.message.contains("服务目录不可用"), "{e:?}");
                }
                other => panic!("应得错误体, got {other:?}"),
            }
        }
        // ② 换 fs 集(桥根全撤):同一 vpath 立即回落守卫口径(热生效,
        // 分发不 panic;文案按平台管线:授权集外 / 绝对形态非法——共同点
        // 是**不再是桥路由**)。真 fs 根照旧放行。
        state.set_roots(vec![crate::roots::RootSpec {
            name: String::new(),
            path: home.path().join("docs").to_string_lossy().into_owned(),
            source: crate::roots::RootSource::Fs,
            visible: true,
        }]);
        for f in [list("/收件箱"), stat("/收件箱"), list("/收件箱/x.pdf")] {
            let resp = f.await;
            assert!(!resp.ok, "换 fs 集后桥 vpath 应拒: {resp:?}");
            match resp.body {
                Some(mtdss_proto::ctrl::ctrl_response::Body::Error(e)) => {
                    assert_eq!(e.code, "INVALID_PATH", "守卫原码: {e:?}");
                    assert!(
                        !e.message.contains("服务目录"),
                        "换集后不得再走桥路由: {e:?}"
                    );
                }
                other => panic!("应得错误体, got {other:?}"),
            }
        }
        let resp = list(&home.path().join("docs").to_string_lossy()).await;
        assert!(resp.ok, "fs 根内照旧放行: {resp:?}");
        // ③ 换回 bridge 集:桥路由恢复(同 vpath 又是桥文案)
        state.set_roots(vec![crate::roots::RootSpec {
            name: "收件箱".into(),
            path: "/收件箱".into(),
            source: crate::roots::RootSource::Bridge,
            visible: true,
        }]);
        let resp = list("/收件箱").await;
        assert!(!resp.ok);
        match resp.body {
            Some(mtdss_proto::ctrl::ctrl_response::Body::Error(e)) => {
                assert!(e.message.contains("服务目录不可用"), "桥路由恢复: {e:?}");
            }
            other => panic!("应得错误体, got {other:?}"),
        }
        // 换根后桥槽也随集重建(state.serve 语义):Delete 桥 vpath 照常
        // 分流(不可达逐条失败,非守卫拒)
        let resp = req(mtdss_proto::ctrl::ctrl_request::Body::Delete(
            mtdss_proto::ctrl::DeleteRequest {
                paths: vec!["/收件箱/x.pdf".into()],
                force_permanent: true,
            },
        ))
        .await;
        assert!(resp.ok, "Delete 分流应保持批语义: {resp:?}");
        match resp.body {
            Some(mtdss_proto::ctrl::ctrl_response::Body::Delete(r)) => {
                assert_eq!(r.failed.len(), 1);
                assert_eq!(r.failed[0].code, "INTERNAL", "桥臂(非守卫拒): {r:?}");
            }
            other => panic!("应得 Delete 体, got {other:?}"),
        }
    }
}
