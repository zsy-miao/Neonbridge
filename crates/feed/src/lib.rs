//! The game feed: the running game's picture, shared over Tailscale.
//!
//! Started by the launcher with NEONBRIDGE_FEED=1. The game's D3D12 provider
//! (TUXBOX_FEED=1, present/feed.rs) copies presented frames into shared memory
//! while someone is watching; this crate serves them on 127.0.0.1:<port> as
//! H.264 in fragmented MP4 (ffmpeg, NVENC when it works, else x264) played in a
//! <video> (iPhone full screen and picture in picture work there), with MJPEG
//! as the fallback. Every route sits behind a site password.
//!
//! The link out is tools/tsproxy.exe, the RuneLink Tailscale proxy (Go tsnet,
//! kept outside the repo like RuneLink does): it joins the tailnet in-process
//! and serves http(s)://<name>.<tailnet>.ts.net. On its first start it prints a
//! sign-in link that the user opens once to approve this PC. If tsproxy is
//! missing or fails, tools/cloudflared.exe gives a quick tunnel instead.
//!
//! Design after RuneLink's Watch (osrs-rl-link m5_watch.rs and vorkath_dash.rs).

mod auth;
mod encoder;
mod frames;
mod http;
mod page;
mod tunnel;

use std::path::PathBuf;

/// The shared-memory name the game's provider writes frames to.
pub const FRAMES_NAME: &str = r"Local\NeonbridgeFeed";

#[derive(Clone, Debug)]
pub struct Settings {
    /// NEONBRIDGE_FEED_PORT (8788; RuneLink's dashboard has 8787).
    pub port: u16,
    /// NEONBRIDGE_FEED_LINK: tailscale (default), cloudflared or none.
    pub link: String,
    /// NEONBRIDGE_FEED_HOST: the Tailscale node name (neonbridge).
    pub host: String,
    /// NEONBRIDGE_FEED_FPS (30, 1-60).
    pub fps: u32,
    /// NEONBRIDGE_FEED_WIDTH (1280, 160-1920): frames are scaled down to at most this.
    pub width: u32,
    /// NEONBRIDGE_FEED_KBPS (4000): the H.264 rate cap.
    pub kbps: u32,
}

impl Settings {
    pub fn from_env() -> Self {
        let var = |k: &str| std::env::var(k).ok().map(|v| v.trim().to_owned()).filter(|v| !v.is_empty());
        let num = |k: &str, d: u32, lo: u32, hi: u32| var(k).and_then(|v| v.parse().ok()).unwrap_or(d).clamp(lo, hi);
        let host = var("NEONBRIDGE_FEED_HOST")
            .filter(|v| v.len() <= 40 && v.bytes().all(|b| b.is_ascii_lowercase() || b.is_ascii_digit() || b == b'-'))
            .unwrap_or_else(|| "neonbridge".into());
        Self {
            port: num("NEONBRIDGE_FEED_PORT", 8788, 1024, 65535) as u16,
            link: var("NEONBRIDGE_FEED_LINK").unwrap_or_else(|| "tailscale".into()).to_ascii_lowercase(),
            host,
            fps: num("NEONBRIDGE_FEED_FPS", 30, 1, 60),
            width: num("NEONBRIDGE_FEED_WIDTH", 1280, 160, 1920),
            kbps: num("NEONBRIDGE_FEED_KBPS", 4000, 300, 50_000),
        }
    }
}

pub fn enabled() -> bool {
    std::env::var("NEONBRIDGE_FEED").is_ok_and(|v| v.trim() == "1")
}

/// The environment the game needs for the feed tap.
pub fn game_env() -> Vec<(String, String)> {
    vec![("TUXBOX_FEED".into(), "1".into()), ("TUXBOX_FEED_NAME".into(), FRAMES_NAME.into())]
}

/// Where the password and the Tailscale login live: outside the repo, under
/// %LOCALAPPDATA%\6ixpack\feed (never committed).
pub(crate) fn state_dir() -> PathBuf {
    let base = std::env::var_os("LOCALAPPDATA").map(PathBuf::from).unwrap_or_else(std::env::temp_dir);
    base.join("6ixpack").join("feed")
}

pub(crate) fn say(line: impl AsRef<str>) {
    println!("[feed] {}", line.as_ref());
}

/// Start the server and the link once per launcher; later calls do nothing.
pub fn start(settings: Settings) {
    static STARTED: std::sync::Once = std::sync::Once::new();
    STARTED.call_once(|| {
        let _ = std::fs::create_dir_all(state_dir());
        frames::configure(settings.fps, settings.width);
        encoder::configure(&settings);
        match http::serve(settings.port) {
            Ok(()) => {
                say(format!("serving http://127.0.0.1:{} (set the password there first, on this PC)", settings.port));
                tunnel::start(&settings);
            }
            Err(e) => say(format!("could not listen on 127.0.0.1:{} ({e}); no feed this run", settings.port)),
        }
    });
}

pub(crate) fn now_unix() -> u64 {
    std::time::SystemTime::now().duration_since(std::time::UNIX_EPOCH).map(|d| d.as_secs()).unwrap_or(0)
}

pub(crate) fn random(n: usize) -> Vec<u8> {
    let mut b = vec![0u8; n];
    // SAFETY: the buffer is n bytes long; the system RNG fills it.
    let status = unsafe {
        windows_sys::Win32::Security::Cryptography::BCryptGenRandom(
            std::ptr::null_mut(), b.as_mut_ptr(), n as u32,
            windows_sys::Win32::Security::Cryptography::BCRYPT_USE_SYSTEM_PREFERRED_RNG)
    };
    assert!(status >= 0, "BCryptGenRandom failed");
    b
}

pub(crate) fn hex(b: &[u8]) -> String {
    b.iter().map(|x| format!("{x:02x}")).collect()
}

/// The feed's helpers (ffmpeg, tsproxy, cloudflared) end with the launcher:
/// one kill-on-close job, its handle never closed.
pub(crate) fn tie(child: &std::process::Child) {
    use std::os::windows::io::AsRawHandle;
    use windows_sys::Win32::System::JobObjects::*;
    struct Job(windows_sys::Win32::Foundation::HANDLE);
    // SAFETY: a job handle is usable from any thread.
    unsafe impl Send for Job {}
    unsafe impl Sync for Job {}
    static JOB: std::sync::OnceLock<Option<Job>> = std::sync::OnceLock::new();
    let job = JOB.get_or_init(|| unsafe {
        let job = CreateJobObjectW(std::ptr::null(), std::ptr::null());
        if job.is_null() {
            return None;
        }
        let mut info: JOBOBJECT_EXTENDED_LIMIT_INFORMATION = std::mem::zeroed();
        info.BasicLimitInformation.LimitFlags = JOB_OBJECT_LIMIT_KILL_ON_JOB_CLOSE;
        (SetInformationJobObject(job, JobObjectExtendedLimitInformation, (&info as *const JOBOBJECT_EXTENDED_LIMIT_INFORMATION).cast(),
            size_of::<JOBOBJECT_EXTENDED_LIMIT_INFORMATION>() as u32) != 0).then_some(Job(job))
    });
    if let Some(job) = job {
        unsafe { AssignProcessToJobObject(job.0, child.as_raw_handle() as _) };
    }
}
