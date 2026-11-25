//! (Neptune) Discord Rich Presence while a title runs: "Playing Grand Theft Auto VI" with
//! the time played, through the Discord client's own pipe on this PC (`\\.\pipe\discord-ipc-N`).
//! Nothing goes over the network from here, and nothing happens if Discord is not running.
//! Off unless asked for (AnyXboxX): `VICTORY_DISCORD=1` turns it on; `VICTORY_DISCORD_APP`
//! names another application id.
//! The picture is the application's Rich Presence art asset named `logo`.

use std::io::{Read, Write};
use std::time::{Duration, SystemTime, UNIX_EPOCH};

/// The application the presence is shown under.
const APPLICATION: &str = "1575009406737846333";

/// One frame of Discord's local protocol: the operation, the length, the JSON.
fn frame(op: u32, json: &str) -> Vec<u8> {
    let mut bytes = Vec::with_capacity(8 + json.len());
    bytes.extend_from_slice(&op.to_le_bytes());
    bytes.extend_from_slice(&(json.len() as u32).to_le_bytes());
    bytes.extend_from_slice(json.as_bytes());
    bytes
}

/// Reads one frame's answer and drops it (the client answers every frame).
fn answer(pipe: &mut std::fs::File) -> std::io::Result<()> {
    let mut head = [0u8; 8];
    pipe.read_exact(&mut head)?;
    let len = u32::from_le_bytes([head[4], head[5], head[6], head[7]]) as usize;
    let mut body = vec![0u8; len.min(1 << 16)];
    pipe.read_exact(&mut body)
}

/// Connects to the first Discord pipe there is and sets the activity; the pipe is returned
/// open, since the activity lasts only as long as the connection.
fn connect(application: &str, started: u64) -> Option<std::fs::File> {
    for n in 0..10 {
        let path = format!(r"\\.\pipe\discord-ipc-{n}");
        let Ok(mut pipe) = std::fs::OpenOptions::new().read(true).write(true).open(&path) else {
            continue;
        };
        let hello = format!(r#"{{"v":1,"client_id":"{application}"}}"#);
        if pipe.write_all(&frame(0, &hello)).is_err() || answer(&mut pipe).is_err() {
            continue;
        }
        let activity = format!(
            r#"{{"cmd":"SET_ACTIVITY","args":{{"pid":{},"activity":{{"details":"Neptune (VIctory Edit.)","state":"In game","timestamps":{{"start":{started}}},"assets":{{"large_image":"logo","large_text":"Grand Theft Auto VI"}}}}}},"nonce":"neptune-{started}"}}"#,
            std::process::id()
        );
        if pipe.write_all(&frame(1, &activity)).is_ok() && answer(&mut pipe).is_ok() {
            return Some(pipe);
        }
    }
    None
}

/// Whether `VICTORY_DISCORD` asks for the presence: only the value `1` does.
fn wanted(value: Option<&std::ffi::OsStr>) -> bool {
    value.is_some_and(|v| v == "1")
}

/// Starts the presence on a thread of its own, for as long as the process lives.
pub fn start() {
    if !wanted(std::env::var_os("VICTORY_DISCORD").as_deref()) {
        return;
    }
    let application = std::env::var("VICTORY_DISCORD_APP").unwrap_or_else(|_| APPLICATION.to_owned());
    let started = SystemTime::now().duration_since(UNIX_EPOCH).map_or(0, |d| d.as_secs());
    let _ = std::thread::Builder::new().name("victory-discord".into()).spawn(move || {
        loop {
            // Held while it answers; looked for again if Discord was closed or is started later.
            if let Some(mut pipe) = connect(&application, started) {
                eprintln!("Neptune: Discord Rich Presence set");
                let mut byte = [0u8; 1];
                while pipe.read(&mut byte).is_ok_and(|n| n > 0) {}
            }
            std::thread::sleep(Duration::from_secs(20));
        }
    });
}

#[cfg(test)]
mod tests {
    use super::wanted;
    use std::ffi::OsStr;

    #[test]
    fn presence_is_off_unless_asked_for() {
        assert!(!wanted(None));
        assert!(!wanted(Some(OsStr::new("0"))));
        assert!(!wanted(Some(OsStr::new(""))));
        assert!(!wanted(Some(OsStr::new("yes"))));
        assert!(wanted(Some(OsStr::new("1"))));
    }
}
