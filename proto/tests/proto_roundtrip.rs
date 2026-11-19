use mtdss_proto::ctrl::*;
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

#[test]
fn ws_client_relay_release_roundtrip() {
    // v3p6 Task 7 显式释放面:RelayRelease{sid} 编解码往返,sid 保真
    let f = WsClientFrame {
        frame: Some(ws_client_frame::Frame::RelayRelease(RelayRelease { sid: 42 })),
    };
    let b = f.encode_to_vec();
    let back = WsClientFrame::decode(b.as_slice()).unwrap();
    match back.frame {
        Some(ws_client_frame::Frame::RelayRelease(rr)) => assert_eq!(rr.sid, 42),
        other => panic!("expected relay_release, got {other:?}"),
    }
}

// ---------- V4-1 预览波 Task 1:三组新消息的 golden 往返样例 ----------
//
// 契约期望:新消息一律经 CtrlRequest/CtrlResponse oneof 包裹往返(消息
// 包消息纪律);mtime 缺省形(0 = 不可得)必须与满形同等可解——旧端解新
// op 得 body=None、新端解缺省形得 0,互通不破。

/// 按需读:FileReadRangeRequest 满形逐字段保真 + mtime 缺省形(0)可解。
#[test]
fn ctrl_preview_read_range_roundtrip() {
    let req = CtrlRequest {
        version: mtdss_proto::CTRL_VERSION,
        body: Some(ctrl_request::Body::FileReadRange(FileReadRangeRequest {
            device_id: "dev-peer-fixed".into(),
            path: "/远端/契约 视频.mp4".into(),
            mtime: 1_760_000_000_000,
            size: 4096,
            offset: 1024,
            len: 512,
        })),
    };
    let back = CtrlRequest::decode(req.encode_to_vec().as_slice()).unwrap();
    match back.body {
        Some(ctrl_request::Body::FileReadRange(r)) => {
            assert_eq!(r.device_id, "dev-peer-fixed");
            assert_eq!(r.path, "/远端/契约 视频.mp4");
            assert_eq!(r.mtime, 1_760_000_000_000);
            assert_eq!(r.size, 4096);
            assert_eq!(r.offset, 1024);
            assert_eq!(r.len, 512);
        }
        other => panic!("expected file_read_range, got {other:?}"),
    }
    // mtime 缺省形:桥条目无 mtime → 缺省 0(handle 以 (path,size) 退化键)
    let req = CtrlRequest {
        version: mtdss_proto::CTRL_VERSION,
        body: Some(ctrl_request::Body::FileReadRange(FileReadRangeRequest {
            device_id: "dev-peer-fixed".into(),
            path: "/远端/无 mtime.bin".into(),
            mtime: 0,
            size: 8,
            offset: 0,
            len: 8,
        })),
    };
    let back = CtrlRequest::decode(req.encode_to_vec().as_slice()).unwrap();
    match back.body {
        Some(ctrl_request::Body::FileReadRange(r)) => {
            assert_eq!(r.mtime, 0, "缺省形必须可解且 mtime=0");
            assert_eq!(r.size, 8);
        }
        other => panic!("expected file_read_range, got {other:?}"),
    }
}

/// 缩略图:ThumbRequest(width 面)/ThumbResponse(jpeg 字节保真)经包裹
/// 往返;响应空字节 = 无缩略图(发起端回退类型图标)。
#[test]
fn ctrl_preview_thumb_roundtrip() {
    let req = CtrlRequest {
        version: mtdss_proto::CTRL_VERSION,
        body: Some(ctrl_request::Body::Thumb(ThumbRequest {
            path: "/远端/契约 图样.jpg".into(),
            mtime: 0, // 缺省形同往
            size: 4096,
            width: 256,
        })),
    };
    let back = CtrlRequest::decode(req.encode_to_vec().as_slice()).unwrap();
    match back.body {
        Some(ctrl_request::Body::Thumb(r)) => {
            assert_eq!(r.path, "/远端/契约 图样.jpg");
            assert_eq!(r.mtime, 0);
            assert_eq!(r.size, 4096);
            assert_eq!(r.width, 256);
        }
        other => panic!("expected thumb, got {other:?}"),
    }
    let resp = CtrlResponse {
        version: mtdss_proto::CTRL_VERSION,
        ok: true,
        body: Some(ctrl_response::Body::Thumb(ThumbResponse {
            jpeg: vec![0xFF, 0xD8, 0xFF, 0xE0],
        })),
    };
    let back = CtrlResponse::decode(resp.encode_to_vec().as_slice()).unwrap();
    match back.body {
        Some(ctrl_response::Body::Thumb(r)) => {
            assert_eq!(r.jpeg, vec![0xFF, 0xD8, 0xFF, 0xE0]);
            assert!(back.ok);
        }
        other => panic!("expected thumb response, got {other:?}"),
    }
    // 空字节形(失败回退面):解码出空 jpeg,ok 恒 true
    let resp = CtrlResponse {
        version: mtdss_proto::CTRL_VERSION,
        ok: true,
        body: Some(ctrl_response::Body::Thumb(ThumbResponse { jpeg: vec![] })),
    };
    let back = CtrlResponse::decode(resp.encode_to_vec().as_slice()).unwrap();
    match back.body {
        Some(ctrl_response::Body::Thumb(r)) => assert!(r.jpeg.is_empty()),
        other => panic!("expected thumb response, got {other:?}"),
    }
}

/// 转码会话:open 请求(target=HLS)/open 响应(session_id/segment_ms/mime)
/// /segment 请求/cancel 经包裹往返,session_id 保真。
#[test]
fn ctrl_preview_rendition_roundtrip() {
    // start_at_ms 显式形(V4-1 T15b:远跳重开,起点毫秒 = 目标段索引×段长)
    let req = CtrlRequest {
        version: mtdss_proto::CTRL_VERSION,
        body: Some(ctrl_request::Body::RenditionOpen(RenditionOpenRequest {
            path: "/远端/契约 影片.mov".into(),
            mtime: 1_760_000_000_000,
            size: 1 << 20,
            target: RenditionTarget::Hls as i32,
            start_at_ms: 48_000,
        })),
    };
    let back = CtrlRequest::decode(req.encode_to_vec().as_slice()).unwrap();
    match back.body {
        Some(ctrl_request::Body::RenditionOpen(r)) => {
            assert_eq!(r.path, "/远端/契约 影片.mov");
            assert_eq!(r.mtime, 1_760_000_000_000);
            assert_eq!(r.size, 1 << 20);
            assert_eq!(r.target(), RenditionTarget::Hls);
            assert_eq!(r.start_at_ms, 48_000, "起点毫秒显式形保真");
        }
        other => panic!("expected rendition_open, got {other:?}"),
    }
    // start_at_ms 缺省形(0 = 从头):旧端/旧客户端不填即此形,必须同等
    // 可解(互通不破的钉面)
    let req = CtrlRequest {
        version: mtdss_proto::CTRL_VERSION,
        body: Some(ctrl_request::Body::RenditionOpen(RenditionOpenRequest {
            path: "/远端/契约 影片.mov".into(),
            mtime: 1_760_000_000_000,
            size: 1 << 20,
            target: RenditionTarget::Hls as i32,
            start_at_ms: 0,
        })),
    };
    let back = CtrlRequest::decode(req.encode_to_vec().as_slice()).unwrap();
    match back.body {
        Some(ctrl_request::Body::RenditionOpen(r)) => {
            assert_eq!(r.start_at_ms, 0, "缺省形必须可解且 start_at_ms=0");
            assert_eq!(r.target(), RenditionTarget::Hls);
        }
        other => panic!("expected rendition_open, got {other:?}"),
    }
    let resp = CtrlResponse {
        version: mtdss_proto::CTRL_VERSION,
        ok: true,
        body: Some(ctrl_response::Body::RenditionOpen(RenditionOpenResponse {
            session_id: 0xDEAD_BEEF,
            segment_ms: 4000,
            mime: "video/iso.segment".into(),
        })),
    };
    let back = CtrlResponse::decode(resp.encode_to_vec().as_slice()).unwrap();
    match back.body {
        Some(ctrl_response::Body::RenditionOpen(r)) => {
            assert_eq!(r.session_id, 0xDEAD_BEEF);
            assert_eq!(r.segment_ms, 4000);
            assert_eq!(r.mime, "video/iso.segment");
        }
        other => panic!("expected rendition_open response, got {other:?}"),
    }
    // 拉段(响应=流式 chunk 帧,帧序在 T2,此处只锁请求形状)+ 取消
    for body in [
        ctrl_request::Body::RenditionSegment(RenditionSegmentRequest {
            session_id: 0xDEAD_BEEF,
            index: 7,
        }),
        ctrl_request::Body::RenditionCancel(RenditionCancel {
            session_id: 0xDEAD_BEEF,
        }),
    ] {
        let req = CtrlRequest {
            version: mtdss_proto::CTRL_VERSION,
            body: Some(body),
        };
        let back = CtrlRequest::decode(req.encode_to_vec().as_slice()).unwrap();
        match back.body {
            Some(ctrl_request::Body::RenditionSegment(r)) => {
                assert_eq!(r.session_id, 0xDEAD_BEEF);
                assert_eq!(r.index, 7);
            }
            Some(ctrl_request::Body::RenditionCancel(r)) => {
                assert_eq!(r.session_id, 0xDEAD_BEEF);
            }
            other => panic!("expected rendition segment/cancel, got {other:?}"),
        }
    }
}

// ---------- V4-1 预览波 Task 2:流式响应帧序的 golden 往返样例 ----------

/// 流式帧序:DataBegin/DataChunk/DataEnd 经 CtrlResponse 包裹往返,
/// stream_id 与 chunk 字节逐字节保真(响应端泵与发起端读循环的共同契约)。
#[test]
fn ctrl_stream_data_frames_roundtrip() {
    // 首帧:DataBegin{stream_id}
    let resp = CtrlResponse {
        version: mtdss_proto::CTRL_VERSION,
        ok: true,
        body: Some(ctrl_response::Body::DataBegin(DataBegin { stream_id: 7 })),
    };
    let back = CtrlResponse::decode(resp.encode_to_vec().as_slice()).unwrap();
    match back.body {
        Some(ctrl_response::Body::DataBegin(b)) => assert_eq!(b.stream_id, 7),
        other => panic!("expected data_begin, got {other:?}"),
    }
    // 若干 DataChunk{stream_id,bytes}(含空块形:零长读段)
    for data in [vec![1u8, 2, 3], Vec::new()] {
        let resp = CtrlResponse {
            version: mtdss_proto::CTRL_VERSION,
            ok: true,
            body: Some(ctrl_response::Body::DataChunk(DataChunk {
                stream_id: 7,
                data: data.clone(),
            })),
        };
        let back = CtrlResponse::decode(resp.encode_to_vec().as_slice()).unwrap();
        match back.body {
            Some(ctrl_response::Body::DataChunk(c)) => {
                assert_eq!(c.stream_id, 7);
                assert_eq!(c.data, data);
            }
            other => panic!("expected data_chunk, got {other:?}"),
        }
    }
    // 末帧:DataEnd{stream_id}
    let resp = CtrlResponse {
        version: mtdss_proto::CTRL_VERSION,
        ok: true,
        body: Some(ctrl_response::Body::DataEnd(DataEnd { stream_id: 7 })),
    };
    let back = CtrlResponse::decode(resp.encode_to_vec().as_slice()).unwrap();
    match back.body {
        Some(ctrl_response::Body::DataEnd(e)) => assert_eq!(e.stream_id, 7),
        other => panic!("expected data_end, got {other:?}"),
    }
}
