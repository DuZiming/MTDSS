use mtdss_core::identity::DeviceIdentity;
use mtdss_core::signaling::envelope::{open, seal, EnvelopeError, SignalBody};

#[test]
fn roundtrip_all_bodies() {
    let a = DeviceIdentity::generate();
    let b = DeviceIdentity::generate();
    let bodies = vec![
        SignalBody::ConnectRequest { session_id: vec![1, 2, 3] },
        SignalBody::ConnectAccept { session_id: vec![1, 2, 3] },
        SignalBody::Candidates { session_id: vec![1, 2, 3], addrs: vec!["127.0.0.1:9000".into()] },
    ];
    for body in bodies {
        let sealed = seal(&a, &b.public_key(), &body);
        let opened = open(&sealed, &b, &a.public_key()).unwrap();
        assert_eq!(format!("{opened:?}"), format!("{body:?}"));
    }
}

#[test]
fn tampered_ciphertext_rejected() {
    let a = DeviceIdentity::generate();
    let b = DeviceIdentity::generate();
    let mut sealed = seal(&a, &b.public_key(), &SignalBody::ConnectRequest { session_id: vec![7] });
    let n = sealed.len();
    sealed[n - 1] ^= 0xFF;
    assert!(matches!(open(&sealed, &b, &a.public_key()), Err(EnvelopeError::Open)));
}

#[test]
fn wrong_sender_key_rejected() {
    let a = DeviceIdentity::generate();
    let b = DeviceIdentity::generate();
    let mallory = DeviceIdentity::generate();
    let sealed = seal(&a, &b.public_key(), &SignalBody::ConnectRequest { session_id: vec![7] });
    // 接收方以为发送者是 mallory（公钥不对）→ 验签失败
    assert!(matches!(open(&sealed, &b, &mallory.public_key()), Err(EnvelopeError::Signature)));
}

#[test]
fn wrong_recipient_cannot_open() {
    let a = DeviceIdentity::generate();
    let b = DeviceIdentity::generate();
    let c = DeviceIdentity::generate();
    let sealed = seal(&a, &b.public_key(), &SignalBody::ConnectRequest { session_id: vec![7] });
    assert!(matches!(open(&sealed, &c, &a.public_key()), Err(EnvelopeError::Open)));
}
