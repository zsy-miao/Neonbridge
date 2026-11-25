//! The site password: every route sits behind it. It is set once from this PC
//! itself (a stranger who found the link before a password existed cannot
//! claim the site); a login gets a 30-day session cookie. Only a PBKDF2 hash
//! of the password and SHA-256 hashes of session tokens are stored, in
//! %LOCALAPPDATA%\6ixpack\feed\auth.txt. 10 wrong passwords in 15 minutes lock
//! logins for the rest of that window.
use sha2::{Digest, Sha256};
use std::sync::Mutex;
use std::time::{Duration, Instant};

const SESSION_SECS: u64 = 30 * 86400;
const ROUNDS: u32 = 200_000;
pub(crate) const MIN_LEN: usize = 10;

#[derive(Default)]
struct Store {
    salt: String,
    hash: String,
    /// (sha256 of the token, expiry unix seconds)
    sessions: Vec<(String, u64)>,
}

static LOCK: Mutex<()> = Mutex::new(());
static FAILS: Mutex<Vec<Instant>> = Mutex::new(Vec::new());

fn path() -> std::path::PathBuf {
    crate::state_dir().join("auth.txt")
}

fn load() -> Option<Store> {
    let text = std::fs::read_to_string(path()).ok()?;
    let mut s = Store::default();
    for line in text.lines() {
        let mut it = line.split_whitespace();
        match (it.next(), it.next(), it.next()) {
            (Some("salt"), Some(v), _) => s.salt = v.into(),
            (Some("hash"), Some(v), _) => s.hash = v.into(),
            (Some("session"), Some(h), Some(e)) => s.sessions.push((h.into(), e.parse().unwrap_or(0))),
            _ => {}
        }
    }
    (!s.salt.is_empty() && !s.hash.is_empty()).then_some(s)
}

fn save(s: &Store) -> std::io::Result<()> {
    let mut text = format!("salt {}\nhash {}\n", s.salt, s.hash);
    for (h, e) in &s.sessions {
        text.push_str(&format!("session {h} {e}\n"));
    }
    let p = path();
    if let Some(d) = p.parent() {
        std::fs::create_dir_all(d)?;
    }
    let tmp = p.with_extension("tmp");
    std::fs::write(&tmp, text)?;
    std::fs::rename(tmp, p)
}

pub(crate) fn password_set() -> bool {
    load().is_some()
}

fn hmac(key: &[u8], parts: &[&[u8]]) -> [u8; 32] {
    let mut k = [0u8; 64];
    if key.len() > 64 {
        k[..32].copy_from_slice(&Sha256::digest(key));
    } else {
        k[..key.len()].copy_from_slice(key);
    }
    let mut inner = Sha256::new();
    inner.update(k.map(|b| b ^ 0x36));
    for p in parts {
        inner.update(p);
    }
    let mut outer = Sha256::new();
    outer.update(k.map(|b| b ^ 0x5c));
    outer.update(inner.finalize());
    outer.finalize().into()
}

/// PBKDF2-HMAC-SHA256, one block (32 bytes).
fn pbkdf2(password: &[u8], salt: &[u8]) -> [u8; 32] {
    let mut u = hmac(password, &[salt, &1u32.to_be_bytes()]);
    let mut out = u;
    for _ in 1..ROUNDS {
        u = hmac(password, &[&u]);
        for (o, x) in out.iter_mut().zip(u) {
            *o ^= x;
        }
    }
    out
}

fn unhex(s: &str) -> Option<Vec<u8>> {
    (s.len() % 2 == 0).then(|| (0..s.len()).step_by(2).map(|i| u8::from_str_radix(&s[i..i + 2], 16).ok()).collect())?
}

fn same(a: &[u8], b: &[u8]) -> bool {
    a.len() == b.len() && a.iter().zip(b).fold(0u8, |d, (x, y)| d | (x ^ y)) == 0
}

fn token_hash(token: &str) -> String {
    crate::hex(&Sha256::digest(token.as_bytes()))
}

pub(crate) enum Login {
    Ok(String),
    Refused(u16, &'static str),
}

/// Check (or, the first time and only from this PC, set) the password; a new
/// session token on success.
pub(crate) fn login(password: &str, local: bool) -> Login {
    let _guard = LOCK.lock().unwrap_or_else(|e| e.into_inner());
    {
        let mut fails = FAILS.lock().unwrap_or_else(|e| e.into_inner());
        fails.retain(|t| t.elapsed() < Duration::from_secs(900));
        if fails.len() >= 10 {
            return Login::Refused(429, "Too many tries. Wait 15 minutes.");
        }
    }
    let mut store = match load() {
        None => {
            if !local {
                return Login::Refused(403, "Set the password on the PC itself first (http://127.0.0.1 on the feed's port).");
            }
            if password.chars().count() < MIN_LEN {
                return Login::Refused(400, "Use at least 10 characters.");
            }
            let salt = crate::random(16);
            Store { salt: crate::hex(&salt), hash: crate::hex(&pbkdf2(password.as_bytes(), &salt)), sessions: Vec::new() }
        }
        Some(store) => {
            let Some(salt) = unhex(&store.salt) else { return Login::Refused(500, "The password file is damaged.") };
            let want = unhex(&store.hash).unwrap_or_default();
            if !same(&pbkdf2(password.as_bytes(), &salt), &want) {
                FAILS.lock().unwrap_or_else(|e| e.into_inner()).push(Instant::now());
                return Login::Refused(401, "Wrong password.");
            }
            store
        }
    };
    let now = crate::now_unix();
    let token = crate::hex(&crate::random(32));
    store.sessions.retain(|(_, e)| *e > now);
    store.sessions.push((token_hash(&token), now + SESSION_SECS));
    match save(&store) {
        Ok(()) => Login::Ok(token),
        Err(_) => Login::Refused(500, "Could not save the session."),
    }
}

/// Whether the request's cookie holds a live session.
pub(crate) fn authed(cookie: Option<&str>) -> bool {
    let Some(token) = cookie.unwrap_or("").split(';').filter_map(|p| p.trim().strip_prefix("nb_feed=")).find(|v| !v.is_empty()) else {
        return false;
    };
    let Some(store) = load() else { return false };
    let (h, now) = (token_hash(token), crate::now_unix());
    store.sessions.iter().any(|(s, e)| same(s.as_bytes(), h.as_bytes()) && *e > now)
}

pub(crate) fn cookie(token: &str, secure: bool) -> String {
    format!("nb_feed={token}; Max-Age={SESSION_SECS}; Path=/; HttpOnly;{} SameSite=Lax", if secure { " Secure;" } else { "" })
}
