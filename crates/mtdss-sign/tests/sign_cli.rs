//! mtdss-sign CLI 联测:直接跑构建产物二进制(`CARGO_BIN_EXE_mtdss-sign`)。
//!
//! 主链:keygen → manifest → sign → verify 全链退出码 0,且中间产物内容
//! (url 形态、sha256、size、signature 空串/有值)逐项核对;负路径:篡改
//! 已签清单一字符 → verify 退出码 1、错公钥 → 退出码 1、非法版本 →
//! manifest 子命令报错退出、keygen 遇已存在密钥文件 → 拒绝退出。

use std::path::{Path, PathBuf};
use std::process::{Command, Output};

const PAYLOAD: &[u8] = b"PE\0fake installer payload";
const VERSION: &str = "1.2.3";

fn run(args: &[&str]) -> Output {
    Command::new(env!("CARGO_BIN_EXE_mtdss-sign"))
        .args(args)
        .output()
        .expect("spawn mtdss-sign")
}

fn show(out: &Output) -> String {
    format!(
        "exit={:?}\n-- stdout --\n{}\n-- stderr --\n{}",
        out.status.code(),
        String::from_utf8_lossy(&out.stdout),
        String::from_utf8_lossy(&out.stderr),
    )
}

fn assert_exit0(out: &Output, what: &str) {
    assert!(out.status.success(), "{what}: expected exit 0, got {}", show(out));
}

fn assert_exit1(out: &Output, what: &str) {
    assert_eq!(out.status.code(), Some(1), "{what}: expected exit 1, got {}", show(out));
}

fn read(p: &Path) -> String {
    std::fs::read_to_string(p).unwrap_or_else(|e| panic!("read {}: {e}", p.display()))
}

/// keygen 一次,断言成功且两枚 hex 文件落盘,返回 (pub, priv) 路径。
fn keygen(dir: &Path) -> (PathBuf, PathBuf) {
    let out = run(&["keygen", "--out", dir.to_str().unwrap()]);
    assert_exit0(&out, "keygen");
    let pub_p = dir.join("update-pub.hex");
    let priv_p = dir.join("update-priv.hex");
    assert!(pub_p.is_file(), "update-pub.hex must be written");
    assert!(priv_p.is_file(), "update-priv.hex must be written");
    assert_eq!(read(&pub_p).trim().len(), 64, "pub key is 64 hex chars");
    assert_eq!(read(&priv_p).trim().len(), 64, "priv key is 64 hex chars");
    (pub_p, priv_p)
}

/// 造 notes + 资产文件,跑 manifest 子命令,返回未签名清单路径。
fn make_manifest(dir: &Path) -> PathBuf {
    let asset = dir.join("setup.exe");
    std::fs::write(&asset, PAYLOAD).unwrap();
    let notes = dir.join("notes.md");
    std::fs::write(&notes, "- bug fixes\n- faster startup\n").unwrap();
    let unsigned = dir.join("unsigned.json");
    let out = run(&[
        "manifest",
        "--version",
        VERSION,
        "--notes-file",
        notes.to_str().unwrap(),
        "--url-base",
        "https://dl.example.invalid/dl",
        "--file",
        asset.to_str().unwrap(),
        "--platform",
        "windows-x86_64",
        "--out",
        unsigned.to_str().unwrap(),
    ]);
    assert_exit0(&out, "manifest");
    unsigned
}

/// 全链前置:keygen + manifest + sign,返回 (pub, priv, signed manifest)。
fn chain(dir: &Path) -> (PathBuf, PathBuf, PathBuf) {
    let (pub_p, priv_p) = keygen(dir);
    let unsigned = make_manifest(dir);
    let signed = dir.join("manifest.json");
    let out = run(&[
        "sign",
        "--manifest",
        unsigned.to_str().unwrap(),
        "--key",
        priv_p.to_str().unwrap(),
        "--out",
        signed.to_str().unwrap(),
    ]);
    assert_exit0(&out, "sign");
    (pub_p, priv_p, signed)
}

#[test]
fn keygen_manifest_sign_verify_chain() {
    let tmp = tempfile::tempdir().unwrap();
    let dir = tmp.path();

    let (pub_p, _priv_p, signed) = chain(dir);

    // 未签名清单内容:signature 为空串,url/sha256/size/notes 逐项核对
    let unsigned_json: serde_json::Value =
        serde_json::from_str(&read(&dir.join("unsigned.json"))).unwrap();
    assert_eq!(unsigned_json["version"], VERSION);
    assert_eq!(unsigned_json["signature"], "", "unsigned manifest carries empty signature");
    assert_eq!(unsigned_json["notes"], "- bug fixes\n- faster startup\n");
    let asset = unsigned_json["platforms"]["windows-x86_64"].clone();
    assert_eq!(
        asset["url"],
        "https://dl.example.invalid/dl/win/1.2.3/setup.exe",
        "asset url = {{url-base}}/win/{{ver}}/{{file name}}"
    );
    use sha2::{Digest, Sha256};
    let expect_sha = hex::encode(Sha256::digest(PAYLOAD));
    assert_eq!(asset["sha256"], expect_sha);
    assert_eq!(asset["size"].as_u64(), Some(PAYLOAD.len() as u64));
    assert!(unsigned_json["pubdate"].as_str().unwrap().ends_with('Z'), "pubdate is UTC RFC3339");

    // 已签清单:signature 变为 128 hex 字符
    let signed_json: serde_json::Value = serde_json::from_str(&read(&signed)).unwrap();
    assert_eq!(signed_json["signature"].as_str().unwrap().len(), 128);

    // verify:正确公钥 → 0
    let out = run(&[
        "verify",
        "--manifest",
        signed.to_str().unwrap(),
        "--pubkey",
        pub_p.to_str().unwrap(),
    ]);
    assert_exit0(&out, "verify with matching pubkey");
}

#[test]
fn tampered_manifest_fails_verify() {
    let tmp = tempfile::tempdir().unwrap();
    let dir = tmp.path();
    let (pub_p, _priv_p, signed) = chain(dir);

    // 篡改恰好一字符(version 段 3→4;签名覆盖 version,必炸)
    let content = read(&signed);
    assert!(
        content.contains(r#""version": "1.2.3""#),
        "pretty JSON keeps the version field intact: {content}"
    );
    let tampered = content.replacen(r#""version": "1.2.3""#, r#""version": "1.2.4""#, 1);
    assert_ne!(content, tampered, "tamper must actually change the file");
    std::fs::write(&signed, tampered).unwrap();

    let out = run(&[
        "verify",
        "--manifest",
        signed.to_str().unwrap(),
        "--pubkey",
        pub_p.to_str().unwrap(),
    ]);
    assert_exit1(&out, "verify after tamper");
}

#[test]
fn wrong_pubkey_fails_verify() {
    let tmp = tempfile::tempdir().unwrap();
    let dir = tmp.path();
    let (_pub_a, _priv_a, signed) = chain(&dir.join("a"));
    let (pub_b, _priv_b) = keygen(&dir.join("b"));

    let out = run(&[
        "verify",
        "--manifest",
        signed.to_str().unwrap(),
        "--pubkey",
        pub_b.to_str().unwrap(),
    ]);
    assert_exit1(&out, "verify with foreign pubkey");
}

#[test]
fn manifest_rejects_non_three_segment_version() {
    let tmp = tempfile::tempdir().unwrap();
    let dir = tmp.path();
    let asset = dir.join("setup.exe");
    std::fs::write(&asset, PAYLOAD).unwrap();
    let notes = dir.join("notes.md");
    std::fs::write(&notes, "n").unwrap();
    let out_path = dir.join("unsigned.json");

    for bad in ["1.0", "1.0.0.0", "v1.0.0", "1.0.0-rc1", "", "1..0"] {
        let out = run(&[
            "manifest",
            "--version",
            bad,
            "--notes-file",
            notes.to_str().unwrap(),
            "--url-base",
            "https://dl.example.invalid/dl",
            "--file",
            asset.to_str().unwrap(),
            "--out",
            out_path.to_str().unwrap(),
        ]);
        assert_exit1(&out, &format!("manifest --version {bad:?} must be rejected"));
        let err = String::from_utf8_lossy(&out.stderr);
        assert!(err.contains("version"), "stderr should mention version: {err}");
        assert!(!out_path.exists(), "no manifest may be written for invalid version");
    }
}

#[test]
fn keygen_refuses_existing_key_files() {
    let tmp = tempfile::tempdir().unwrap();
    let dir = tmp.path();
    let (pub_p, priv_p) = keygen(dir);
    let pub_before = read(&pub_p);
    let priv_before = read(&priv_p);

    let out = run(&["keygen", "--out", dir.to_str().unwrap()]);
    assert_exit1(&out, "second keygen into same dir must be refused");
    assert_eq!(read(&pub_p), pub_before, "existing pub key must stay untouched");
    assert_eq!(read(&priv_p), priv_before, "existing priv key must stay untouched");
}
