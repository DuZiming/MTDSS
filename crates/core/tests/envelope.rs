use mtdss_core::identity::DeviceIdentity;
use mtdss_core::signaling::envelope::{open, seal, EnvelopeError, SignalBody, TransferPolicySig};
use mtdss_core::transport::intake::{reconcile_with_policy, FilePlan, FsDestIo};
use mtdss_proto::manifest::{ChunkDesc, FileEntry, Manifest};

#[test]
fn roundtrip_all_bodies() {
    let a = DeviceIdentity::generate();
    let b = DeviceIdentity::generate();
    let bodies = vec![
        SignalBody::ConnectRequest {
            session_id: vec![1, 2, 3],
            target_dir: String::new(),
            job_id: String::new(),
            policy: None,
        },
        SignalBody::ConnectAccept { session_id: vec![1, 2, 3] },
        SignalBody::Candidates { session_id: vec![1, 2, 3], addrs: vec!["127.0.0.1:9000".into()] },
    ];
    for body in bodies {
        let sealed = seal(&a, &b.public_key(), &body);
        let opened = open(&sealed, &b, &a.public_key()).unwrap();
        assert_eq!(format!("{opened:?}"), format!("{body:?}"));
    }
}

/// M3c Task 5:ConnectRequest 定向三字段(target_dir/job_id/policy)的
/// seal→open 往返——满配(policy 嵌套 skip/overwrite 两表)与 legacy 全空
/// (空目录、空任务、无策略)两个形态都必须保真。
#[test]
fn connect_request_target_fields_roundtrip() {
    let a = DeviceIdentity::generate();
    let b = DeviceIdentity::generate();
    let bodies = vec![
        SignalBody::ConnectRequest {
            session_id: vec![1, 2, 3],
            target_dir: "/tmp/定向 目录".into(),
            job_id: "job-42".into(),
            policy: Some(TransferPolicySig {
                skip: vec!["docs".into(), "a/b".into()],
                overwrite: vec!["c.bin".into()],
            }),
        },
        // legacy 全空:旧发送端不带字段的线上形态(空串 + 无策略)
        SignalBody::ConnectRequest {
            session_id: vec![9],
            target_dir: String::new(),
            job_id: String::new(),
            policy: None,
        },
    ];
    for body in bodies {
        let sealed = seal(&a, &b.public_key(), &body);
        let opened = open(&sealed, &b, &a.public_key()).unwrap();
        assert_eq!(opened, body, "定向字段必须往返保真");
    }
}

/// Task 3 评审约束在传输边缘闭合:带尾斜杠的策略条目(`"ab/"`)在引擎的
/// 段边界前缀匹配里静默不命中,信令层(策略入系统处)必须在**接收转换**
/// 时归一——剥尾斜杠、丢弃清空后的条目。经真实 seal→open 路径钉死:
/// `["ab/"]` 开箱后归一为 `["ab"]`,且对 rel `ab/x` 精确命中(判 Skip)。
#[test]
fn policy_entries_normalized_on_receive_and_hit_engine() {
    let a = DeviceIdentity::generate();
    let b = DeviceIdentity::generate();
    let sealed = seal(
        &a,
        &b.public_key(),
        &SignalBody::ConnectRequest {
            session_id: vec![7],
            target_dir: String::new(),
            job_id: String::new(),
            policy: Some(TransferPolicySig {
                skip: vec!["ab/".into(), "/".into(), "x//".into(), String::new()],
                overwrite: vec![],
            }),
        },
    );
    let opened = open(&sealed, &b, &a.public_key()).unwrap();
    let SignalBody::ConnectRequest { policy: Some(sig), .. } = opened else {
        panic!("expected ConnectRequest, got {opened:?}");
    };
    // 尾斜杠剥净("ab/"→"ab"、"x//"→"x"),清空后的条目("/" 与 "")整体丢弃
    assert_eq!(
        sig.skip,
        vec!["ab".to_string(), "x".to_string()],
        "接收转换必须归一策略条目: {sig:?}"
    );

    // 归一后的条目必须被引擎命中:"ab" 以段边界前缀命中 rel "ab/x" → Skip
    let out = tempfile::tempdir().unwrap();
    let man = Manifest {
        version: mtdss_proto::MANIFEST_VERSION,
        transfer_id: "t".into(),
        files: vec![FileEntry {
            rel_path: "ab/x".into(),
            size: 1,
            sha256: vec![0u8; 32],
            chunks: vec![ChunkDesc { offset: 0, length: 1, sha256: vec![0u8; 32] }],
        }],
        sender_signature: vec![],
    };
    let plans = reconcile_with_policy(&FsDestIo::new(out.path()), &man, &sig.to_engine());
    assert_eq!(plans, vec![FilePlan::Skip], "归一后的条目必须命中引擎前缀匹配");
}

#[test]
fn ctrl_bodies_roundtrip() {
    let a = DeviceIdentity::generate();
    let b = DeviceIdentity::generate();
    let bodies = vec![
        SignalBody::CtrlConnect { session_id: vec![9, 8, 7] },
        SignalBody::CtrlAccept { session_id: vec![5, 4, 3] },
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
    let mut sealed = seal(
        &a,
        &b.public_key(),
        &SignalBody::ConnectRequest {
            session_id: vec![7],
            target_dir: String::new(),
            job_id: String::new(),
            policy: None,
        },
    );
    let n = sealed.len();
    sealed[n - 1] ^= 0xFF;
    assert!(matches!(open(&sealed, &b, &a.public_key()), Err(EnvelopeError::Open)));
}

#[test]
fn wrong_sender_key_rejected() {
    let a = DeviceIdentity::generate();
    let b = DeviceIdentity::generate();
    let mallory = DeviceIdentity::generate();
    let sealed = seal(
        &a,
        &b.public_key(),
        &SignalBody::ConnectRequest {
            session_id: vec![7],
            target_dir: String::new(),
            job_id: String::new(),
            policy: None,
        },
    );
    // 接收方以为发送者是 mallory（公钥不对）→ 验签失败
    assert!(matches!(open(&sealed, &b, &mallory.public_key()), Err(EnvelopeError::Signature)));
}

#[test]
fn wrong_recipient_cannot_open() {
    let a = DeviceIdentity::generate();
    let b = DeviceIdentity::generate();
    let c = DeviceIdentity::generate();
    let sealed = seal(
        &a,
        &b.public_key(),
        &SignalBody::ConnectRequest {
            session_id: vec![7],
            target_dir: String::new(),
            job_id: String::new(),
            policy: None,
        },
    );
    assert!(matches!(open(&sealed, &c, &a.public_key()), Err(EnvelopeError::Open)));
}
