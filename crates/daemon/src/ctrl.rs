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
//! 驱逐(无 keep-alive,[`CTRL_TUNING`] 60s 空闲上界治理静默死链),请求
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
//! 标志原样携带,批量逐条失败表随响应携带)。

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
use std::path::PathBuf;
use std::sync::{Arc, Mutex};
use std::time::Duration;

/// 控制通道传输调优:无 keep-alive + 60s 空闲上界——池内空闲连接懒驱逐
/// (下次取用时按 `closed()` 惰性判死),静默死链最迟 60s 显形关闭。
/// MTU 上界取默认 1452(控制通道不走中继,无需给 16B 标签头留余量)。
pub(crate) const CTRL_TUNING: mtdss_core::transport::endpoint::TransportTuning =
    mtdss_core::transport::endpoint::TransportTuning {
        keep_alive_interval: None,
        max_idle_timeout: Duration::from_secs(60),
        max_mtu: 1452,
    };

/// 发起端单次 ctrl 请求-响应往返上界(打洞已另算;覆盖慢盘大目录枚举的
/// 合理余量,对端死链靠 QUIC 连接关闭/超时显形)。
const CTRL_SEND_TIMEOUT: Duration = Duration::from_secs(15);

/// CtrlAccept 单次等待窗口:CtrlConnect 本身已由枢纽定向通道保帧(随代际
/// 安装即存在,不存在「订阅前丢弃」窗),但响应端会话任务仍可能有启动
/// 开销(身份加载/目录缓存未命中),Accept 晚到时发起端短窗重发同一
/// CtrlConnect 兜底——任一次等到即过,多余会话随 CTRL_TUNING 空闲上界
/// 自灭(见 [`respond_session`] 注)。
const CTRL_ACCEPT_WINDOW: Duration = Duration::from_millis(2500);
/// CtrlAccept 重发上限(同一会话 id;总预算 3×2.5s ≈ 8s,与请求级 15s
/// 上界相加仍满足被杀对端场景 ≤30s 的有界失败约束)。
const CTRL_ACCEPT_ATTEMPTS: u32 = 3;
/// CtrlAccept 重发间隔(窗口之间的小歇,给响应端留建端点的时间)。
const CTRL_ACCEPT_RETRY_PACE: Duration = Duration::from_millis(400);

/// ctrl 打洞参数(与传输面同规格:budget 10s / 握手 2s,调优换 CTRL_TUNING)。
fn ctrl_punch() -> punch::PunchConfig {
    punch::PunchConfig {
        budget: Duration::from_secs(10),
        handshake_timeout: Duration::from_secs(2),
        tuning: CTRL_TUNING,
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

/// 池条目:连接 + 保活锚。quinn 端点 drop 会终止端点驱动并连带关闭其上
/// 连接,端点必须随连接一并存活(只随条目入池/出池,永不单独读取)。
#[derive(Clone)]
struct PooledConn {
    _keep: quinn::Endpoint,
    conn: quinn::Connection,
}

/// 发起端连接池:per-device 单飞锁 + 连接缓存。
pub(crate) struct CtrlPool {
    /// per-device 单飞锁(打洞全程串行:并发首查不双打洞;不同设备互不
    /// 阻塞——锁 Arc 在 std 锁内取出,await 期间绝不持 std 锁)。
    locks: Mutex<HashMap<String, Arc<tokio::sync::Mutex<()>>>>,
    /// per-device 连接缓存(端点随连接一并保活,见 [`PooledConn`])。
    conns: Mutex<HashMap<String, PooledConn>>,
}

impl CtrlPool {
    pub(crate) fn new() -> Self {
        Self {
            locks: Mutex::new(HashMap::new()),
            conns: Mutex::new(HashMap::new()),
        }
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
        // device == 本机 device_id → core fs 直调(短路,不建连接)
        if state.self_device_id().as_deref() == Some(device) {
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
        if state.self_device_id().as_deref() == Some(device) {
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
        if state.self_device_id().as_deref() == Some(device) {
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
    /// detail)供调用方落作业 failed。
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
            return Ok(job_query_response(snap.iter().find(|j| j.id == job_id)));
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
        let (ep, conn) = self.punch_via_hub(state, device).await?;
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

    /// 经枢纽打洞:先订阅(对端信令不会早于订阅窗口)→ CtrlConnect →
    /// 等 CtrlAccept → 建双角色端点 → 互发 Candidates → punch_connect。
    /// 返回 (端点, 连接),端点随连接保活(调用方入池)。
    async fn punch_via_hub(
        &self,
        state: &Arc<DaemonState>,
        device: &str,
    ) -> Result<(quinn::Endpoint, quinn::Connection), CtrlErr> {
        let hub = state.hub();
        let local = |e: anyhow::Error| CtrlErr::Internal(format!("{e:#}"));

        // 先订阅再发信令(Accept/Candidates 不会早于订阅窗口,漏帧即失败)
        let mut rx = hub
            .subscribe()
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
            flow::setup_punch_endpoint(&me, device, &peer_pk, &hub.stun(), CTRL_TUNING)
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
        let conn = punch::punch_connect(&ep, &me, &allowed, &peer_pk, &theirs, &ctrl_punch())
            .await
            .map_err(|e| {
                CtrlErr::Punch(flow::punch_err(e, nat_symmetric, mine.len()).to_string())
            })?;
        Ok((ep, conn))
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

/// proto DirEntry → 数据侧 DirEntryData(逐字段拷贝)。
fn proto_dir_entry(e: &mtdss_proto::ctrl::DirEntry) -> mtdss_core::fs::DirEntryData {
    mtdss_core::fs::DirEntryData {
        name: e.name.clone(),
        path: e.path.clone(),
        size: e.size,
        mtime_ms: e.mtime_ms,
        is_dir: e.is_dir,
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
fn job_query_response(
    snap: Option<&crate::jobs::JobSnap>,
) -> mtdss_proto::ctrl::JobQueryResponse {
    match snap {
        Some(j) => mtdss_proto::ctrl::JobQueryResponse {
            state: j.state.clone(),
            files_done: j.files_done,
            files_total: j.files_total,
            bytes_done: j.bytes_done,
            bytes_total: j.bytes_total,
            error: j.error.clone().unwrap_or_default(),
        },
        None => mtdss_proto::ctrl::JobQueryResponse {
            state: "missing".into(),
            files_done: 0,
            files_total: 0,
            bytes_done: 0,
            bytes_total: 0,
            error: "任务不存在".into(),
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
/// punch_connect → serve_conn(连接存活期服务请求)。任一环节失败只告警
/// 弃会话,不影响其余会话;会话任务无返回值,失败即静默收尾。
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
    let (ep, mine, _nat_symmetric, _stun_ok) = match flow::setup_punch_endpoint(
        &me,
        &from,
        &peer_pk,
        &hub.stun(),
        CTRL_TUNING,
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
    match punch::punch_connect(&ep, &me, &allowed, &peer_pk, &theirs, &ctrl_punch()).await {
        Ok(conn) => {
            tracing::info!("ctrl 会话已打通(from={from})");
            // 连接存活期服务请求;对端退出/网络断即返回(任务收尾)。
            // handler 携 state(Task 7:orchestrate 响应分支要起本地 upload
            // 作业)——Arc 按流克隆,handler 闭包只要求 Send + Sync。
            let st = state.clone();
            serve_conn(conn, move |req: CtrlRequest| {
                let st = st.clone();
                async move { handle_ctrl_request(st, req).await }
            })
            .await;
        }
        Err(e) => {
            tracing::warn!("ctrl 会话:打洞失败,弃会话: {e:#}");
        }
    }
}

/// ctrl 请求分发(serve_conn 的 async handler):version 不符 →
/// UNSUPPORTED_VERSION(ok=false);List/Stat/Delete/Move/Copy → core fs,
/// fs 调用一律经 [`blocking_fs`] 落阻塞线程池(见其注);Orchestrate/
/// CancelJob/JobQuery(Task 7)→ 作业注册表(orchestrate 在**本机**起
/// 回显 job_id 的 upload 作业,拒绝不按错误上抛、原样带回 accepted=false
/// 与 detail);Move/Copy 臂在 policy 转换后即校验策略上限(与发起端
/// [`check_policy_limits`] 对称,超限 POLICY_LIMIT)。FsError 原码透传,
/// 截断标志原样携带,批量逐条失败表随响应携带。请求读不出/响应写不出由
/// serve_conn 逐流兜底。
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
    let result: Result<Resp, mtdss_core::fs::FsError> = match req.body {
        Some(Req::List(r)) => blocking_fs(move || mtdss_core::fs::list_dir(&r.path))
            .await
            .map(|out| {
                Resp::List(mtdss_proto::ctrl::ListResponse {
                    entries: out.entries.iter().map(entry_proto).collect(),
                    truncated: out.truncated,
                })
            }),
        Some(Req::Stat(r)) => blocking_fs(move || mtdss_core::fs::stat_path(&r.path))
            .await
            .map(|d| {
                Resp::Stat(mtdss_proto::ctrl::StatResponse {
                    entry: Some(entry_proto(&d)),
                })
            }),
        Some(Req::Delete(r)) => {
            blocking_fs(move || mtdss_core::fs::delete_paths(&r.paths, r.force_permanent))
                .await
                .map(|(deleted, recycled, failed)| {
                    Resp::Delete(proto_delete_response(deleted, recycled, &failed))
                })
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
        Some(Req::Orchestrate(r)) => {
            let policy = r.policy.as_ref().map(policy_engine).unwrap_or_default();
            match state
                .jobs()
                .accept_orchestration(&state, &r.job_id, &r.to_device, &r.paths, &r.target_dir, policy)
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
            Ok(Resp::JobQuery(job_query_response(
                snap.iter().find(|j| j.id == r.job_id),
            )))
        }
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

/// 数据侧 DirEntryData → proto DirEntry(逐字段拷贝)。
fn entry_proto(d: &mtdss_core::fs::DirEntryData) -> mtdss_proto::ctrl::DirEntry {
    mtdss_proto::ctrl::DirEntry {
        name: d.name.clone(),
        path: d.path.clone(),
        size: d.size,
        mtime_ms: d.mtime_ms,
        is_dir: d.is_dir,
    }
}

#[cfg(test)]
mod tests {
    use super::*;

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
                Default::default(),
                Default::default(),
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
}
