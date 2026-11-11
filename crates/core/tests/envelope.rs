use ed25519_dalek::VerifyingKey;
use mtdss_core::identity::DeviceIdentity;
use mtdss_core::signaling::envelope::{open, seal, EnvelopeError, SignalBody, TransferPolicySig};
use mtdss_core::transport::intake::{reconcile_with_policy, FilePlan, FsDestIo};
use mtdss_proto::manifest::{ChunkDesc, FileEntry, Manifest};
use mtdss_proto::signal::{
    body_proto, BodyProto, ConnectAccept, ConnectRequest, CtrlAccept, CtrlConnect, SignalEnvelope,
    SignalPayload,
};
use prost::Message;

#[test]
fn roundtrip_all_bodies() {
    let a = DeviceIdentity::generate();
    let b = DeviceIdentity::generate();
    let bodies = vec![
        SignalBody::ConnectRequest {
            session_id: vec![1, 2, 3],
            target_dir: String::new(),
            job_id: String::new(),
            display_name: String::new(),
            policy: None,
            warm_reuse: false,
        },
        SignalBody::ConnectAccept { session_id: vec![1, 2, 3], warm_ok: false },
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
            display_name: String::new(),
            policy: Some(TransferPolicySig {
                skip: vec!["docs".into(), "a/b".into()],
                overwrite: vec!["c.bin".into()],
            }),
            warm_reuse: false,
        },
        // legacy 全空:旧发送端不带字段的线上形态(空串 + 无策略)
        SignalBody::ConnectRequest {
            session_id: vec![9],
            target_dir: String::new(),
            job_id: String::new(),
            display_name: String::new(),
            policy: None,
            warm_reuse: false,
        },
    ];
    for body in bodies {
        let sealed = seal(&a, &b.public_key(), &body);
        let opened = open(&sealed, &b, &a.public_key()).unwrap();
        assert_eq!(opened, body, "定向字段必须往返保真");
    }
}

/// V3-1 §6:ConnectRequest 的任务代表名(display_name)seal→open 往返保真
/// ——发起端算好的代表名原样到接收端;空名(旧端形态:proto3 缺省不编码
/// 该字段)开箱后仍是空串,接收端据此回退现状。
#[test]
fn connect_request_display_name_roundtrip_and_legacy_empty() {
    let a = DeviceIdentity::generate();
    let b = DeviceIdentity::generate();

    // 新端携名:非空代表名(含「等 N 项」形态)往返保真
    let sealed = seal(
        &a,
        &b.public_key(),
        &SignalBody::ConnectRequest {
            session_id: vec![1],
            target_dir: "/tmp/dst".into(),
            job_id: "job-1".into(),
            display_name: "报告.pdf 等 3 项".into(),
            policy: None,
            warm_reuse: false,
        },
    );
    let opened = open(&sealed, &b, &a.public_key()).unwrap();
    let SignalBody::ConnectRequest { display_name, .. } = opened else {
        panic!("expected ConnectRequest");
    };
    assert_eq!(display_name, "报告.pdf 等 3 项", "代表名必须往返保真");

    // 旧端不携名:空名封信(prost 对缺省空串不编码字段,字节形态与旧版
    // 发送端逐字节一致),开箱解出空串——回退现状的判定依据
    let sealed = seal(
        &a,
        &b.public_key(),
        &SignalBody::ConnectRequest {
            session_id: vec![2],
            target_dir: String::new(),
            job_id: String::new(),
            display_name: String::new(),
            policy: None,
            warm_reuse: false,
        },
    );
    let opened = open(&sealed, &b, &a.public_key()).unwrap();
    let SignalBody::ConnectRequest { display_name, .. } = opened else {
        panic!("expected ConnectRequest");
    };
    assert!(display_name.is_empty(), "旧端缺省必须解成空串: {display_name:?}");
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
            display_name: String::new(),
            policy: Some(TransferPolicySig {
                skip: vec!["ab/".into(), "/".into(), "x//".into(), String::new()],
                overwrite: vec![],
            }),
            warm_reuse: false,
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
        SignalBody::CtrlConnect { session_id: vec![9, 8, 7], mux: false },
        SignalBody::CtrlAccept { session_id: vec![5, 4, 3], mux: false },
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
            display_name: String::new(),
            policy: None,
            warm_reuse: false,
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
            display_name: String::new(),
            policy: None,
            warm_reuse: false,
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
            display_name: String::new(),
            policy: None,
            warm_reuse: false,
        },
    );
    assert!(matches!(open(&sealed, &c, &a.public_key()), Err(EnvelopeError::Open)));
}

/// 手工密封(envelope::seal 的步骤镜像,body 字节由测试手工 prost 编码):
/// x25519 派生与 envelope 同式(Montgomery u = (1+y)/(1-y)),sig 覆盖
/// 手编 body 字节,内层 SignalPayload 整体匿名密封。用于把「旧端线上
/// 字节」喂进真实 open() 路径。
fn seal_raw_body(
    sender: &DeviceIdentity,
    recipient: &VerifyingKey,
    body: &BodyProto,
) -> Vec<u8> {
    use crypto_box::rand_core::OsRng;
    use curve25519_dalek::edwards::CompressedEdwardsY;

    let point = CompressedEdwardsY(*recipient.as_bytes())
        .decompress()
        .expect("ed25519 verifying key is a valid curve point");
    let box_pk = crypto_box::PublicKey::from(point.to_montgomery().to_bytes());
    let inner = body.encode_to_vec();
    let payload = SignalPayload { sig: sender.sign(&inner).to_bytes().to_vec(), body: inner };
    let sealed = crypto_box::seal(&mut OsRng, &box_pk, &payload.encode_to_vec())
        .expect("crypto_box seal encryption is infallible");
    SignalEnvelope { sender_device_id: sender.device_id.clone(), sealed }.encode_to_vec()
}

/// V3-5 Stage B(传输复用):信令四体新增协商字段(ConnectRequest.warm_reuse
/// / ConnectAccept.warm_ok / CtrlConnect.mux / CtrlAccept.mux,proto3 只增
/// 不改号)的解码地基:
/// 1) 四体新字段置 true,seal→open(两把固定种子身份)逐字段断言往返保真;
/// 2) 旧形态:手工 prost 编码 BodyProto(消息不带新字段,即旧端线上字节)
///    → open 解出 warm_reuse/mux/warm_ok 恒 false——版本闸门据此判「对端
///    未协商复用」,旧端互通无感。
#[test]
fn new_signal_fields_roundtrip_and_legacy_default() {
    let a = DeviceIdentity::from_seed(&[
        0x11, 0x22, 0x33, 0x44, 0x55, 0x66, 0x77, 0x88, 0x99, 0xaa, 0xbb, 0xcc, 0xdd, 0xee, 0xff,
        0x01, 0x11, 0x22, 0x33, 0x44, 0x55, 0x66, 0x77, 0x88, 0x99, 0xaa, 0xbb, 0xcc, 0xdd, 0xee,
        0xff, 0x01,
    ]);
    let b = DeviceIdentity::from_seed(&[
        0xfe, 0xdc, 0xba, 0x98, 0x76, 0x54, 0x32, 0x10, 0xfe, 0xdc, 0xba, 0x98, 0x76, 0x54, 0x32,
        0x10, 0xfe, 0xdc, 0xba, 0x98, 0x76, 0x54, 0x32, 0x10, 0xfe, 0xdc, 0xba, 0x98, 0x76, 0x54,
        0x32, 0x10,
    ]);

    // 1) 新端协商位置 true:seal→open 逐字段往返保真
    let sealed = seal(
        &a,
        &b.public_key(),
        &SignalBody::ConnectRequest {
            session_id: vec![1],
            target_dir: "/tmp/dst".into(),
            job_id: "job-1".into(),
            display_name: "报告.pdf".into(),
            policy: None,
            warm_reuse: true,
        },
    );
    let SignalBody::ConnectRequest {
        session_id,
        target_dir,
        job_id,
        display_name,
        policy,
        warm_reuse,
    } = open(&sealed, &b, &a.public_key()).unwrap()
    else {
        panic!("expected ConnectRequest");
    };
    assert_eq!(session_id, vec![1]);
    assert_eq!(target_dir, "/tmp/dst");
    assert_eq!(job_id, "job-1");
    assert_eq!(display_name, "报告.pdf");
    assert_eq!(policy, None);
    assert!(warm_reuse, "warm_reuse=true 必须往返保真");

    let sealed = seal(
        &a,
        &b.public_key(),
        &SignalBody::ConnectAccept { session_id: vec![2], warm_ok: true },
    );
    let SignalBody::ConnectAccept { session_id, warm_ok } =
        open(&sealed, &b, &a.public_key()).unwrap()
    else {
        panic!("expected ConnectAccept");
    };
    assert_eq!(session_id, vec![2]);
    assert!(warm_ok, "warm_ok=true 必须往返保真");

    let sealed =
        seal(&a, &b.public_key(), &SignalBody::CtrlConnect { session_id: vec![3], mux: true });
    let SignalBody::CtrlConnect { session_id, mux } = open(&sealed, &b, &a.public_key()).unwrap()
    else {
        panic!("expected CtrlConnect");
    };
    assert_eq!(session_id, vec![3]);
    assert!(mux, "CtrlConnect.mux=true 必须往返保真");

    let sealed =
        seal(&a, &b.public_key(), &SignalBody::CtrlAccept { session_id: vec![4], mux: true });
    let SignalBody::CtrlAccept { session_id, mux } = open(&sealed, &b, &a.public_key()).unwrap()
    else {
        panic!("expected CtrlAccept");
    };
    assert_eq!(session_id, vec![4]);
    assert!(mux, "CtrlAccept.mux=true 必须往返保真");

    // 2) 旧形态:手工 prost 编码,消息不带新字段(prost 对 proto3 缺省
    //    bool 不编码,编码字节与旧端线上形态逐字节一致)→ 解出恒 false
    let sealed = seal_raw_body(
        &a,
        &b.public_key(),
        &BodyProto {
            body: Some(body_proto::Body::ConnectRequest(ConnectRequest {
                session_id: vec![1],
                target_dir: "/tmp/dst".into(),
                job_id: "job-1".into(),
                display_name: String::new(),
                policy: None,
                warm_reuse: false,
            })),
        },
    );
    let SignalBody::ConnectRequest { warm_reuse, .. } = open(&sealed, &b, &a.public_key()).unwrap()
    else {
        panic!("expected ConnectRequest");
    };
    assert!(!warm_reuse, "旧端字节必须解出 warm_reuse=false: {warm_reuse}");

    let sealed = seal_raw_body(
        &a,
        &b.public_key(),
        &BodyProto {
            body: Some(body_proto::Body::ConnectAccept(ConnectAccept {
                session_id: vec![2],
                warm_ok: false,
            })),
        },
    );
    let SignalBody::ConnectAccept { warm_ok, .. } = open(&sealed, &b, &a.public_key()).unwrap()
    else {
        panic!("expected ConnectAccept");
    };
    assert!(!warm_ok, "旧端字节必须解出 warm_ok=false: {warm_ok}");

    let sealed = seal_raw_body(
        &a,
        &b.public_key(),
        &BodyProto {
            body: Some(body_proto::Body::CtrlConnect(CtrlConnect {
                session_id: vec![3],
                mux: false,
            })),
        },
    );
    let SignalBody::CtrlConnect { mux, .. } = open(&sealed, &b, &a.public_key()).unwrap() else {
        panic!("expected CtrlConnect");
    };
    assert!(!mux, "旧端字节必须解出 CtrlConnect.mux=false: {mux}");

    let sealed = seal_raw_body(
        &a,
        &b.public_key(),
        &BodyProto {
            body: Some(body_proto::Body::CtrlAccept(CtrlAccept {
                session_id: vec![4],
                mux: false,
            })),
        },
    );
    let SignalBody::CtrlAccept { mux, .. } = open(&sealed, &b, &a.public_key()).unwrap() else {
        panic!("expected CtrlAccept");
    };
    assert!(!mux, "旧端字节必须解出 CtrlAccept.mux=false: {mux}");
}
