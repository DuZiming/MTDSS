use crate::db::now;
use crate::AppState;
use anyhow::{anyhow, Result};
use ed25519_dalek::VerifyingKey;
use jsonwebtoken::{decode, encode, DecodingKey, EncodingKey, Header, Validation};
use mtdss_proto::ws::AuthNotice;
use rand::RngCore;
use rusqlite::{Connection, OptionalExtension};
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

/// verify 三形态之审批挂起面(响应 `{status:"approval", session_id, expires_at}`)。
pub struct ApprovalPending {
    pub session_id: String,
    pub expires_at: i64,
}

/// verify 结果:直入(老终端/自动受信)铸票,或建审批会话。
/// limit 形态(enroll_token)归 T5 配额门,本任务直入路径占位直通 ok。
pub enum VerifyOutcome {
    Ok(VerifyOk),
    Approval(ApprovalPending),
}

/// login-status 轮询结果。Ok(VerifyOk) 为单次消费:approved 会话首次轮询
/// 领票并立即转终态,再轮询恒 Expired。
pub enum LoginStatus {
    Approval { expires_at: i64 },
    Denied,
    Expired,
    Ok(VerifyOk),
}

const CODE_TTL_SECS: i64 = 600;
/// 码错作废阈值(spec §3.10):错满 5 次删行,原码再对也不放行;
/// 重新 request-code 即重置(旧行被覆盖)。
const MAX_CODE_ATTEMPTS: i64 = 5;
/// 审批会话 TTL(spec §3.10 记录人裁量清单 2):300s 无裁决即 expired,
/// 不自动降级直入。
const APPROVAL_TTL_SECS: i64 = 300;
/// 审批码错作废阈值(spec §3.10):approvals.attempts 错满 5 次会话作废,
/// 须重新发起登录。
const MAX_APPROVAL_ATTEMPTS: i64 = 5;

/// 生成 6 位数字验证码。
pub fn gen_code() -> String {
    format!("{:06}", rand::thread_rng().next_u64() % 1_000_000)
}

/// 落库(哈希存储,TTL 600s;覆盖同邮箱旧行)。调用方必须已成功发信
/// (dev 回显门除外)——spec §3.5:发信成功才落库,失败 500 不产半态。
pub fn store_code(state: &AppState, email: &str, code: &str) -> Result<()> {
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
    Ok(())
}

pub fn verify_code(
    state: &AppState,
    email: &str,
    code: &str,
    pubkey_hex: &str,
    name: &str,
    platform: &str,
) -> Result<VerifyOutcome> {
    let mut h = Sha256::new();
    h.update(code.as_bytes());
    let account_id: String = {
        let conn = state.db.lock().unwrap();
        let ok: bool = conn
            .query_row(
                "SELECT expires_at > ?3 FROM codes WHERE email=?1 AND code_hash=?2",
                rusqlite::params![email, hex::encode(h.finalize()), now()],
                |r| r.get(0),
            )
            .unwrap_or(false);
        if !ok {
            // 码错/过期计一次尝试;错满 5 次作废(删行,原码再对也不放行;
            // 行已不在则两条语句各匹配 0 行,同为 Err 语义)
            conn.execute(
                "UPDATE codes SET attempts=attempts+1 WHERE email=?1",
                rusqlite::params![email],
            )?;
            conn.execute(
                "DELETE FROM codes WHERE email=?1 AND attempts>=?2",
                rusqlite::params![email, MAX_CODE_ATTEMPTS],
            )?;
            return Err(anyhow!("invalid or expired code"));
        }
        conn.execute("DELETE FROM codes WHERE email=?1", rusqlite::params![email])?;

        // 认证收口:账号必须已预置,不再自动建号(防已删账号经有效验证码复活;
        // 正常流 request-code 已在入口 404 拦截,这里兜"发码后账号被删"的窗口)
        conn.query_row("SELECT id FROM accounts WHERE email=?1", [email], |r| r.get(0))
            .map_err(|_| anyhow!("unknown account"))?
    };

    let pk_bytes = hex::decode(pubkey_hex)?;
    let pk: VerifyingKey = VerifyingKey::from_bytes(
        &pk_bytes
            .try_into()
            .map_err(|_| anyhow!("pubkey must be 32 bytes"))?,
    )?;
    let device_id = hex::encode(Sha256::digest(pk.as_bytes()))[..32].to_string();

    // 信任判定(spec §3.1,verify 时刻裁定):老终端(device 行已属本账号)直入;
    // 其余看 presence 快照——新终端且快照无本账号在线受信终端 = 自动受信直插;
    // 已属他账号的 pubkey 恒走审批(直入分支「row 不存在 OR 已属本账号」守卫)。
    let owner: Option<String> = {
        let conn = state.db.lock().unwrap();
        conn.query_row(
            "SELECT account_id FROM devices WHERE device_id=?1",
            [&device_id],
            |r| r.get(0),
        )
        .optional()?
    };
    let peers = state.presence.account_peers(&account_id);
    let direct = match owner {
        // 老终端重登:直接铸票,与快照无关
        Some(a) => a == account_id,
        // 新终端:快照空(无在线受信端)= 自动受信
        None => peers.is_empty(),
        // 换绑(已属他账号):true 分支不可达,恒走审批
    };
    if direct {
        let ok = enroll_and_mint(state, &account_id, &device_id, pubkey_hex, name, platform)?;
        return Ok(VerifyOutcome::Ok(ok));
    }

    // 审批会话(spec §3.1 ②):建行(6 位码,300s)+ 对全端尽力推 join_request。
    // 同 pubkey 重复 verify = 作废旧会话建新会话(重推)。会话一旦存在只认
    // approvals 行裁决,后续受信端掉线不降级(fail-closed,见 login_status)。
    let approval_code = gen_code();
    let mut h = Sha256::new();
    h.update(approval_code.as_bytes());
    let session_id = hex::encode(rand::random::<[u8; 16]>());
    let expires_at = now() + APPROVAL_TTL_SECS;
    {
        let conn = state.db.lock().unwrap();
        conn.execute(
            "UPDATE approvals SET state='expired'
             WHERE account_id=?1 AND pubkey=?2 AND state='pending'",
            rusqlite::params![account_id, pubkey_hex],
        )?;
        conn.execute(
            "INSERT INTO approvals(id, account_id, pubkey, name, platform, code_hash,
                                   attempts, state, expires_at)
             VALUES(?1,?2,?3,?4,?5,?6,0,'pending',?7)",
            rusqlite::params![
                session_id,
                account_id,
                pubkey_hex,
                name,
                platform,
                hex::encode(h.finalize()),
                expires_at
            ],
        )?;
    }
    let notice = AuthNotice {
        kind: "join_request".into(),
        session_id: session_id.clone(),
        code: approval_code,
        candidate_name: name.into(),
        candidate_platform: platform.into(),
        expires_at,
        data_key: String::new(),
    };
    for (peer_device, _peer) in &peers {
        crate::ws::push_notice(state, peer_device, notice.clone());
    }
    Ok(VerifyOutcome::Approval(ApprovalPending {
        session_id,
        expires_at,
    }))
}

/// 设备行插入 + 铸票的唯一收口(email 直入 / 审批 approved 轮询两路共用;
/// T5 配额门在此包一层成 enroll_device 三路径收口)。upsert 保持既有
/// ON CONFLICT 语义:换绑迁移 account_id 只经审批 approved 路径可达。
fn enroll_and_mint(
    state: &AppState,
    account_id: &str,
    device_id: &str,
    pubkey_hex: &str,
    name: &str,
    platform: &str,
) -> Result<VerifyOk> {
    {
        let conn = state.db.lock().unwrap();
        // 同一 device_id（= 同一 pubkey 身份）整体迁移到当前账号:DO UPDATE
        // 漏更新 account_id 会让 JWT 声称新账号而库里仍挂旧账号——设备列表
        // 为空、WS hello 恒 code 1、恢复流程死锁。v3p1 起已属他账号的 pubkey
        // 恒走审批,迁移仅经 approved 路径可达(verify 直入分支有归属守卫)。
        conn.execute(
            "INSERT INTO devices(account_id, device_id, pubkey, name, platform, created_at)
             VALUES(?1,?2,?3,?4,?5,?6)
             ON CONFLICT(device_id) DO UPDATE SET account_id=?1, name=?4, platform=?5",
            rusqlite::params![account_id, device_id, pubkey_hex, name, platform, now()],
        )?;
    }
    let jwt = mint_access_jwt(&state.jwt_secret, account_id, device_id);
    let refresh = mint_refresh_jwt(&state.jwt_secret, account_id, device_id);
    Ok(VerifyOk {
        account_id: account_id.into(),
        device_id: device_id.into(),
        jwt,
        refresh,
    })
}

// ---------- 审批会话操作(spec §3.1 settling,先到先裁 / 单次有效) ----------

/// approve-code 错误面:未知会话 / 已裁决(终态或过期)/ 码错 / 库错。
pub enum ApproveCodeErr {
    UnknownSession,
    NotPending,
    WrongCode,
    Db(rusqlite::Error),
}

/// 新终端直接输码(免认证):对上 approvals.code_hash → state='approved'。
/// 错码计 attempts,错满 5 次会话作废(原码再对也不放行)。settle 为
/// 「state='pending' 且未过期」守卫下的原子 UPDATE,与 approval-respond
/// 先到先裁;输 0 行 = 已被他路裁决或已过期。
pub fn approve_with_code(state: &AppState, session_id: &str, code: &str) -> Result<(), ApproveCodeErr> {
    let mut h = Sha256::new();
    h.update(code.as_bytes());
    let hash = hex::encode(h.finalize());
    let conn = state.db.lock().unwrap();
    let Some((st, _)) = session_state(&conn, session_id).map_err(ApproveCodeErr::Db)? else {
        return Err(ApproveCodeErr::UnknownSession);
    };
    if st != "pending" {
        return Err(ApproveCodeErr::NotPending);
    }
    let stored: String = conn
        .query_row("SELECT code_hash FROM approvals WHERE id=?1", [session_id], |r| r.get(0))
        .map_err(ApproveCodeErr::Db)?;
    if stored != hash {
        // 错码计数;错满 5 次作废(作废后原码再对也走 NotPending)
        conn.execute(
            "UPDATE approvals SET attempts=attempts+1 WHERE id=?1 AND state='pending'",
            [session_id],
        )
        .map_err(ApproveCodeErr::Db)?;
        conn.execute(
            "UPDATE approvals SET state='expired'
             WHERE id=?1 AND state='pending' AND attempts>=?2",
            rusqlite::params![session_id, MAX_APPROVAL_ATTEMPTS],
        )
        .map_err(ApproveCodeErr::Db)?;
        return Err(ApproveCodeErr::WrongCode);
    }
    let settled = conn
        .execute(
            "UPDATE approvals SET state='approved'
             WHERE id=?1 AND state='pending' AND expires_at>?2",
            rusqlite::params![session_id, now()],
        )
        .map_err(ApproveCodeErr::Db)?;
    if settled == 1 {
        Ok(())
    } else {
        Err(ApproveCodeErr::NotPending)
    }
}

/// approval-respond 错误面:未知会话 / 已裁决 / 响应者非该账号在线受信设备。
pub enum RespondErr {
    UnknownSession,
    NotPending,
    Forbidden,
    Db(rusqlite::Error),
}

/// 受信终端裁决(Bearer 面由 api 层验签,这里做归属/在线/受信复核):
/// 响应者必须是会话所属账号的、devices 行在册且此刻 WS 在线的设备。
/// allow=true→approved,false→denied(一票否决);同一「pending 且未过期」
/// 守卫与 approve-code 先到先裁。
pub fn approval_respond(
    state: &AppState,
    responder_account: &str,
    responder_device: &str,
    session_id: &str,
    allow: bool,
) -> Result<(), RespondErr> {
    let conn = state.db.lock().unwrap();
    let row: Option<(String, String, i64)> = conn
        .query_row(
            "SELECT account_id, state, expires_at FROM approvals WHERE id=?1",
            [session_id],
            |r| Ok((r.get(0)?, r.get(1)?, r.get(2)?)),
        )
        .optional()
        .map_err(RespondErr::Db)?;
    let Some((account_id, st, expires_at)) = row else {
        return Err(RespondErr::UnknownSession);
    };
    // 须为该账号的在线受信设备(spec §3.1):devices 行在册(受信)+ presence
    // 在场(在线)——离线 JWT 持有者不得裁决
    let trusted: bool = conn
        .query_row(
            "SELECT 1 FROM devices WHERE device_id=?1 AND account_id=?2",
            rusqlite::params![responder_device, responder_account],
            |r| r.get::<_, i64>(0),
        )
        .is_ok();
    if account_id != responder_account || !trusted || !state.presence.is_online(responder_device) {
        return Err(RespondErr::Forbidden);
    }
    if expired_pending(&st, expires_at) {
        mark_expired(&conn, session_id).map_err(RespondErr::Db)?;
    }
    let st = effective_state(&st, expires_at);
    if st != "pending" {
        return Err(RespondErr::NotPending);
    }
    let target = if allow { "approved" } else { "denied" };
    let settled = conn
        .execute(
            "UPDATE approvals SET state=?2 WHERE id=?1 AND state='pending' AND expires_at>?3",
            rusqlite::params![session_id, target, now()],
        )
        .map_err(RespondErr::Db)?;
    if settled == 1 {
        Ok(())
    } else {
        Err(RespondErr::NotPending)
    }
}

/// login-status 轮询(免认证):只认 approvals 行,不重查 presence
/// (快照后受信端全掉线不降级)。approved → 单次领票:先原子消费
/// (state 转 'expired')再 enroll+铸票,并发/重复轮询只有一次成功。
pub fn login_status(state: &AppState, session_id: &str) -> Result<LoginStatus> {
    enum Phase {
        Pending(i64),
        Terminal(&'static str),
        Approve {
            account_id: String,
            pubkey: String,
            name: String,
            platform: String,
        },
    }
    let phase = {
        let conn = state.db.lock().unwrap();
        let row: Option<(String, i64, String, String, String, String)> = conn
            .query_row(
                "SELECT state, expires_at, account_id, pubkey, name, platform
                 FROM approvals WHERE id=?1",
                [session_id],
                |r| Ok((r.get(0)?, r.get(1)?, r.get(2)?, r.get(3)?, r.get(4)?, r.get(5)?)),
            )
            .optional()?;
        let Some((st, expires_at, account_id, pubkey, name, platform)) = row else {
            // 未知/已消费会话按终态呈现:轮询面无 404 通道,客户端据此收口重登
            return Ok(LoginStatus::Expired);
        };
        if expired_pending(&st, expires_at) {
            mark_expired(&conn, session_id)?;
        }
        match effective_state(&st, expires_at).as_str() {
            "pending" => Phase::Pending(expires_at),
            "denied" => Phase::Terminal("denied"),
            "expired" => Phase::Terminal("expired"),
            // 单次有效:approved 只发一次票。先原子消费再铸票(拿锁内 UPDATE
            // 行数裁定唯一赢家),消费后即使 enroll 失败也 fail-closed——会话
            // 已终态,客户端重走登录,绝不出现重复领票面。
            _ => {
                let claimed = conn.execute(
                    "UPDATE approvals SET state='expired' WHERE id=?1 AND state='approved'",
                    [session_id],
                )? == 1;
                if claimed {
                    Phase::Approve { account_id, pubkey, name, platform }
                } else {
                    Phase::Terminal("expired")
                }
            }
        }
    };
    match phase {
        Phase::Pending(expires_at) => Ok(LoginStatus::Approval { expires_at }),
        Phase::Terminal("denied") => Ok(LoginStatus::Denied),
        Phase::Terminal(_) => Ok(LoginStatus::Expired),
        Phase::Approve { account_id, pubkey, name, platform } => {
            // 候选面在 verify 时已验过形(pubkey decompress + device_id 派生),
            // 这里复核形并走同一 enroll 收口
            let pk_bytes = hex::decode(&pubkey)?;
            let pk = VerifyingKey::from_bytes(
                &pk_bytes
                    .try_into()
                    .map_err(|_| anyhow!("pubkey must be 32 bytes"))?,
            )?;
            let device_id = hex::encode(Sha256::digest(pk.as_bytes()))[..32].to_string();
            let ok = enroll_and_mint(state, &account_id, &device_id, &pubkey, &name, &platform)?;
            Ok(LoginStatus::Ok(ok))
        }
    }
}

/// pending 且已过 TTL(TTL 判定唯一口径:expires_at <= now 即过期)。
fn expired_pending(state: &str, expires_at: i64) -> bool {
    state == "pending" && expires_at <= now()
}

/// effective_state:pending 过 TTL 视作 expired(不回写;回写由调用方
/// mark_expired 完成),其余原样。
fn effective_state(state: &str, expires_at: i64) -> String {
    if expired_pending(state, expires_at) {
        "expired".into()
    } else {
        state.to_string()
    }
}

/// 惰性落 TTL:pending 过期的行写回 state='expired'(一次写,后续读走终态)。
fn mark_expired(conn: &Connection, session_id: &str) -> rusqlite::Result<()> {
    conn.execute(
        "UPDATE approvals SET state='expired' WHERE id=?1 AND state='pending'",
        [session_id],
    )?;
    Ok(())
}

/// 读会话当前裁定态(pending 惰性落 TTL 后返回)。None=行不存在。
fn session_state(conn: &Connection, session_id: &str) -> rusqlite::Result<Option<(String, i64)>> {
    let row: Option<(String, i64)> = conn
        .query_row(
            "SELECT state, expires_at FROM approvals WHERE id=?1",
            [session_id],
            |r| Ok((r.get(0)?, r.get(1)?)),
        )
        .optional()?;
    Ok(row.map(|(st, exp)| {
        if expired_pending(&st, exp) {
            let _ = mark_expired(conn, session_id);
            ("expired".into(), exp)
        } else {
            (st, exp)
        }
    }))
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
    // 认证收口:设备在而账号已删(如绕过 user_remove 的手工删号)时拒绝续签,
    // 防已删账号借长期 refresh token 继续换有效凭据(api 层把 anyhow 统一映射 401)
    let known: bool = conn
        .query_row("SELECT 1 FROM accounts WHERE id=?1", [&account_id], |r| {
            r.get::<_, i64>(0)
        })
        .is_ok();
    if !known {
        return Err(anyhow!("unknown account"));
    }
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

/// 测试钩子:把所有审批会话置为过期(`_expire_all_codes_for_test` 同款先例:
/// 直接置 expires_at=0,自然 TTL 判定路径被真实走到)。
pub fn _expire_all_approvals_for_test(state: &AppState) {
    state
        .db
        .lock()
        .unwrap()
        .execute("UPDATE approvals SET expires_at = 0", [])
        .unwrap();
}
