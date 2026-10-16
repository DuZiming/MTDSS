use mtdss_core::identity::DeviceIdentity;
use mtdss_core::protocol::manifest::*;
use mtdss_proto::manifest::{ChunkDesc, FileEntry};
use sha2::Digest;
use std::io::Write;

fn write_tmp(dir: &std::path::Path, name: &str, size: usize) -> std::path::PathBuf {
    let p = dir.join(name);
    let mut f = std::fs::File::create(&p).unwrap();
    let mut rng_state: u64 = size as u64 + 0x9E3779B97F4A7C15;
    let mut buf = vec![0u8; 65536];
    for chunk in buf.chunks_mut(64) {
        for b in chunk.iter_mut() {
            rng_state ^= rng_state << 13;
            rng_state ^= rng_state >> 7;
            rng_state ^= rng_state << 17;
            *b = (rng_state & 0xFF) as u8;
        }
    }
    let mut written = 0;
    while written < size {
        let n = std::cmp::min(65536, size - written);
        f.write_all(&buf[..n]).unwrap();
        written += n;
    }
    p
}

#[test]
fn single_chunk_file() {
    let dir = tempfile::tempdir().unwrap();
    let p = write_tmp(dir.path(), "small.bin", 1000);
    let id = DeviceIdentity::generate();
    let m = build_single(&p, "small.bin", &id).unwrap();
    assert_eq!(m.version, mtdss_proto::MANIFEST_VERSION);
    assert_eq!(m.files.len(), 1);
    let e = &m.files[0];
    assert_eq!(e.size, 1000);
    assert_eq!(e.chunks.len(), 1);
    assert_eq!(e.chunks[0].length, 1000);
    let v = verify(&m, &id.public_key()).unwrap();
    assert_eq!(v[0].rel_path, "small.bin");
}

#[test]
fn multi_chunk_and_exact_chunk_hashes() {
    let dir = tempfile::tempdir().unwrap();
    let size = CHUNK_SIZE * 2 + 12345;
    let p = write_tmp(dir.path(), "big.bin", size);
    let id = DeviceIdentity::generate();
    let m = build_single(&p, "big.bin", &id).unwrap();
    assert_eq!(m.files[0].chunks.len(), 3);
    assert_eq!(m.files[0].chunks[2].length as usize, 12345);
    verify(&m, &id.public_key()).unwrap();
    // 分块哈希确实来自内容
    let data = std::fs::read(&p).unwrap();
    let mut h = sha2::Sha256::new();
    h.update(&data[..CHUNK_SIZE]);
    assert_eq!(m.files[0].chunks[0].sha256, h.finalize().to_vec());
}

#[test]
fn zero_byte_file_manifest() {
    let dir = tempfile::tempdir().unwrap();
    let p = dir.path().join("empty");
    std::fs::write(&p, b"").unwrap();
    let id = DeviceIdentity::generate();
    let m = build_single(&p, "empty", &id).unwrap();
    assert_eq!(m.files[0].size, 0);
    assert_eq!(m.files[0].chunks.len(), 0);
    verify(&m, &id.public_key()).unwrap();
}

#[test]
fn unicode_rel_path_allowed() {
    let dir = tempfile::tempdir().unwrap();
    let name = "图😄片/照片 v2.jpg";
    let sub = dir.path().join("图😄片");
    std::fs::create_dir(&sub).unwrap();
    std::fs::write(sub.join("照片 v2.jpg"), b"x").unwrap();
    let id = DeviceIdentity::generate();
    let m = build_single(&sub.join("照片 v2.jpg"), name, &id).unwrap();
    assert_eq!(m.files[0].rel_path, name);
    assert!(validate_rel_path(name));
}

#[test]
fn traversal_paths_rejected() {
    assert!(!validate_rel_path("../evil"));
    assert!(!validate_rel_path("a/../../evil"));
    assert!(!validate_rel_path("/abs"));
    assert!(!validate_rel_path("C:\\abs"));
    assert!(!validate_rel_path("a\\b"));
    assert!(!validate_rel_path(""));
    assert!(!validate_rel_path("."));
    assert!(validate_rel_path("a/b/c.txt"));
    assert!(validate_rel_path(".hidden"));
}

#[test]
fn tampered_manifest_fails_signature() {
    let dir = tempfile::tempdir().unwrap();
    let p = write_tmp(dir.path(), "f", 10);
    let id = DeviceIdentity::generate();
    let mut m = build_single(&p, "f", &id).unwrap();
    m.files[0].size = 999999; // 篡改
    assert!(verify(&m, &id.public_key()).is_err());
}

#[test]
fn wrong_signer_rejected() {
    let dir = tempfile::tempdir().unwrap();
    let p = write_tmp(dir.path(), "f", 10);
    let id = DeviceIdentity::generate();
    let other = DeviceIdentity::generate();
    let m = build_single(&p, "f", &id).unwrap();
    assert!(verify(&m, &other.public_key()).is_err());
}

fn entry(rel: &str, size_le: [u8; 8], hash: Vec<u8>) -> FileEntry {
    FileEntry {
        rel_path: rel.to_string(),
        size: u64::from_le_bytes(size_le),
        sha256: hash,
        chunks: vec![],
    }
}

#[test]
fn signable_bytes_v2_golden() {
    let files = vec![FileEntry {
        rel_path: "f".into(),
        size: 1,
        sha256: vec![7u8; 32],
        chunks: vec![ChunkDesc { offset: 0, length: 1, sha256: vec![9u8; 32] }],
    }];
    let got = signable_bytes("ab", &files);
    let mut want = Vec::new();
    want.extend_from_slice(b"MTDSS-MAN-SIG-V2");
    want.extend_from_slice(&2u32.to_be_bytes()); // len(transfer_id)
    want.extend_from_slice(b"ab");
    want.extend_from_slice(&1u32.to_be_bytes()); // file_count
    want.extend_from_slice(&1u32.to_be_bytes()); // len(rel_path)
    want.extend_from_slice(b"f");
    want.extend_from_slice(&1u64.to_le_bytes()); // size
    want.extend_from_slice(&[7u8; 32]); // 整文件哈希
    want.extend_from_slice(&1u32.to_be_bytes()); // chunk_count
    want.extend_from_slice(&0u64.to_le_bytes()); // offset
    want.extend_from_slice(&1u64.to_le_bytes()); // length
    want.extend_from_slice(&[9u8; 32]); // 块哈希
    assert_eq!(got, want);
}

#[test]
fn signable_bytes_v2_disambiguates_rel_path_boundaries() {
    // v1 串接格式下字节完全相同的两个清单,v2 必须产生不同签名字节。
    // A 布局: "a" || S1(8) || H1(32) || "bc" || S2(8) || H2(32)
    // B 布局: "ab" || S1'(8) || H1'(32) || "c" || S2(8) || H2(32)
    // 其中 S1[0]='b'、S1' = S1[1..8]||H1[0]、H1' = H1[1..32]||'b',逐字节重排相等。
    let h1: Vec<u8> = (0u8..32).collect();
    let s2 = [7u8; 8];
    let h2: Vec<u8> = (32u8..64).collect();
    let mut s1 = [0u8; 8];
    s1[0] = b'b';
    let a_files = vec![entry("a", s1, h1.clone()), entry("bc", s2, h2.clone())];

    let b_f1_size: [u8; 8] = {
        let mut v = s1[1..8].to_vec();
        v.push(h1[0]);
        v.try_into().unwrap()
    };
    let b_f1_hash = {
        let mut v = h1[1..32].to_vec();
        v.push(b'b');
        v
    };
    let b_files = vec![entry("ab", b_f1_size, b_f1_hash), entry("c", s2, h2.clone())];

    // 测试内复刻 v1 串接(无长度前缀),证明输入对确实在 v1 下碰撞
    let v1 = |files: &[FileEntry]| -> Vec<u8> {
        let mut out = b"tid".to_vec();
        for f in files {
            out.extend_from_slice(f.rel_path.as_bytes());
            out.extend_from_slice(&f.size.to_le_bytes());
            out.extend_from_slice(&f.sha256);
        }
        out
    };
    assert_eq!(v1(&a_files), v1(&b_files), "sanity: v1 collides on this pair");
    assert_ne!(
        signable_bytes("tid", &a_files),
        signable_bytes("tid", &b_files),
        "v2 must disambiguate"
    );
}

#[test]
fn verify_rejects_v1_manifest() {
    let dir = tempfile::tempdir().unwrap();
    let p = write_tmp(dir.path(), "v1.bin", 10);
    let id = DeviceIdentity::generate();
    let mut m = build_single(&p, "v1.bin", &id).unwrap();
    m.version = 1;
    let err = verify(&m, &id.public_key()).unwrap_err();
    assert!(err.to_string().contains("unsupported manifest version"));
}
