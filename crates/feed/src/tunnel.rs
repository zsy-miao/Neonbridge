//! The link out: tools/tsproxy.exe (RuneLink's Go tsnet proxy; Tailscale runs
//! in-process, no install) first, else a cloudflared quick tunnel.
//!
//! tsproxy's stdout lines: AUTHURL <url> (approve this PC once), URL <url>
//! (up), ERR <text> (failed). Its login lives in the state directory, so later
//! starts need nothing. TS_AUTHKEY, if set, goes to it through the environment
//! (never argv). Its stdin stays open while the launcher lives: when the
//! launcher ends, tsproxy leaves the tailnet too.
use std::io::{BufRead, BufReader, Write};
use std::path::PathBuf;
use std::process::{Command, Stdio};

/// tools/<name> next to the launcher or up to three folders above it (a
/// cargo build's target/release), else in the working directory's tools/.
/// NEONBRIDGE_TSPROXY / NEONBRIDGE_CLOUDFLARED name the file directly.
fn tool(name: &str, var: &str) -> Option<PathBuf> {
    if let Some(p) = std::env::var_os(var).map(PathBuf::from).filter(|p| p.is_file()) {
        return Some(p);
    }
    let exe = std::env::current_exe().ok();
    let mut dirs: Vec<PathBuf> = exe.iter().flat_map(|e| e.ancestors().skip(1).take(4).map(PathBuf::from)).collect();
    dirs.extend(std::env::current_dir().ok());
    dirs.into_iter().map(|d| d.join("tools").join(name)).find(|p| p.is_file())
}

pub(crate) fn start(s: &crate::Settings) {
    let (port, host, link) = (s.port, s.host.clone(), s.link.clone());
    std::thread::Builder::new().name("feed link".into()).spawn(move || match link.as_str() {
        "none" | "off" | "0" => crate::say("NEONBRIDGE_FEED_LINK=none: the feed stays on this PC"),
        "cloudflared" => cloudflared(port),
        _ => {
            if !tailscale(port, &host) {
                cloudflared(port);
            }
        }
    }).ok();
}

/// Runs tsproxy until it ends. False when it never came up (fall back).
fn tailscale(port: u16, host: &str) -> bool {
    let Some(exe) = tool("tsproxy.exe", "NEONBRIDGE_TSPROXY") else {
        crate::say("tools/tsproxy.exe not found (copy RuneLink's build there); trying cloudflared");
        return false;
    };
    // tsproxy keeps its sealed key three folders above -dir (its govault root).
    let dir = crate::state_dir().join("tailscale").join("state").join("tailscale");
    let _ = std::fs::create_dir_all(&dir);
    use std::os::windows::process::CommandExt;
    let child = Command::new(&exe).arg("-dir").arg(&dir).args(["-hostname", host, "-target", &format!("http://127.0.0.1:{port}")])
        .stdin(Stdio::piped()).stdout(Stdio::piped()).stderr(Stdio::piped()).creation_flags(0x0800_0000).spawn();
    let mut child = match child {
        Ok(c) => c,
        Err(e) => {
            crate::say(format!("tsproxy did not start ({e}); trying cloudflared"));
            return false;
        }
    };
    crate::tie(&child);
    crate::say(format!("joining Tailscale as {host}..."));
    // Held for the launcher's life: closing it ends tsproxy.
    let stdin = child.stdin.take();
    if let Some(err) = child.stderr.take() {
        let log = crate::state_dir().join("tsproxy.log");
        std::thread::spawn(move || {
            if let Ok(mut f) = std::fs::OpenOptions::new().create(true).append(true).open(log) {
                for line in BufReader::new(err).lines().map_while(Result::ok) {
                    let _ = writeln!(f, "{line}");
                }
            }
        });
    }
    let mut up = false;
    if let Some(out) = child.stdout.take() {
        for line in BufReader::new(out).lines().map_while(Result::ok) {
            if let Some(url) = line.strip_prefix("AUTHURL ") {
                crate::say(format!("Tailscale sign-in needed: open {} and approve this PC (once)", url.trim()));
            } else if let Some(url) = line.strip_prefix("URL ") {
                up = true;
                crate::say(format!("live at {} (Tailscale)", url.trim()));
                let _ = std::fs::write(crate::state_dir().join("link.txt"), url.trim());
            } else if let Some(e) = line.strip_prefix("ERR ") {
                crate::say(format!("tsproxy: {e}"));
            }
        }
    }
    let _ = child.wait();
    drop(stdin);
    crate::say(if up { "Tailscale link closed" } else { "the Tailscale link failed; trying cloudflared" });
    up
}

fn cloudflared(port: u16) {
    let Some(exe) = tool("cloudflared.exe", "NEONBRIDGE_CLOUDFLARED") else {
        crate::say("no tools/cloudflared.exe either: the feed stays on this PC");
        return;
    };
    use std::os::windows::process::CommandExt;
    let child = Command::new(exe).args(["tunnel", "--no-autoupdate", "--url", &format!("http://127.0.0.1:{port}")])
        .stdin(Stdio::null()).stdout(Stdio::null()).stderr(Stdio::piped()).creation_flags(0x0800_0000).spawn();
    let mut child = match child {
        Ok(c) => c,
        Err(e) => return crate::say(format!("cloudflared did not start ({e})")),
    };
    crate::tie(&child);
    if let Some(err) = child.stderr.take() {
        for line in BufReader::new(err).lines().map_while(Result::ok) {
            if let Some(at) = line.find("https://") {
                let url: String = line[at..].chars().take_while(|c| !c.is_whitespace() && *c != '|').collect();
                if url.ends_with(".trycloudflare.com") {
                    crate::say(format!("live at {url} (cloudflared)"));
                    let _ = std::fs::write(crate::state_dir().join("link.txt"), &url);
                }
            }
        }
    }
    let _ = child.wait();
    crate::say("cloudflared tunnel closed");
}
