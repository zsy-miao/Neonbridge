//! ffmpeg encoders fed with the game's frames: H.264 in fragmented MP4 (one
//! fragment per frame, keyframe every 10 s, no B-frames) and MJPEG. An encoder
//! runs while it has viewers and stops 5 s after the last one leaves.
use std::collections::{HashMap, VecDeque};
use std::io::{Read, Write};
use std::path::PathBuf;
use std::process::{Child, Command, Stdio};
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::{Arc, Condvar, Mutex, OnceLock};
use std::time::{Duration, Instant};

#[derive(Clone, Copy, PartialEq, Eq, Hash, Debug)]
pub(crate) enum Kind {
    Mp4,
    Jpeg,
}

struct Config {
    fps: u32,
    kbps: u32,
}

static CONFIG: OnceLock<Config> = OnceLock::new();
/// NVENC failed once (no NVIDIA GPU or driver): x264 from then on.
static NVENC_BAD: AtomicBool = AtomicBool::new(false);

pub(crate) fn configure(s: &crate::Settings) {
    let _ = CONFIG.set(Config { fps: s.fps, kbps: s.kbps });
}

fn config() -> &'static Config {
    CONFIG.get_or_init(|| Config { fps: 30, kbps: 4000 })
}

/// ffmpeg on PATH, else the newest WinGet Gyan.FFmpeg build; NEONBRIDGE_FFMPEG overrides.
pub(crate) fn ffmpeg() -> Option<PathBuf> {
    if let Some(p) = std::env::var_os("NEONBRIDGE_FFMPEG").map(PathBuf::from).filter(|p| p.is_file()) {
        return Some(p);
    }
    if let Some(path) = std::env::var_os("PATH") {
        for d in std::env::split_paths(&path) {
            let f = d.join("ffmpeg.exe");
            if f.is_file() {
                return Some(f);
            }
        }
    }
    let packages = PathBuf::from(std::env::var_os("LOCALAPPDATA")?).join("Microsoft/WinGet/Packages");
    let mut hits = Vec::new();
    for a in std::fs::read_dir(packages).ok()?.flatten() {
        if !a.file_name().to_string_lossy().starts_with("Gyan.FFmpeg") {
            continue;
        }
        for b in std::fs::read_dir(a.path()).into_iter().flatten().flatten() {
            let f = b.path().join("bin/ffmpeg.exe");
            if f.is_file() {
                hits.push(f);
            }
        }
    }
    hits.sort();
    hits.pop()
}

pub(crate) struct Sub {
    pub queue: VecDeque<Arc<Vec<u8>>>,
    wait_key: bool,
}

#[derive(Default)]
struct State {
    init: Option<Arc<Vec<u8>>>,
    codec: Option<String>,
    /// Fragments since the last keyframe: a new viewer starts from there.
    gop: Vec<Arc<Vec<u8>>>,
    subs: Vec<(u64, Sub)>,
    idle_since: Option<Instant>,
}

pub(crate) struct Encoder {
    kind: Kind,
    dead: AtomicBool,
    state: Mutex<State>,
    cv: Condvar,
}

fn encoders() -> &'static Mutex<HashMap<Kind, Arc<Encoder>>> {
    static E: OnceLock<Mutex<HashMap<Kind, Arc<Encoder>>>> = OnceLock::new();
    E.get_or_init(Default::default)
}

fn next_id() -> u64 {
    static N: std::sync::atomic::AtomicU64 = std::sync::atomic::AtomicU64::new(1);
    N.fetch_add(1, Ordering::Relaxed)
}

pub(crate) fn viewers() -> usize {
    encoders().lock().unwrap().values().filter(|e| !e.dead.load(Ordering::Relaxed)).map(|e| e.state.lock().unwrap().subs.len()).sum()
}

impl Encoder {
    pub(crate) fn get(kind: Kind) -> Arc<Encoder> {
        let mut all = encoders().lock().unwrap();
        if let Some(e) = all.get(&kind).filter(|e| !e.dead.load(Ordering::SeqCst)) {
            return e.clone();
        }
        let e = Arc::new(Encoder { kind, dead: AtomicBool::new(false), state: Mutex::default(), cv: Condvar::new() });
        all.insert(kind, e.clone());
        let run = e.clone();
        std::thread::Builder::new().name("feed encoder".into()).spawn(move || run.run()).ok();
        e
    }

    fn args(&self, w: usize, h: usize, fps: u32) -> Vec<String> {
        let c = config();
        let mut a: Vec<String> = ["-hide_banner", "-loglevel", "error", "-f", "rawvideo", "-pix_fmt", "bgr0", "-video_size"]
            .map(String::from).to_vec();
        a.push(format!("{w}x{h}"));
        a.extend(["-framerate".into(), fps.to_string(), "-i".into(), "-".into()]);
        let gop = (fps * 10).to_string();
        let (rate, buf) = (format!("{}k", c.kbps), format!("{}k", c.kbps * 2));
        let tail: Vec<&str> = match self.kind {
            Kind::Jpeg => vec!["-vf", "format=yuvj420p", "-c:v", "mjpeg", "-q:v", "6", "-f", "mjpeg", "-"],
            Kind::Mp4 if !NVENC_BAD.load(Ordering::Relaxed) => vec![
                "-vf", "format=yuv420p", "-c:v", "h264_nvenc", "-preset", "p1", "-tune", "ll", "-zerolatency", "1", "-delay", "0",
                "-rc", "vbr", "-cq", "26", "-b:v", "0", "-maxrate", &rate, "-bufsize", &buf, "-g", &gop, "-no-scenecut", "1",
                "-bf", "0", "-profile:v", "main", "-f", "mp4", "-movflags", "empty_moov+default_base_moof+frag_every_frame", "-",
            ],
            Kind::Mp4 => vec![
                "-vf", "format=yuv420p", "-c:v", "libx264", "-preset", "veryfast", "-tune", "zerolatency", "-crf", "24",
                "-maxrate", &rate, "-bufsize", &buf, "-g", &gop, "-sc_threshold", "0", "-bf", "0", "-profile:v", "main",
                "-f", "mp4", "-movflags", "empty_moov+default_base_moof+frag_every_frame", "-",
            ],
        };
        a.extend(tail.into_iter().map(String::from));
        a
    }

    fn run(self: Arc<Self>) {
        let mut child: Option<Child> = None;
        let result = (|| -> Result<(), String> {
            let exe = ffmpeg().ok_or("ffmpeg not found (PATH, WinGet Gyan.FFmpeg or NEONBRIDGE_FFMPEG)")?;
            // The game captures only while someone watches: wake it and wait for a frame.
            let until = Instant::now() + Duration::from_secs(10);
            let first = loop {
                crate::frames::heartbeat();
                if let Some(f) = crate::frames::latest(0) {
                    break f;
                }
                if Instant::now() > until {
                    return Err(format!("no frames from the game yet ({} published to the shared memory so far)", crate::frames::published()));
                }
                std::thread::sleep(Duration::from_millis(100));
            };
            let (w, h) = (first.width, first.height);
            let fps = if self.kind == Kind::Jpeg { config().fps.min(10) } else { config().fps };
            use std::os::windows::process::CommandExt;
            let mut c = Command::new(exe).args(self.args(w, h, fps)).stdin(Stdio::piped()).stdout(Stdio::piped())
                .stderr(Stdio::piped()).creation_flags(0x0800_0000).spawn().map_err(|e| e.to_string())?;
            crate::tie(&c);
            let out = c.stdout.take().ok_or("no ffmpeg stdout")?;
            let err = c.stderr.take();
            let mut stdin = c.stdin.take().ok_or("no ffmpeg stdin")?;
            child = Some(c);
            let me = self.clone();
            std::thread::spawn(move || me.read(out));
            if let Some(mut err) = err {
                std::thread::spawn(move || {
                    let mut s = String::new();
                    let _ = err.read_to_string(&mut s);
                    if !s.trim().is_empty() {
                        crate::say(format!("ffmpeg: {}", s.trim().lines().last().unwrap_or("")));
                    }
                });
            }
            let started = Instant::now();
            let mut frame = first;
            let mut next = Instant::now();
            let period = Duration::from_secs_f64(1.0 / f64::from(fps));
            // Constant rate: the newest frame is re-sent until the next one lands.
            while !self.dead.load(Ordering::SeqCst) {
                crate::frames::heartbeat();
                if let Some(f) = crate::frames::latest(frame.number) {
                    if (f.width, f.height) != (w, h) {
                        break; // resized: viewers reconnect to a new encoder
                    }
                    frame = f;
                }
                if stdin.write_all(&frame.pixels).is_err() {
                    if self.kind == Kind::Mp4 && !NVENC_BAD.load(Ordering::Relaxed)
                        && self.state.lock().unwrap().init.is_none() && started.elapsed() < Duration::from_secs(10)
                    {
                        NVENC_BAD.store(true, Ordering::Relaxed);
                        crate::say("NVENC is unavailable here; encoding with x264 instead");
                    }
                    break;
                }
                {
                    let mut st = self.state.lock().unwrap();
                    if !st.subs.is_empty() {
                        st.idle_since = None;
                    } else if st.idle_since.get_or_insert_with(Instant::now).elapsed() > Duration::from_secs(5) {
                        break;
                    }
                }
                next += period;
                let now = Instant::now();
                if next > now {
                    std::thread::sleep(next - now);
                } else {
                    next = now;
                }
            }
            Ok(())
        })();
        if let Err(e) = result {
            crate::say(format!("encoder stopped: {e}"));
        }
        if let Some(mut c) = child {
            let _ = c.kill();
            let _ = c.wait();
        }
        self.dead.store(true, Ordering::SeqCst);
        let _guard = self.state.lock().unwrap();
        self.cv.notify_all();
    }

    fn read(self: Arc<Self>, mut out: std::process::ChildStdout) {
        match self.kind {
            Kind::Mp4 => self.read_mp4(&mut out),
            Kind::Jpeg => self.read_jpeg(&mut out),
        }
        self.dead.store(true, Ordering::SeqCst);
        let _guard = self.state.lock().unwrap();
        self.cv.notify_all();
    }

    fn read_mp4(&self, out: &mut std::process::ChildStdout) {
        let mut head = Vec::new();
        let mut moof: Option<Vec<u8>> = None;
        loop {
            let mut hdr = [0u8; 8];
            if out.read_exact(&mut hdr).is_err() {
                return;
            }
            let n = u32::from_be_bytes([hdr[0], hdr[1], hdr[2], hdr[3]]) as usize;
            if n < 8 || n > 64 << 20 {
                return;
            }
            let mut bx = hdr.to_vec();
            bx.resize(n, 0);
            if out.read_exact(&mut bx[8..]).is_err() {
                return;
            }
            match &hdr[4..8] {
                b"ftyp" => head.extend_from_slice(&bx),
                b"moov" => {
                    head.extend_from_slice(&bx);
                    let codec = bx.windows(4).position(|w| w == b"avcC").and_then(|i| bx.get(i + 5..i + 8))
                        .map(|c| format!("avc1.{:02X}{:02X}{:02X}", c[0], c[1], c[2]));
                    let mut st = self.state.lock().unwrap();
                    st.codec = Some(codec.unwrap_or_else(|| "avc1.4D401F".into()));
                    st.init = Some(Arc::new(std::mem::take(&mut head)));
                    self.cv.notify_all();
                }
                b"moof" => moof = Some(bx),
                b"mdat" => {
                    let Some(mut frag) = moof.take() else { continue };
                    // IDR = NAL type 5 in the AVCC sample.
                    let (mut key, mut j) = (false, 8usize);
                    while j + 5 <= bx.len() {
                        if bx[j + 4] & 0x1f == 5 {
                            key = true;
                            break;
                        }
                        j += 4 + u32::from_be_bytes([bx[j], bx[j + 1], bx[j + 2], bx[j + 3]]) as usize;
                    }
                    frag.extend_from_slice(&bx);
                    self.push(Arc::new(frag), key);
                }
                _ => {}
            }
        }
    }

    /// ffmpeg's mjpeg muxer writes bare JPEGs back to back; each ends at FFD9
    /// (0xFF in entropy-coded data is always stuffed).
    fn read_jpeg(&self, out: &mut std::process::ChildStdout) {
        let mut buf = Vec::new();
        let mut chunk = vec![0u8; 1 << 16];
        loop {
            let n = match out.read(&mut chunk) {
                Ok(0) | Err(_) => return,
                Ok(n) => n,
            };
            let from = buf.len().saturating_sub(1);
            buf.extend_from_slice(&chunk[..n]);
            if let Some(end) = buf[from..].windows(2).position(|w| w == [0xff, 0xd9]).map(|p| from + p + 2) {
                let jpeg: Vec<u8> = buf.drain(..end).collect();
                if jpeg.starts_with(&[0xff, 0xd8]) {
                    self.push(Arc::new(jpeg), true);
                }
            }
            if buf.len() > 32 << 20 {
                buf.clear();
            }
        }
    }

    fn push(&self, frag: Arc<Vec<u8>>, key: bool) {
        let mut st = self.state.lock().unwrap();
        if self.kind == Kind::Mp4 {
            if key {
                st.gop.clear();
            }
            st.gop.push(frag.clone());
        }
        let cap = (config().fps as usize * 30).max(30);
        for (_, s) in st.subs.iter_mut() {
            if self.kind == Kind::Jpeg {
                // Only the newest picture matters.
                s.queue.clear();
                s.queue.push_back(frag.clone());
                continue;
            }
            if s.wait_key && !key {
                continue;
            }
            s.wait_key = false;
            // Only a viewer 30 s behind skips ahead, to the next keyframe.
            if s.queue.len() > cap {
                s.queue.clear();
                s.wait_key = true;
                continue;
            }
            s.queue.push_back(frag.clone());
        }
        self.cv.notify_all();
    }

    /// Join: (id, init segment, codec) once the encoder is up, or None.
    pub(crate) fn subscribe(&self) -> Option<(u64, Option<Arc<Vec<u8>>>, Option<String>)> {
        let st = self.state.lock().unwrap();
        let wait = if self.kind == Kind::Mp4 { Duration::from_secs(12) } else { Duration::ZERO };
        let (mut st, _) = self.cv.wait_timeout_while(st, wait, |s| s.init.is_none() && !self.dead.load(Ordering::SeqCst)).unwrap();
        if self.dead.load(Ordering::SeqCst) || (self.kind == Kind::Mp4 && st.init.is_none()) {
            return None;
        }
        let id = next_id();
        let queue = st.gop.iter().cloned().collect();
        st.subs.push((id, Sub { queue, wait_key: false }));
        Some((id, st.init.clone(), st.codec.clone()))
    }

    /// The next fragments for this viewer; None when the encoder ended.
    pub(crate) fn next(&self, id: u64) -> Option<Vec<Arc<Vec<u8>>>> {
        let mut st = self.state.lock().unwrap();
        loop {
            if self.dead.load(Ordering::SeqCst) {
                return None;
            }
            let sub = &mut st.subs.iter_mut().find(|(i, _)| *i == id)?.1;
            if !sub.queue.is_empty() {
                return Some(sub.queue.drain(..).collect());
            }
            st = self.cv.wait_timeout(st, Duration::from_secs(5)).unwrap().0;
        }
    }

    pub(crate) fn unsubscribe(&self, id: u64) {
        self.state.lock().unwrap().subs.retain(|(i, _)| *i != id);
    }
}
