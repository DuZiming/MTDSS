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

/// 三种信令体（对应 proto 的 BodyProto oneof）。
#[derive(Debug, PartialEq, Clone)]
pub enum SignalBody {
    ConnectRequest { session_id: Vec<u8> },
    ConnectAccept { session_id: Vec<u8> },
    Candidates { session_id: Vec<u8>, addrs: Vec<String> },
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
        SignalBody::ConnectRequest { session_id } => {
            body_proto::Body::ConnectRequest(ConnectRequest { session_id: session_id.clone() })
        }
        SignalBody::ConnectAccept { session_id } => {
            body_proto::Body::ConnectAccept(ConnectAccept { session_id: session_id.clone() })
        }
        SignalBody::Candidates { session_id, addrs } => body_proto::Body::Candidates(Candidates {
            session_id: session_id.clone(),
            addrs: addrs.clone(),
        }),
    }
}

fn body_from_proto(b: &body_proto::Body) -> Result<SignalBody, EnvelopeError> {
    Ok(match b {
        body_proto::Body::ConnectRequest(m) => {
            SignalBody::ConnectRequest { session_id: m.session_id.clone() }
        }
        body_proto::Body::ConnectAccept(m) => {
            SignalBody::ConnectAccept { session_id: m.session_id.clone() }
        }
        body_proto::Body::Candidates(m) => SignalBody::Candidates {
            session_id: m.session_id.clone(),
            addrs: m.addrs.clone(),
        },
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
