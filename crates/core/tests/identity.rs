use ed25519_dalek::Verifier;
use mtdss_core::identity::*;

#[test]
fn generate_save_load_roundtrip() {
    let dir = tempfile::tempdir().unwrap();
    let p = dir.path().join("seed.hex");
    let a = DeviceIdentity::generate();
    a.save_seed(&p).unwrap();
    let b = DeviceIdentity::from_seed_file(&p).unwrap();
    assert_eq!(a.device_id, b.device_id);
    assert_eq!(a.public_key(), b.public_key());
}

#[test]
fn device_id_is_derived_from_pubkey() {
    let a = DeviceIdentity::generate();
    assert_eq!(a.device_id.len(), 32);
    assert_eq!(a.device_id, device_id_for(&a.public_key()));
    assert_ne!(a.device_id, DeviceIdentity::generate().device_id);
}

#[test]
fn cert_spki_matches_pubkey() {
    let a = DeviceIdentity::generate();
    let der = a.cert_der();
    let (_, cert) = x509_parser::parse_x509_certificate(&der).unwrap();
    let spki = cert.public_key().raw;
    assert_eq!(spki, spki_ed25519(&a.public_key()).as_slice());
}

#[test]
fn cert_der_is_stable_per_identity() {
    let a = DeviceIdentity::generate();
    assert_eq!(a.cert_der(), a.cert_der());
}

#[test]
fn sign_verify() {
    let a = DeviceIdentity::generate();
    let sig = a.sign(b"hello");
    assert!(a.public_key().verify(b"hello", &sig).is_ok());
    assert!(a.public_key().verify(b"hellO", &sig).is_err());
}
