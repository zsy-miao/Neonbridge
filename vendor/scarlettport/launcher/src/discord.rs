//! Discord Rich Presence over Discord's local IPC pipe (`\\.\pipe\discord-ipc-0` to `-9`): a
//! handshake with the project's own application ID, then SET_ACTIVITY. No SDK DLL, no token, no
//! network. With no application ID compiled in, or Discord not running, it does nothing.
use serde_json::json;
use std::io::{Read, Write};
use std::sync::mpsc::{channel, Receiver, RecvTimeoutError, Sender};
use std::time::Duration;

/// The "ScarlettPort" application on Discord's developer portal (owned by the project). Empty
/// until it is created; presence is off while it is empty.
pub const DISCORD_APP_ID: &str = "";

/// What to show.
#[derive(Clone, Debug, PartialEq)]
pub struct Activity {
    pub details: String,
    pub state: String,
    /// Unix seconds the current activity started (shown as elapsed time).
    pub start: u64,
    /// Small image key: "nvidia", "amd" or "intel" (uploaded as art assets of the app).
    pub small: Option<&'static str>,
}

/// Owns the presence thread; `set(None)` clears it.
pub struct Presence {
    tx: Option<Sender<Option<Activity>>>,
}

impl Presence {
    pub fn new() -> Presence {
        if DISCORD_APP_ID.is_empty() {
            return Presence { tx: None };
        }
        let (tx, rx) = channel();
        std::thread::Builder::new().name("discord-presence".into()).spawn(move || run(rx)).ok();
        Presence { tx: Some(tx) }
    }

    pub fn set(&self, a: Option<Activity>) {
        if let Some(tx) = &self.tx {
            let _ = tx.send(a);
        }
    }
}

/// GPU vendor image key from an adapter name.
pub fn vendor_key(gpu: &str) -> Option<&'static str> {
    let g = gpu.to_ascii_lowercase();
    if g.contains("nvidia") || g.contains("geforce") || g.contains("rtx") {
        Some("nvidia")
    } else if g.contains("amd") || g.contains("radeon") {
        Some("amd")
    } else if g.contains("intel") || g.contains("arc ") {
        Some("intel")
    } else {
        None
    }
}

/// One IPC frame: opcode, length (both little-endian u32), JSON.
pub fn frame(op: u32, body: &serde_json::Value) -> Vec<u8> {
    let b = body.to_string().into_bytes();
    let mut out = Vec::with_capacity(8 + b.len());
    out.extend_from_slice(&op.to_le_bytes());
    out.extend_from_slice(&(b.len() as u32).to_le_bytes());
    out.extend_from_slice(&b);
    out
}

pub fn activity_json(a: &Activity) -> serde_json::Value {
    let mut assets = json!({ "large_image": "logo", "large_text": "ScarlettPort" });
    if let Some(s) = a.small {
        assets["small_image"] = json!(s);
        assets["small_text"] = json!(s.to_uppercase());
    }
    json!({
        "details": a.details,
        "state": a.state,
        "timestamps": { "start": a.start },
        "assets": assets,
    })
}

fn connect() -> Option<std::fs::File> {
    (0..10).find_map(|i| std::fs::OpenOptions::new().read(true).write(true).open(format!(r"\\.\pipe\discord-ipc-{i}")).ok())
}

fn read_frame(f: &mut std::fs::File) -> Option<()> {
    let mut head = [0u8; 8];
    f.read_exact(&mut head).ok()?;
    let len = u32::from_le_bytes(head[4..8].try_into().ok()?) as usize;
    let mut body = vec![0u8; len.min(1 << 20)];
    f.read_exact(&mut body).ok()
}

fn send(f: &mut std::fs::File, a: &Option<Activity>, nonce: &mut u64) -> Option<()> {
    *nonce += 1;
    let args = match a {
        Some(a) => json!({ "pid": std::process::id(), "activity": activity_json(a) }),
        None => json!({ "pid": std::process::id() }),
    };
    f.write_all(&frame(1, &json!({ "cmd": "SET_ACTIVITY", "args": args, "nonce": nonce.to_string() }))).ok()?;
    read_frame(f)
}

fn run(rx: Receiver<Option<Activity>>) {
    let mut want: Option<Activity> = None;
    let mut pipe: Option<std::fs::File> = None;
    let mut dirty = false;
    let mut nonce = 0u64;
    loop {
        match rx.recv_timeout(Duration::from_secs(15)) {
            Ok(a) => {
                dirty |= a != want;
                want = a;
            }
            Err(RecvTimeoutError::Timeout) => {}
            Err(RecvTimeoutError::Disconnected) => return,
        }
        if pipe.is_none() && want.is_some() {
            // Discord may start later: try again on the next tick.
            pipe = connect().and_then(|mut f| {
                f.write_all(&frame(0, &json!({ "v": 1, "client_id": DISCORD_APP_ID }))).ok()?;
                read_frame(&mut f)?;
                Some(f)
            });
            dirty = pipe.is_some();
        }
        if dirty {
            if let Some(f) = pipe.as_mut() {
                if send(f, &want, &mut nonce).is_none() {
                    pipe = None;
                }
            }
            dirty = pipe.is_none() && want.is_some();
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn frames_and_activity() {
        let f = frame(0, &json!({ "v": 1 }));
        assert_eq!(&f[..4], &0u32.to_le_bytes());
        assert_eq!(u32::from_le_bytes(f[4..8].try_into().unwrap()) as usize, f.len() - 8);
        let a = Activity { details: "In the world".into(), state: "World engine".into(), start: 5, small: vendor_key("NVIDIA GeForce RTX 3060") };
        let j = activity_json(&a);
        assert_eq!(j["assets"]["large_image"], "logo");
        assert_eq!(j["assets"]["small_image"], "nvidia");
        assert_eq!(j["timestamps"]["start"], 5);
        assert_eq!(vendor_key("AMD Radeon RX 6800"), Some("amd"));
        assert_eq!(vendor_key("Microsoft Basic Render Driver"), None);
    }
}
