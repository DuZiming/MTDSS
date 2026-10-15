use crate::db::now;
use crate::AppState;
use anyhow::{anyhow, Result};
use ed25519_dalek::VerifyingKey;
use jsonwebtoken::{decode, encode, DecodingKey, EncodingKey, Header, Validation};
use rand::RngCore;
use serde::{Deserialize, Serialize};
use sha2::{Digest, Sha256};

#[derive(Serialize, Deserialize)]
pub struct Claims {
    pub sub: String, // account_id
    pub device: String,
    pub exp: usize,
    pub typ: String, // "access" | "refresh"
}

pub struct VerifyOk {
    pub account_id: String,
    pub device_id: String,
    pub jwt: String,
    pub refresh: String,
}

const CODE_TTL_SECS: i64 = 600;

pub fn request_code(state: &AppState, email: &str) -> Result<String> {
    let code = format!("{:06}", rand::thread_rng().next_u64() % 1_000_000);
    let mut h = Sha256::new();
    h.update(code.as_bytes());
    let conn = state.db.lock().unwrap();
    conn.execute(
        "DELETE FROM codes WHERE email=?1",
        rusqlite::params![email],
    )?;
    conn.execute(
        "INSERT INTO codes(email, code_hash, expires_at) VALUES(?1,?2,?3)",
        rusqlite::params![email, hex::encode(h.finalize()), now() + CODE_TTL_SECS],
    )?;
    tracing::info!(email = %email, "verification code issued (dev mode returns it)");
    Ok(code)
}

pub fn verify_code(
    state: &AppState,
    email: &str,
    code: &str,
    pubkey_hex: &str,
    name: &str,
    platform: &str,
) -> Result<VerifyOk> {
    let mut h = Sha256::new();
    h.update(code.as_bytes());
    let conn = state.db.lock().unwrap();
    let ok: bool = conn
        .query_row(
            "SELECT expires_at > ?3 FROM codes WHERE email=?1 AND code_hash=?2",
            rusqlite::params![email, hex::encode(h.finalize()), now()],
            |r| r.get(0),
        )
        .unwrap_or(false);
    if !ok {
        return Err(anyhow!("invalid or expired code"));
    }
    conn.execute("DELETE FROM codes WHERE email=?1", rusqlite::params![email])?;

    let account_id: String = match conn
        .query_row("SELECT id FROM accounts WHERE email=?1", [email], |r| r.get(0))
    {
        Ok(id) => id,
        Err(_) => {
            let id = hex::encode(rand::random::<[u8; 16]>());
            conn.execute(
                "INSERT INTO accounts(id, email, created_at) VALUES(?1,?2,?3)",
                rusqlite::params![id, email, now()],
            )?;
            id
        }
    };

    let pk_bytes = hex::decode(pubkey_hex)?;
    let pk: VerifyingKey = VerifyingKey::from_bytes(
        &pk_bytes
            .try_into()
            .map_err(|_| anyhow!("pubkey must be 32 bytes"))?,
    )?;
    let device_id = hex::encode(Sha256::digest(pk.as_bytes()))[..32].to_string();
    conn.execute(
        "INSERT INTO devices(account_id, device_id, pubkey, name, platform, created_at)
         VALUES(?1,?2,?3,?4,?5,?6)
         ON CONFLICT(device_id) DO UPDATE SET name=?4, platform=?5",
        rusqlite::params![account_id, device_id, pubkey_hex, name, platform, now()],
    )?;
    drop(conn);

    let jwt = mint_access_jwt(&state.jwt_secret, &account_id, &device_id);
    let refresh = mint_refresh_jwt(&state.jwt_secret, &account_id, &device_id);
    Ok(VerifyOk {
        account_id,
        device_id,
        jwt,
        refresh,
    })
}

pub fn mint_access_jwt(secret: &[u8; 32], account_id: &str, device_id: &str) -> String {
    mint(secret, account_id, device_id, "access", 60 * 60 * 24)
}
pub fn mint_refresh_jwt(secret: &[u8; 32], account_id: &str, device_id: &str) -> String {
    mint(secret, account_id, device_id, "refresh", 60 * 60 * 24 * 30)
}
fn mint(secret: &[u8; 32], account_id: &str, device_id: &str, typ: &str, ttl: usize) -> String {
    let c = Claims {
        sub: account_id.into(),
        device: device_id.into(),
        exp: (std::time::SystemTime::now()
            .duration_since(std::time::UNIX_EPOCH)
            .unwrap()
            .as_secs() as usize)
            + ttl,
        typ: typ.into(),
    };
    encode(&Header::default(), &c, &EncodingKey::from_secret(secret)).unwrap()
}

pub fn verify_jwt(secret: &[u8; 32], token: &str) -> Result<Claims> {
    let data = decode::<Claims>(
        token,
        &DecodingKey::from_secret(secret),
        &Validation::default(),
    )?;
    Ok(data.claims)
}

pub fn rotate_refresh(state: &AppState, refresh: &str) -> Result<VerifyOk> {
    let c = verify_jwt(&state.jwt_secret, refresh)?;
    if c.typ != "refresh" {
        return Err(anyhow!("not a refresh token"));
    }
    let conn = state.db.lock().unwrap();
    let device_id = c.device.clone();
    let account_id: String = conn.query_row(
        "SELECT account_id FROM devices WHERE device_id=?1",
        [&device_id],
        |r| r.get(0),
    )?;
    drop(conn);
    let jwt = mint_access_jwt(&state.jwt_secret, &c.sub, &device_id);
    let refresh = mint_refresh_jwt(&state.jwt_secret, &c.sub, &device_id);
    Ok(VerifyOk {
        account_id,
        device_id,
        jwt,
        refresh,
    })
}

// 测试钩子：把所有验证码置为过期
pub fn _expire_all_codes_for_test(state: &AppState) {
    state
        .db
        .lock()
        .unwrap()
        .execute("UPDATE codes SET expires_at = 0", [])
        .unwrap();
}
