//! Discord Rich Presence (Flamingo): "Playing <application name>" with the title's
//! name, whether it is in its menus or its world, the frame rate and the time
//! played. It talks to the Discord client on this PC through its local pipe
//! (`\\.\pipe\discord-ipc-N`); Discord shows the presence to the user's
//! friends.
//!
//! It needs the id of a Discord application of the user's own (Discord
//! Developer Portal, New Application, named as the presence should read, with
//! the logo uploaded under Rich Presence, Art Assets, as `flamingo`):
//! `FLAMINGO_DISCORD_APP_ID=<id>`, or the id alone in
//! `%LOCALAPPDATA%\flamingo\discord_app_id.txt`. `FLAMINGO_DISCORD=0` turns it
//! off; `FLAMINGO_DISCORD_HIDE_TITLE=1` leaves the title's name out.

use std::io::{Read, Write};
use std::path::Path;
use std::time::{Duration, SystemTime, UNIX_EPOCH};

const OP_HANDSHAKE: u32 = 0;
const OP_FRAME: u32 = 1;
const OP_CLOSE: u32 = 2;

/// Discord takes five activity updates per 20 seconds: a changed presence
/// goes out at most this often (it is looked at every second).
const EVERY: Duration = Duration::from_secs(4);

/// How long to wait before connecting again when Discord is not there.
const RETRY: Duration = Duration::from_secs(3);

/// Starts the presence on a thread of its own, if an application id is set.
pub fn start(title_dir: &Path) {
    if std::env::var_os("FLAMINGO_DISCORD").is_some_and(|v| v == "0") {
        return;
    }
    let Some(app) = app_id() else {
        println!("discord        off (no FLAMINGO_DISCORD_APP_ID; see FLAMINGO.md)");
        return;
    };
    let title = if std::env::var_os("FLAMINGO_DISCORD_HIDE_TITLE").is_some_and(|v| v == "1") {
        None
    } else {
        std::fs::read_to_string(title_dir.join("MicrosoftGame.config"))
            .ok()
            .and_then(|m| display_name(&m))
    };
    println!("discord        presence for application {app}");
    let started = SystemTime::now().duration_since(UNIX_EPOCH).map_or(0, |d| d.as_secs());
    let _ = std::thread::Builder::new()
        .name("discord presence".into())
        .spawn(move || run(&app, title.as_deref(), started));
}

fn app_id() -> Option<String> {
    let id = std::env::var("FLAMINGO_DISCORD_APP_ID").ok().or_else(|| {
        let dir = std::env::var_os("LOCALAPPDATA")?;
        std::fs::read_to_string(Path::new(&dir).join("flamingo").join("discord_app_id.txt")).ok()
    })?;
    let id = id.trim();
    (!id.is_empty() && id.bytes().all(|b| b.is_ascii_digit())).then(|| id.to_owned())
}

/// The title's name in its `MicrosoftGame.config` (`DefaultDisplayName`),
/// unless it is a resource reference.
fn display_name(manifest: &str) -> Option<String> {
    const KEY: &str = "DefaultDisplayName=\"";
    let at = manifest.find(KEY)? + KEY.len();
    let name = &manifest[at..at + manifest[at..].find('"')?];
    (!name.is_empty() && !name.starts_with("ms-resource:")).then(|| name.to_owned())
}

/// Connects, sends the presence at once and again whenever what it shows
/// changes (at most every [`EVERY`]), and connects again after Discord was
/// closed or restarted. The presence goes when the process ends.
fn run(app: &str, title: Option<&str>, started: u64) {
    let mut nonce = 0u64;
    loop {
        if let Some(mut pipe) = connect(app) {
            let mut shown = String::new();
            let mut sent: Option<std::time::Instant> = None;
            loop {
                let (fps, draws) = victory_gpu::frame_rate();
                let state = state(fps, draws);
                if state != shown && sent.is_none_or(|at| at.elapsed() >= EVERY) {
                    nonce += 1;
                    let update = activity(std::process::id(), title, &state, started, nonce);
                    if send(&mut pipe, OP_FRAME, &update).and_then(|()| receive(&mut pipe)).is_err() {
                        break;
                    }
                    (shown, sent) = (state, Some(std::time::Instant::now()));
                }
                std::thread::sleep(Duration::from_secs(1));
            }
        }
        std::thread::sleep(RETRY);
    }
}

/// Where the title is (500 draws a frame or more is the world, as the frame
/// captures count it) and its frame rate.
fn state(fps: f64, draws: u32) -> String {
    match (fps > 0.0, draws >= 500) {
        (false, _) => "Starting up".to_owned(),
        (true, true) => format!("In the world, {fps:.0} fps"),
        (true, false) => format!("In the menus, {fps:.0} fps"),
    }
}

fn connect(app: &str) -> Option<std::fs::File> {
    (0..10).find_map(|n| {
        let mut pipe = std::fs::OpenOptions::new()
            .read(true)
            .write(true)
            .open(format!(r"\\.\pipe\discord-ipc-{n}"))
            .ok()?;
        send(&mut pipe, OP_HANDSHAKE, &format!(r#"{{"v":1,"client_id":"{app}"}}"#)).ok()?;
        receive(&mut pipe).ok()?;
        Some(pipe)
    })
}

fn send(pipe: &mut impl Write, op: u32, json: &str) -> std::io::Result<()> {
    pipe.write_all(&frame(op, json))
}

/// A message on the pipe: operation and length (little-endian), then JSON.
fn frame(op: u32, json: &str) -> Vec<u8> {
    let mut bytes = Vec::with_capacity(8 + json.len());
    bytes.extend_from_slice(&op.to_le_bytes());
    bytes.extend_from_slice(&(json.len() as u32).to_le_bytes());
    bytes.extend_from_slice(json.as_bytes());
    bytes
}

/// Reads one reply. A close (an application id Discord does not know) or
/// the pipe closing is an error.
fn receive(pipe: &mut impl Read) -> std::io::Result<()> {
    let mut head = [0u8; 8];
    pipe.read_exact(&mut head)?;
    let op = u32::from_le_bytes([head[0], head[1], head[2], head[3]]);
    let len = u32::from_le_bytes([head[4], head[5], head[6], head[7]]) as usize;
    if len > 1 << 20 {
        return Err(std::io::Error::other("discord reply too long"));
    }
    let mut body = vec![0u8; len];
    pipe.read_exact(&mut body)?;
    if op == OP_CLOSE {
        return Err(std::io::Error::other(String::from_utf8_lossy(&body).into_owned()));
    }
    Ok(())
}

/// The `SET_ACTIVITY` command: the title's name, `state` ([`state`]), the
/// time since the start, and the logo.
fn activity(pid: u32, title: Option<&str>, state: &str, started: u64, nonce: u64) -> String {
    let details = title.map(|t| format!(r#""details":"{}","#, escape(t))).unwrap_or_default();
    format!(
        concat!(
            r#"{{"cmd":"SET_ACTIVITY","args":{{"pid":{},"activity":{{{}"state":"{}","#,
            r#""timestamps":{{"start":{}}},"assets":{{"large_image":"flamingo","large_text":"Flamingo"}}}}}},"nonce":"{}"}}"#
        ),
        pid,
        details,
        escape(state),
        started,
        nonce
    )
}

/// A JSON string's contents.
fn escape(text: &str) -> String {
    let mut out = String::with_capacity(text.len());
    for c in text.chars() {
        match c {
            '"' => out.push_str("\\\""),
            '\\' => out.push_str("\\\\"),
            c if u32::from(c) < 0x20 => out.push_str(&format!("\\u{:04x}", u32::from(c))),
            c => out.push(c),
        }
    }
    out
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn the_title_name_comes_from_the_manifest() {
        let manifest = r#"<ShellVisuals DefaultDisplayName="Example Title" PublisherDisplayName="x"/>"#;
        assert_eq!(display_name(manifest).as_deref(), Some("Example Title"));
        assert_eq!(display_name(r#"DefaultDisplayName="ms-resource:Title""#), None);
        assert_eq!(display_name("<Game/>"), None);
    }

    /// A frame is the operation and length, little-endian, then the JSON;
    /// the activity is one JSON object with the strings escaped.
    #[test]
    fn frames_and_activity_are_well_formed() {
        let json = activity(42, Some(r#"A "quoted" \ name"#), &state(7.4, 4980), 1_700_000_000, 3);
        let bytes = frame(OP_FRAME, &json);
        assert_eq!(&bytes[..4], &1u32.to_le_bytes());
        assert_eq!(u32::from_le_bytes([bytes[4], bytes[5], bytes[6], bytes[7]]) as usize, json.len());
        assert!(json.contains(r#""details":"A \"quoted\" \\ name","#), "{json}");
        assert!(json.contains(r#""state":"In the world, 7 fps""#), "{json}");
        assert!(json.contains(r#""timestamps":{"start":1700000000}"#), "{json}");
        assert!(json.ends_with(r#""nonce":"3"}"#), "{json}");
        let opens = json.matches('{').count();
        assert_eq!(opens, json.matches('}').count(), "{json}");
        // no title, before the first frame
        let json = activity(42, None, &state(0.0, 0), 5, 1);
        assert_eq!(state(29.6, 12), "In the menus, 30 fps");
        assert!(!json.contains("details") && json.contains(r#""state":"Starting up""#), "{json}");
    }

    /// A close frame (an unknown application id) is an error; a reply is read whole.
    #[test]
    fn a_close_reply_is_an_error() {
        let mut close = std::io::Cursor::new(frame(OP_CLOSE, r#"{"code":4000,"message":"Invalid Client ID"}"#));
        assert!(receive(&mut close).is_err());
        let mut ready = std::io::Cursor::new(frame(OP_FRAME, r#"{"evt":"READY"}"#));
        assert!(receive(&mut ready).is_ok());
        assert_eq!(ready.position() as usize, ready.get_ref().len());
    }
}
