use mtdss_proto::manifest::*;
use mtdss_proto::signal::*;
use mtdss_proto::ws::*;
use prost::Message;

#[test]
fn ws_frame_roundtrip() {
    let f = WsClientFrame {
        frame: Some(ws_client_frame::Frame::Hello(Hello {
            device_id: "abc".into(),
            jwt: "jwt".into(),
        })),
    };
    let b = f.encode_to_vec();
    let back = WsClientFrame::decode(b.as_slice()).unwrap();
    assert!(matches!(back.frame, Some(ws_client_frame::Frame::Hello(_))));
}

#[test]
fn manifest_roundtrip() {
    let m = Manifest {
        version: mtdss_proto::MANIFEST_VERSION,
        transfer_id: "deadbeef".into(),
        files: vec![FileEntry {
            rel_path: "图😄片.jpg".into(),
            size: 3,
            sha256: vec![1; 32],
            chunks: vec![ChunkDesc { offset: 0, length: 3, sha256: vec![2; 32] }],
        }],
        sender_signature: vec![9; 64],
    };
    let b = m.encode_to_vec();
    let back = Manifest::decode(b.as_slice()).unwrap();
    assert_eq!(back.files[0].rel_path, "图😄片.jpg");
}

#[test]
fn envelope_roundtrip() {
    let e = SignalEnvelope { sender_device_id: "d1".into(), sealed: vec![7, 7] };
    let b = e.encode_to_vec();
    let back = SignalEnvelope::decode(b.as_slice()).unwrap();
    assert_eq!(back.sealed, vec![7, 7]);
}
