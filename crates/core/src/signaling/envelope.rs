//! 信令端到端加密信封。
//!
//! 密封 = crypto_box 匿名密封盒（临时密钥，仅接收方能解）；内层载荷带发送方
//! Ed25519 签名（覆盖 body 字节）。接收方解封后用已知发送方公钥验签。
//! 信令服务器只见不透明字节（SignalEnvelope{sender_device_id, sealed}）。

use crate::identity::DeviceIdentity;
use crypto_box::rand_core::OsRng;
use ed25519_dalek::Verifier;
use mtdss_proto::signal::*;
use prost::Message;
use thiserror::Error;

/// 五种信令体（对应 proto 的 BodyProto oneof）。
#[derive(Debug, PartialEq, Clone)]
pub enum SignalBody {
    ConnectRequest {
        session_id: Vec<u8>,
        /// 定向落盘目录（绝对形态；M3c Task 5）。空 = 接收方默认收件目录
        /// （legacy 互通）；非空但非法由接收方 warn 回退默认目录——坏 dir
        /// 不拒收来件，互通语义收口在接收侧。
        target_dir: String,
        /// 任务绑定 id（Task 6/7 的 jobs 注册表接线；空 = legacy）。
        job_id: String,
        /// 冲突策略（skip/overwrite）。刻意不进清单/线协议：清单与其签名
        /// 域保持字节不变，旧端互通。None = 空策略（legacy 等价，线上不
        /// 携带空消息）；接收转换时归一（尾斜杠清理，见
        /// [`TransferPolicySig::from_proto`]）。
        policy: Option<TransferPolicySig>,
    },
    ConnectAccept { session_id: Vec<u8> },
    Candidates { session_id: Vec<u8>, addrs: Vec<String> },
    CtrlConnect { session_id: Vec<u8> },
    CtrlAccept { session_id: Vec<u8> },
}

/// 信令面冲突策略（ConnectRequest.policy 的 Rust 形态，与 proto
/// TransferPolicy 一一对应）。不复用 `transport::intake::TransferPolicy`
/// 直接入枚举：信令层与落盘引擎是两个边界，策略的归一（Task 3 评审约束）
/// 必须收口在信令接收转换这一处，引擎只认归一后的条目。
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct TransferPolicySig {
    /// 命中即整条跳过（rel 或目录前缀）。
    pub skip: Vec<String>,
    /// 命中即覆盖（rel 或目录前缀）。
    pub overwrite: Vec<String>,
}

impl TransferPolicySig {
    /// proto → 信令策略（接收转换边界）。Task 3 评审约束在此闭合：引擎的
    /// 前缀匹配以「条目 == p 或 rel 以 `p + "/"` 起头」判段边界，条目带
    /// 尾斜杠（`"ab/"`）会让匹配静默落空；策略入系统处统一剥尾斜杠，
    /// 清空后的条目（如 `"/"`、空串）整体丢弃。只清尾不动内部与大小写
    /// （引擎匹配是字节精确比较）。
    fn from_proto(p: &TransferPolicy) -> Self {
        Self {
            skip: normalize_entries(&p.skip),
            overwrite: normalize_entries(&p.overwrite),
        }
    }

    /// 信令策略 → 引擎策略（接收方 RecvCtl 装配）。条目此时已归一。
    pub fn to_engine(&self) -> crate::transport::intake::TransferPolicy {
        crate::transport::intake::TransferPolicy {
            skip: self.skip.clone(),
            overwrite: self.overwrite.clone(),
        }
    }

    /// 引擎策略 → 信令策略（发送方装配 ConnectRequest）。两表皆空 →
    /// None（legacy 等价：空策略与不携带在线上语义一致，不为空表发消息）。
    pub fn from_engine(p: &crate::transport::intake::TransferPolicy) -> Option<Self> {
        if p.skip.is_empty() && p.overwrite.is_empty() {
            return None;
        }
        Some(Self {
            skip: p.skip.clone(),
            overwrite: p.overwrite.clone(),
        })
    }
}

/// 策略条目归一：剥尾部 `/`（可叠多个）后丢弃清空的条目。
fn normalize_entries(list: &[String]) -> Vec<String> {
    list.iter()
        .map(|s| s.trim_end_matches('/'))
        .filter(|s| !s.is_empty())
        .map(str::to_owned)
        .collect()
}

#[derive(Debug, Error)]
pub enum EnvelopeError {
    #[error("decode failed")]
    Decode,
    #[error("cannot open sealed box")]
    Open,
    #[error("sender signature invalid")]
    Signature,
    #[error("payload malformed")]
    Malformed,
}

fn body_to_proto(b: &SignalBody) -> body_proto::Body {
    match b {
        SignalBody::ConnectRequest { session_id, target_dir, job_id, policy } => {
            body_proto::Body::ConnectRequest(ConnectRequest {
                session_id: session_id.clone(),
                target_dir: target_dir.clone(),
                job_id: job_id.clone(),
                policy: policy.as_ref().map(|p| TransferPolicy {
                    skip: p.skip.clone(),
                    overwrite: p.overwrite.clone(),
                }),
            })
        }
        SignalBody::ConnectAccept { session_id } => {
            body_proto::Body::ConnectAccept(ConnectAccept { session_id: session_id.clone() })
        }
        SignalBody::Candidates { session_id, addrs } => body_proto::Body::Candidates(Candidates {
            session_id: session_id.clone(),
            addrs: addrs.clone(),
        }),
        SignalBody::CtrlConnect { session_id } => {
            body_proto::Body::CtrlConnect(CtrlConnect { session_id: session_id.clone() })
        }
        SignalBody::CtrlAccept { session_id } => {
            body_proto::Body::CtrlAccept(CtrlAccept { session_id: session_id.clone() })
        }
    }
}

fn body_from_proto(b: &body_proto::Body) -> Result<SignalBody, EnvelopeError> {
    Ok(match b {
        body_proto::Body::ConnectRequest(m) => SignalBody::ConnectRequest {
            session_id: m.session_id.clone(),
            // 策略归一收口在接收转换（Task 3 评审约束）：经 open() 进入
            // 系统的策略条目一律无尾斜杠、无空条目
            policy: m.policy.as_ref().map(TransferPolicySig::from_proto),
            target_dir: m.target_dir.clone(),
            job_id: m.job_id.clone(),
        },
        body_proto::Body::ConnectAccept(m) => {
            SignalBody::ConnectAccept { session_id: m.session_id.clone() }
        }
        body_proto::Body::Candidates(m) => SignalBody::Candidates {
            session_id: m.session_id.clone(),
            addrs: m.addrs.clone(),
        },
        body_proto::Body::CtrlConnect(m) => {
            SignalBody::CtrlConnect { session_id: m.session_id.clone() }
        }
        body_proto::Body::CtrlAccept(m) => {
            SignalBody::CtrlAccept { session_id: m.session_id.clone() }
        }
    })
}

/// Ed25519 公钥 -> X25519 公钥（Montgomery u = (1+y)/(1-y)），
/// 与 x25519_sk 派生的标量在 DH 上自洽：u(A) = x25519(clamp(sk), base)。
fn x25519_pk(pk: &ed25519_dalek::VerifyingKey) -> crypto_box::PublicKey {
    use curve25519_dalek::edwards::CompressedEdwardsY;
    let point = CompressedEdwardsY(*pk.as_bytes())
        .decompress()
        .expect("ed25519 verifying key is a valid curve point");
    crypto_box::PublicKey::from(point.to_montgomery().to_bytes())
}

/// 同一 Ed25519 身份的 X25519 私钥：SHA-512(seed) 前 32 字节，
/// x25519 使用时按标准 clamp（与 Ed25519 签名标量一致，可重复派生）。
fn x25519_sk(me: &DeviceIdentity) -> crypto_box::SecretKey {
    crypto_box::SecretKey::from(me.signing.to_scalar_bytes())
}

/// 密封信令体给接收方：内层 BodyProto 编码字节由发送方 Ed25519 签名，
/// SignalPayload{sig, body} 整体匿名密封，外层 SignalEnvelope 编码返回。
pub fn seal(
    sender: &DeviceIdentity,
    recipient: &ed25519_dalek::VerifyingKey,
    body: &SignalBody,
) -> Vec<u8> {
    let inner = BodyProto { body: Some(body_to_proto(body)) }.encode_to_vec();
    let sig = sender.sign(&inner);
    let payload = SignalPayload { sig: sig.to_bytes().to_vec(), body: inner };
    let sealed = crypto_box::seal(&mut OsRng, &x25519_pk(recipient), &payload.encode_to_vec())
        .expect("crypto_box seal encryption is infallible");
    SignalEnvelope { sender_device_id: sender.device_id.clone(), sealed }.encode_to_vec()
}

/// 解封并验签：以自身身份解密封盒，用发送方公钥验证 sig 覆盖的 body 字节，
/// 再解码出信令体。
pub fn open(
    data: &[u8],
    me: &DeviceIdentity,
    sender_pk: &ed25519_dalek::VerifyingKey,
) -> Result<SignalBody, EnvelopeError> {
    let env = SignalEnvelope::decode(data).map_err(|_| EnvelopeError::Decode)?;
    let plain = crypto_box::seal_open(&x25519_sk(me), &env.sealed).map_err(|_| EnvelopeError::Open)?;
    let payload = SignalPayload::decode(plain.as_slice()).map_err(|_| EnvelopeError::Malformed)?;
    let sig = ed25519_dalek::Signature::from_slice(&payload.sig)
        .map_err(|_| EnvelopeError::Signature)?;
    sender_pk.verify(&payload.body, &sig).map_err(|_| EnvelopeError::Signature)?;
    let inner =
        BodyProto::decode(payload.body.as_slice()).map_err(|_| EnvelopeError::Malformed)?;
    body_from_proto(&inner.body.ok_or(EnvelopeError::Malformed)?)
}
