use mtdss_core::identity::DeviceIdentity;
use mtdss_core::protocol::manifest::*;
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
