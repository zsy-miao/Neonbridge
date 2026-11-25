//! ScarlettPort launcher: native Rust (eframe/egui), fixed-size frameless window.
//! No network access except opening the project links in the browser.
#![cfg_attr(not(debug_assertions), windows_subsystem = "windows")]

mod art;
mod cli;
mod display;
mod pages;
mod perf;
mod presets;
mod tuning;
mod verify;
mod engine;
mod flow;
mod game;
mod runtime;
mod discord;
mod report;
mod saves;
mod update;
mod shaderbuild;
mod theme;
mod view;

use eframe::egui;
use serde::{Deserialize, Serialize};
use std::{
    io::Read,
    path::PathBuf,
    process::{Command, Stdio},
    sync::{
        atomic::{AtomicBool, Ordering},
        mpsc::{channel, Receiver, Sender},
        Arc,
    },
    time::Instant,
};

pub const GAME_EXE: &str = "game_scarlett_master.exe";
pub const VERSION: &str = env!("CARGO_PKG_VERSION");
pub const REPO_URL: &str = "https://github.com/ZigZagBoi1/ScarlettPort";
pub const PAYPAL_URL: &str = "https://www.paypal.me/SalwanStudios";
pub const KOFI_URL: &str = "https://ko-fi.com/ziggymar";
pub const PLAYABLE_PERCENT: f32 = 50.;
const MAX_LOG: usize = 3000;

#[derive(Clone, Copy, PartialEq, Eq)]
pub enum Page {
    Play,
    News,
    Setup,
    Logs,
    Links,
    Settings,
    Saves,
    Verify,
    /// The first-run wizard: game folder, verify, shaders, preset, Play.
    Welcome,
}

pub const BUNDLED: &str = "bundled";
pub const LOCAL: &str = "local";

#[derive(Clone, Copy, PartialEq, Eq)]
pub enum RunKind {
    Play,
    Precompile,
    Pipelines,
}

#[derive(Clone, Copy, PartialEq, Eq)]
pub enum LogKind {
    Info,
    Out,
    Error,
}

#[derive(Serialize, Deserialize, Clone)]
pub struct Config {
    #[serde(default)]
    pub game_path: String,
    #[serde(default)]
    pub auto_menu: bool,
    /// Where the DXIL cache lives (`<cache>\dxil`); empty until first run picks the default.
    #[serde(default)]
    pub cache_dir: String,
    /// Runtime hash the shader cache was built for; a different runtime makes it stale.
    #[serde(default)]
    pub shaders_hash: String,
    /// Size of the pipeline journal when the world pipelines were last built.
    #[serde(default)]
    pub world_len: u64,
    /// Window mode, resolution, frame rate cap, vsync and monitor (passed to the runtime).
    #[serde(default)]
    pub display: display::DisplayCfg,
    /// "classic" (XCL) or "world" (the 3D-world engine).
    #[serde(default = "default_engine")]
    pub engine: String,
    /// World engine: Series S console profile instead of Series X.
    #[serde(default)]
    pub console_s: bool,
    #[serde(default)]
    pub raytracing: bool,
    #[serde(default = "yes")]
    pub fallback_light: bool,
    #[serde(default)]
    pub debug_view: bool,
    /// World engine bundle hash the shaders were built for.
    #[serde(default)]
    pub world_shaders_hash: String,
    /// Set once the engine default moved to World (older settings files said Classic by default).
    #[serde(default)]
    pub engine_v2: bool,
    /// Shader build: stop a step that shows no progress for `slow_secs` and leave its shaders
    /// to compile on demand in game.
    #[serde(default = "yes")]
    pub skip_slow: bool,
    #[serde(default = "default_slow_secs")]
    pub slow_secs: u64,
    /// Discord Rich Presence (needs Discord running and the project's application ID).
    #[serde(default = "yes")]
    pub discord: bool,
    /// Check GitHub Releases for a newer launcher at start.
    #[serde(default = "yes")]
    pub auto_update: bool,
    /// Play preset: "stable", "experimental" or "custom" (`presets`).
    #[serde(default = "default_preset")]
    pub preset: String,
    /// "bundled" (the runtime folder beside the launcher) or "local" (a victory.exe the user
    /// picked, e.g. their own newest build). Updates never touch a local build.
    #[serde(default = "default_runtime")]
    pub runtime_mode: String,
    #[serde(default)]
    pub local_exe: String,
    /// The local build's cache folder (VICTORY_CACHE_DIR as is); empty: `<cache>\local`.
    #[serde(default)]
    pub local_cache: String,
    /// Shader build hash for the local build (kept apart from the bundled one's).
    #[serde(default)]
    pub local_shaders_hash: String,
    /// Settings, Features (`tuning`).
    #[serde(default)]
    pub tuning: tuning::Tuning,
    /// The first-run wizard was finished or skipped.
    #[serde(default)]
    pub wizard_done: bool,
}

fn default_preset() -> String {
    presets::STABLE.to_string()
}

fn default_runtime() -> String {
    BUNDLED.to_string()
}

fn default_engine() -> String {
    engine::WORLD.to_string()
}

fn default_slow_secs() -> u64 {
    60
}

fn yes() -> bool {
    true
}

impl Default for Config {
    fn default() -> Self {
        Config {
            game_path: String::new(),
            auto_menu: false,
            cache_dir: String::new(),
            shaders_hash: String::new(),
            world_len: 0,
            display: display::DisplayCfg::default(),
            engine: default_engine(),
            console_s: true,
            raytracing: false,
            fallback_light: true,
            debug_view: false,
            world_shaders_hash: String::new(),
            engine_v2: true,
            skip_slow: true,
            slow_secs: default_slow_secs(),
            discord: true,
            auto_update: true,
            preset: default_preset(),
            runtime_mode: default_runtime(),
            local_exe: String::new(),
            local_cache: String::new(),
            local_shaders_hash: String::new(),
            tuning: tuning::Tuning::default(),
            wizard_done: false,
        }
    }
}

pub enum Message {
    Log(String, LogKind),
    Fps(f32),
    Shader { done: u64, total: u64, secs: u64 },
    Pipelines(u32),
    World { done: u64, left: u64 },
    Ended(RunKind, Option<i32>),
    /// The engines found beside the launcher.
    RuntimeReady(Result<runtime::Found, String>),
    /// Shader build progress and its end (a summary, or why it did not finish).
    Build(shaderbuild::Progress),
    BuildEnded(Result<String, String>),
    UpdateFound(Result<Option<update::Release>, String>),
    UpdateProgress(u64, u64),
    UpdateDone(Result<String, String>),
    Picked(Option<PathBuf>),
    PickedExe(Option<PathBuf>),
    PickedCache(Option<PathBuf>),
    /// The local build's hash and description.
    Local(Option<String>, String),
    Verified(verify::Report),
    Saves(Result<Vec<cli::Slot>, String>),
    Backups(String, Result<Vec<cli::Backup>, String>),
    SavesDone(Result<String, String>),
    Crashes(Result<Vec<String>, String>),
}

pub struct Launcher {
    pub page: Page,
    pub page_at: Instant,
    pub started: Instant,
    pub config: Config,
    pub runtime_dir: Option<PathBuf>,
    pub runtime_note: String,
    /// Set when victory.exe is in the runtime folder.
    pub world_dir: Option<PathBuf>,
    pub world_hash: Option<String>,
    /// The running (or last) shader build's progress.
    pub build: Option<shaderbuild::Progress>,
    /// The header picture from the selected game folder, and the folder it was loaded for.
    pub header: Option<egui::TextureHandle>,
    pub header_for: Option<String>,
    pub presence: discord::Presence,
    pub presence_last: Option<discord::Activity>,
    pub run_started: u64,
    pub gpu_name: String,
    pub update: Option<update::Release>,
    pub update_note: String,
    pub update_busy: bool,
    pub report_note: String,
    /// Scroll offset of the Settings, Game tab and the height of its content.
    pub game_scroll: f32,
    pub game_content: f32,
    pub game_ok: bool,
    pub game_checks: Vec<game::FileCheck>,
    pub runtime_hash: Option<String>,
    pub eta: flow::Eta,
    pub eta_secs: Option<u64>,
    pub world_progress: Option<(u64, u64)>,
    pub cache_free: Option<(Instant, Option<u64>)>,
    pub journal: Option<(Instant, Option<u64>)>,
    pub game_edit: String,
    pub folder_note: (String, bool),
    pub folder_busy: bool,
    pub settings_tab: usize,
    pub monitors: Vec<display::Monitor>,
    /// The open dropdown on the Display tab (0 resolution, 1 monitor) and its scroll offset.
    pub dd: Option<u8>,
    pub dd_scroll: usize,
    pub running: Option<RunKind>,
    pub stop: Option<Arc<AtomicBool>>,
    pub logs: Vec<(String, LogKind)>,
    pub fps: Option<f32>,
    pub shader: Option<(u64, u64, u64)>,
    pub pipelines: Option<u32>,
    pub status: String,
    pub confirm_precompile: bool,
    pub copied_at: Option<Instant>,
    pub tx: Sender<Message>,
    pub rx: Receiver<Message>,
    pub mark: egui::TextureHandle,
    pub art: egui::TextureHandle,
    pub shot: Option<(String, u32)>,
    /// Result line of the last save backup (Settings, Game).
    pub save_note: String,
    /// When the current Play run started (a crash report newer than this belongs to it).
    pub play_since: Option<std::time::SystemTime>,
    /// The crash report written by the last Play run, if it crashed.
    pub crash_report: Option<PathBuf>,
    pub local_hash: Option<String>,
    pub local_desc: String,
    pub verify: Option<verify::Report>,
    pub verify_busy: bool,
    pub saves: Option<Result<Vec<cli::Slot>, String>>,
    pub backups: Option<(String, Result<Vec<cli::Backup>, String>)>,
    pub saves_busy: bool,
    pub saves_msg: String,
    pub crashes: Option<Result<Vec<String>, String>>,
    pub crashes_busy: bool,
    pub log_filter: String,
    /// The last run's log tail (from its file) when nothing ran in this session.
    pub last_log: Vec<String>,
    pub run_log: Option<(PathBuf, std::fs::File)>,
    /// Seconds of the UTC day when the current Play run started (for "time to world").
    pub run_tod: Option<f64>,
    pub perf: Option<perf::Perf>,
    pub wizard_step: usize,
    /// Features as they were when the running game started ("restart needed").
    pub tuning_at_start: Option<tuning::Tuning>,
    pub report_copied: Option<Instant>,
    pub shot_wait: u32,
}

pub fn config_path() -> PathBuf {
    runtime::data_dir().join("launcher.json")
}

pub fn valid_game(dir: &std::path::Path) -> bool {
    dir.join(GAME_EXE).is_file()
}

/// Walks up from a picked folder so choosing a subfolder still finds the game folder.
pub fn normalize(dir: &std::path::Path) -> Option<PathBuf> {
    let mut d = dir.to_path_buf();
    for _ in 0..4 {
        if valid_game(&d) {
            return Some(d);
        }
        if !d.pop() {
            break;
        }
    }
    None
}

impl Launcher {
    fn new(cc: &eframe::CreationContext<'_>) -> Self {
        theme::install(&cc.egui_ctx);
        let (tx, rx) = channel();
        let mark = {
            let img = image::load_from_memory(include_bytes!("../assets/logo.png")).unwrap().to_rgba8();
            let size = [img.width() as usize, img.height() as usize];
            cc.egui_ctx.load_texture("mark", egui::ColorImage::from_rgba_unmultiplied(size, img.as_raw()), egui::TextureOptions::LINEAR)
        };
        let art = cc.egui_ctx.load_texture("art", art::fallback(), egui::TextureOptions::LINEAR);
        let mut config: Config = std::fs::read(config_path())
            .ok()
            .and_then(|v| serde_json::from_slice(&v).ok())
            .unwrap_or_default();
        if !config.engine_v2 {
            // The World engine is the default now: its shader build takes minutes, not hours.
            config.engine = engine::WORLD.to_string();
            config.console_s = true;
            config.engine_v2 = true;
        }
        let fresh_cache = config.cache_dir.trim().is_empty();
        if fresh_cache {
            config.cache_dir = game::default_cache();
        }
        let game_ok = !config.game_path.is_empty() && valid_game(std::path::Path::new(&config.game_path));
        let game_checks = if game_ok { game::verify(std::path::Path::new(&config.game_path)) } else { Vec::new() };
        let game_edit = config.game_path.clone();
        let l = Launcher {
            page: Page::Play,
            // in the past: the first frame is fully drawn (no fade-in), even if frames are throttled
            page_at: Instant::now() - std::time::Duration::from_secs(1),
            started: Instant::now(),
            config,
            runtime_dir: None,
            runtime_note: "Looking for the engines…".into(),
            world_dir: None,
            world_hash: None,
            build: None,
            header: None,
            header_for: None,
            presence: discord::Presence::new(),
            presence_last: None,
            run_started: 0,
            gpu_name: report::system().gpu,
            update: None,
            update_note: update::staged().map(|v| format!("{v} is downloaded and will be used the next time you start the launcher.")).unwrap_or_default(),
            update_busy: false,
            report_note: String::new(),
            game_scroll: 0.,
            game_content: 0.,
            game_ok,
            game_checks,
            runtime_hash: None,
            eta: flow::Eta::default(),
            eta_secs: None,
            world_progress: None,
            cache_free: None,
            journal: None,
            game_edit,
            folder_note: (String::new(), true),
            folder_busy: false,
            settings_tab: 0,
            monitors: display::monitors(),
            dd: None,
            dd_scroll: 0,
            running: None,
            stop: None,
            logs: Vec::new(),
            fps: None,
            shader: None,
            pipelines: None,
            status: String::new(),
            confirm_precompile: false,
            copied_at: None,
            tx,
            rx,
            mark,
            art,
            shot: std::env::args().find_map(|a| a.strip_prefix("--screenshot=").map(|p| (p.to_string(), 0))),
            save_note: String::new(),
            play_since: None,
            crash_report: None,
            local_hash: None,
            local_desc: String::new(),
            verify: None,
            verify_busy: false,
            saves: None,
            backups: None,
            saves_busy: false,
            saves_msg: String::new(),
            crashes: None,
            crashes_busy: false,
            log_filter: String::new(),
            last_log: perf::last_log().map(|p| perf::tail(&p, 400)).unwrap_or_default(),
            run_log: None,
            run_tod: None,
            perf: perf::last_log().map(|p| perf::parse(perf::tail(&p, 20_000).iter().map(String::as_str), None)).filter(|p| p.line().is_some()),
            wizard_step: 0,
            tuning_at_start: None,
            report_copied: None,
            shot_wait: 0,
        };
        if fresh_cache {
            l.save();
        }
        l.unpack_runtime();
        let mut l = l;
        if l.config.auto_update && l.update_note.is_empty() {
            l.check_update();
        }
        let l = l;
        let mut l = l;
        // `--page=settings` opens Settings on its Display tab (`--tab=game|display|options|about`)
        if std::env::args().any(|a| a == "--page=settings") {
            l.page = Page::Settings;
            l.settings_tab = 1;
        }
        if let Some(t) = std::env::args().find_map(|a| a.strip_prefix("--tab=").map(str::to_string)) {
            l.settings_tab = ["game", "display", "options", "about", "runtime", "features"].iter().position(|n| *n == t).unwrap_or(l.settings_tab);
        }
        // An existing setup skips the first-run wizard.
        if !l.config.wizard_done && l.game_ok {
            l.config.wizard_done = true;
            l.save();
        }
        if !l.config.wizard_done && !std::env::args().any(|a| a.starts_with("--page=")) {
            l.page = Page::Welcome;
        }
        // `--page=<name>` opens a page (screenshots, support); `--wizard=N` a wizard step.
        if let Some(p) = std::env::args().find_map(|a| a.strip_prefix("--page=").map(str::to_string)) {
            l.page = match p.as_str() {
                "play" => Page::Play,
                "news" => Page::News,
                "setup" => Page::Setup,
                "logs" => Page::Logs,
                "links" => Page::Links,
                "saves" => Page::Saves,
                "verify" => Page::Verify,
                "welcome" => Page::Welcome,
                _ => l.page,
            };
            match l.page {
                Page::Saves => l.refresh_saves(),
                Page::Verify => l.run_verify(),
                Page::Logs => l.refresh_crashes(),
                _ => {}
            }
        }
        if let Some(n) = std::env::args().find_map(|a| a.strip_prefix("--wizard=").and_then(|n| n.parse().ok())) {
            l.page = Page::Welcome;
            l.wizard_step = n;
        }
        l.refresh_local();
        l
    }

    /// Checks GitHub Releases for a newer launcher (a background thread).
    pub fn check_update(&mut self) {
        self.update_note = "Checking for updates…".into();
        let tx = self.tx.clone();
        std::thread::spawn(move || {
            let _ = tx.send(Message::UpdateFound(update::check()));
        });
    }

    /// Downloads, verifies and stages the update found by check_update.
    pub fn download_update(&mut self) {
        let Some(rel) = self.update.clone() else { return };
        self.update_busy = true;
        self.update_note = "Downloading…".into();
        let tx = self.tx.clone();
        std::thread::spawn(move || {
            let ptx = tx.clone();
            let r = update::download(&rel, &move |got, total| {
                let _ = ptx.send(Message::UpdateProgress(got, total));
            });
            let _ = tx.send(Message::UpdateDone(r.map(|_| rel.version.clone())));
        });
    }

    /// What Discord shows now: building, playing, or nothing.
    fn presence_now(&mut self) -> Option<discord::Activity> {
        if !self.config.discord {
            return None;
        }
        let now = std::time::SystemTime::now().duration_since(std::time::UNIX_EPOCH).map_or(0, |d| d.as_secs());
        let (details, state) = match self.running? {
            RunKind::Play => {
                let place = if self.fps.is_some() { "In game" } else { "Starting the game" };
                (place.to_string(), format!("{} engine", if self.world_selected() { "World" } else { "Classic" }))
            }
            RunKind::Precompile => ("Building shaders".to_string(), "Getting ready to play".to_string()),
            RunKind::Pipelines => ("Building pipelines".to_string(), "Getting ready to play".to_string()),
        };
        if self.run_started == 0 {
            self.run_started = now;
        }
        Some(discord::Activity { details, state, start: self.run_started, small: discord::vendor_key(&self.gpu_name) })
    }

    fn update_presence(&mut self) {
        let want = self.presence_now();
        if want.is_none() {
            self.run_started = 0;
        }
        if want != self.presence_last {
            self.presence.set(want.clone());
            self.presence_last = want;
        }
    }

    /// Gathers the hardware report and opens a prefilled GitHub issue (or copies it).
    pub fn hardware_report(&mut self, ctx: &egui::Context) {
        let sys = report::system();
        let lines: Vec<String> = self.logs.iter().map(|(t, _)| t.clone()).collect();
        let tail = lines[lines.len().saturating_sub(200)..].join("\n");
        let folders = [(self.config.game_path.as_str(), "<game>"), (self.config.cache_dir.as_str(), "<cache>")];
        let engine = if self.world_selected() { format!("World engine (VIctory-based), {}", self.world_hash.as_deref().map(|h| &h[..12]).unwrap_or("?")) } else { format!("Classic engine (XCL), {}", self.runtime_hash.as_deref().map(|h| &h[..12]).unwrap_or("?")) };
        let build = self.build.as_ref().map(|b| format!("\nLast shader build: {} in {}", b.name, flow::duration(b.started.elapsed().as_secs()))).unwrap_or_default();
        let fields: Vec<(&str, String)> = vec![
            ("gpu", sys.gpu.clone()),
            ("driver", sys.driver.clone()),
            ("system", format!("{}; {} GB RAM; {}", sys.cpu, sys.ram_gb, sys.os)),
            ("runtime", format!("Launcher v{VERSION}; {engine}{build}")),
            ("fps", report::fps_summary(&lines)),
            ("notes", report::scrub(&format!("Last {} log lines:\n`\n{tail}\n`", tail.lines().count()), &folders)),
        ];
        let url = report::issue_url(&fields);
        if url.len() <= report::MAX_URL {
            ctx.open_url(egui::OpenUrl::new_tab(&url));
            self.report_note = "Opened a prefilled hardware report in your browser. Check it, add how far the game got, then submit.".into();
        } else {
            ctx.copy_text(report::as_text(&fields));
            let short: Vec<(&str, String)> = fields[..5].to_vec();
            ctx.open_url(egui::OpenUrl::new_tab(report::issue_url(&short)));
            self.report_note = "The report is too long for a link: the full text (with the log) is on your clipboard. Paste it into the Notes field of the issue that opened.".into();
        }
        self.log(self.report_note.clone(), LogKind::Info);
    }

    pub fn unpack_runtime(&self) {
        let tx = self.tx.clone();
        std::thread::spawn(move || {
            let _ = tx.send(Message::RuntimeReady(runtime::locate()));
        });
    }

    pub fn save(&self) {
        if let Some(dir) = config_path().parent() {
            let _ = std::fs::create_dir_all(dir);
        }
        if let Ok(json) = serde_json::to_vec_pretty(&self.config) {
            let _ = std::fs::write(config_path(), json);
        }
    }

    pub fn go(&mut self, page: Page) {
        if self.page != page {
            self.page = page;
            self.page_at = Instant::now();
        }
    }

    pub fn log(&mut self, text: impl Into<String>, kind: LogKind) {
        self.logs.push((text.into(), kind));
        if self.logs.len() > MAX_LOG {
            self.logs.drain(..self.logs.len() - MAX_LOG);
        }
    }

    pub fn use_folder(&mut self, dir: PathBuf) {
        match normalize(&dir) {
            Some(found) => {
                self.config.game_path = found.display().to_string();
                self.game_edit = self.config.game_path.clone();
                self.game_ok = true;
                self.game_checks = game::verify(&found);
                let missing = game::missing(&self.game_checks);
                self.folder_note = if missing.is_empty() {
                    (format!("All game files found: {}", game::summary(&self.game_checks)), true)
                } else {
                    (format!("Found {GAME_EXE}, but missing {}.", missing.join(", ")), false)
                };
                self.save();
            }
            None => {
                self.game_ok = false;
                self.game_checks = Vec::new();
                self.folder_note = (format!("That folder does not contain {GAME_EXE}."), false);
            }
        }
    }

    pub fn pick_folder(&mut self) {
        self.folder_busy = true;
        let tx = self.tx.clone();
        let start = PathBuf::from(self.game_edit.trim());
        std::thread::spawn(move || {
            let mut d = rfd::FileDialog::new().set_title("Select your game folder");
            if start.is_dir() {
                d = d.set_directory(&start);
            }
            let _ = tx.send(Message::Picked(d.pick_folder()));
        });
    }

    /// Copies the title's saves to `<cache>\save-backups\<date>` and keeps the newest `saves::KEEP`.
    pub fn backup_saves(&mut self) -> Option<String> {
        if !self.game_ok {
            return None;
        }
        let data = game::title_data_dir(std::path::Path::new(&self.config.game_path));
        let root = saves::backup_root(&self.config.cache_dir);
        let note = match saves::backup(&saves::sources(&data), &root, &saves::now_stamp()) {
            Ok(Some((dest, files, bytes))) => {
                saves::prune(&root, saves::KEEP);
                format!("Backed up {files} files ({}) to {}", game::human(bytes), dest.display())
            }
            Ok(None) => "No saves to back up yet.".to_string(),
            Err(e) => format!("Backup failed: {e}"),
        };
        self.log(note.clone(), LogKind::Info);
        Some(note)
    }

    pub fn start(&mut self, kind: RunKind) {
        if self.running.is_some() {
            return;
        }
        if !self.engine_ready() {
            self.status = self.runtime_note.clone();
            return;
        }
        if !self.game_ok {
            self.status = format!("Pick a game folder containing {GAME_EXE} first.");
            self.go(Page::Settings);
            self.settings_tab = 0;
            return;
        }
        let game = PathBuf::from(&self.config.game_path);
        let _ = std::fs::create_dir_all(self.config.cache_dir.trim());
        if kind == RunKind::Precompile {
            return self.start_build(game);
        }
        if kind == RunKind::Play {
            self.save_note = self.backup_saves().unwrap_or_default();
            self.play_since = Some(std::time::SystemTime::now());
            self.crash_report = None;
        }
        let rt = runtime::runtime_dir();
        if self.world_selected() {
            if kind != RunKind::Play {
                return;
            }
            let exe = self.victory_path();
            let mut cmd = Command::new(&exe);
            let args = self.play_args(&game);
            cmd.current_dir(self.victory_dir()).args(&args);
            for k in presets::CLEARED {
                cmd.env_remove(k);
            }
            let env = self.play_env(&game);
            for (k, v) in &env {
                cmd.env(k, v);
            }
            self.fps = None;
            self.perf = None;
            self.status = "Running".into();
            self.tuning_at_start = Some(self.config.tuning.clone());
            self.run_tod = std::time::SystemTime::now().duration_since(std::time::UNIX_EPOCH).ok().map(|d| (d.as_secs_f64()).rem_euclid(86400.));
            let path = perf::new_log();
            self.run_log = std::fs::File::create(&path).ok().map(|f| (path, f));
            self.log(format!("> {} {}  [{} preset, {}]", exe.display(), args.join(" "), presets::label(&self.config.preset), self.build_label()), LogKind::Info);
            self.log(format!("  env: {}", env.iter().map(|(k, v)| format!("{k}={v}")).collect::<Vec<_>>().join(" ")), LogKind::Info);
            return self.spawn(cmd, kind);
        }
        let _ = std::fs::create_dir_all(game::dxil_dir(&self.config.cache_dir));
        let mut cmd = Command::new(runtime::xcl_exe());
        cmd.current_dir(&rt)
            .env("XCL_NO_WATCHDOG", "1")
            .env("XCL_DXC_DIR", &rt)
            .env("XCL_DXIL_CACHE", game::dxil_dir(&self.config.cache_dir));
        match kind {
            RunKind::Play => {
                cmd.env("XCL_DXC_THREADS", "4").env("XCL_PSO_PRELOAD", "0").arg("run").arg(&game);
                for (k, v) in display::env(&self.config.display, self.monitors.len()) {
                    cmd.env(k, v);
                }
                if self.config.auto_menu {
                    let at: Vec<String> = (22..=70).step_by(6).map(|s| format!("{s}:A")).collect();
                    cmd.env("XCL_PRESS_AT", at.join(","));
                }
                self.fps = None;
                self.status = "Starting…".into();
                self.log(format!("> xcl.exe run \"{}\"", game.display()), LogKind::Info);
            }
            RunKind::Pipelines => {
                cmd.arg("pipelines").arg(&game).args(["--threads", "2"]);
                self.world_progress = None;
                self.status = "Building world pipelines…".into();
                self.log(format!("> xcl.exe pipelines \"{}\" --threads 2", game.display()), LogKind::Info);
            }
            RunKind::Precompile => return,
        }
        self.spawn(cmd, kind);
    }

    /// Starts the shader build of the selected engine on its own thread (`shaderbuild`).
    fn start_build(&mut self, game: PathBuf) {
        let world = self.world_selected();
        let hash = if world { self.effective_world_hash().map(str::to_string) } else { self.runtime_hash.clone() }.unwrap_or_default();
        let stop = Arc::new(AtomicBool::new(false));
        let env = if world { self.play_env(&game) } else { Vec::new() };
        let job = shaderbuild::Job {
            world,
            exe: if world { self.victory_path() } else { runtime::xcl_exe() },
            runtime: if world { self.victory_dir() } else { runtime::runtime_dir() },
            game,
            cache_dir: self.config.cache_dir.trim().to_string(),
            hash,
            env,
            slow_secs: self.config.skip_slow.then_some(self.config.slow_secs.max(10)),
            stop: stop.clone(),
            tx: self.tx.clone(),
        };
        self.stop = Some(stop);
        self.running = Some(RunKind::Precompile);
        self.build = None;
        self.shader = None;
        self.pipelines = None;
        self.status = "Building shaders…".into();
        std::thread::spawn(move || shaderbuild::build(job));
    }

    /// True when the selected engine was found beside the launcher.
    pub fn engine_ready(&self) -> bool {
        if self.world_selected() {
            if self.local_selected() { self.local_ok() } else { self.world_dir.is_some() }
        } else {
            self.runtime_dir.is_some() && self.runtime_hash.is_some()
        }
    }

    /// True when the World engine is selected and present (else Classic is used).
    pub fn world_selected(&self) -> bool {
        (engine::is_world(&self.config) || self.runtime_hash.is_none()) && (self.world_dir.is_some() || self.local_selected())
    }

    /// "Local build" is chosen in Settings, Runtime.
    pub fn local_selected(&self) -> bool {
        self.config.runtime_mode == LOCAL
    }

    pub fn local_ok(&self) -> bool {
        let p = std::path::Path::new(self.config.local_exe.trim());
        !self.config.local_exe.trim().is_empty() && p.is_file()
    }

    /// The World engine executable that runs: the local build, else the bundled runtime.
    pub fn victory_path(&self) -> PathBuf {
        if self.local_selected() { PathBuf::from(self.config.local_exe.trim()) } else { runtime::victory_exe() }
    }

    pub fn victory_dir(&self) -> PathBuf {
        self.victory_path().parent().map(PathBuf::from).unwrap_or_else(runtime::runtime_dir)
    }

    /// VICTORY_CACHE_DIR before the preset's own: the local build's cache folder, else `<cache>\victory`.
    pub fn victory_cache_base(&self) -> PathBuf {
        if self.local_selected() {
            if self.config.local_cache.trim().is_empty() { std::path::Path::new(self.config.cache_dir.trim()).join("local") } else { PathBuf::from(self.config.local_cache.trim()) }
        } else {
            engine::victory_cache(&self.config.cache_dir)
        }
    }

    pub fn victory_cache(&self) -> PathBuf {
        presets::cache_for(&self.config.preset, &self.victory_cache_base())
    }

    pub fn effective_world_hash(&self) -> Option<&str> {
        if self.local_selected() { self.local_hash.as_deref() } else { self.world_hash.as_deref() }
    }

    /// Which runtime build a run uses, for the Play page and the log.
    pub fn build_label(&self) -> String {
        if self.local_selected() {
            if !self.local_ok() {
                return "Local build: pick a victory.exe in Settings, Runtime".into();
            }
            let d = if self.local_desc.is_empty() { "reading the build…".to_string() } else { self.local_desc.clone() };
            format!("Local build ({d})")
        } else {
            format!("Bundled runtime ({})", presets::BUNDLED_BUILD)
        }
    }

    /// `run "<exe>" --execute`, plus the Series S profile (always for Stable and Experimental,
    /// like play.ps1; Custom follows the Series S option).
    pub fn play_args(&self, game: &std::path::Path) -> Vec<String> {
        if self.config.preset == presets::CUSTOM {
            return engine::world_args(&self.config, game);
        }
        vec!["run".into(), game.join(GAME_EXE).display().to_string(), "--execute".into(), "--console".into(), "series-s".into()]
    }

    /// The World engine environment: the preset's, then Settings, Features, then the launcher's
    /// `victory.toml` (VICTORY_CONFIG), written here.
    pub fn play_env(&self, game: &std::path::Path) -> Vec<(&'static str, String)> {
        let cache = self.victory_cache();
        let mut env = match self.config.preset.as_str() {
            presets::EXPERIMENTAL => presets::experimental_env(game, &cache),
            presets::CUSTOM => {
                let mut e = engine::world_env(&self.config, game, &self.config.cache_dir);
                presets::set(&mut e, "VICTORY_CACHE_DIR", cache.display().to_string());
                e
            }
            _ => presets::stable_env(game, &cache),
        };
        let t = &self.config.tuning;
        let fsr = tuning::fsr_available(&self.victory_dir());
        for (k, v) in t.env(fsr) {
            presets::set(&mut env, k, v);
        }
        if t.perf_log {
            let level = env.iter().find(|(k, _)| *k == "VICTORY_LOG").map(|(_, v)| v.clone()).unwrap_or_else(|| "info".into());
            presets::set(&mut env, "VICTORY_LOG", format!("{level},victory_gpu::host=info"));
        }
        let toml = runtime::data_dir().join("victory.toml");
        if tuning::write_toml(&toml, &t.toml_keys(fsr)).is_ok() {
            presets::set(&mut env, "VICTORY_CONFIG", toml.display().to_string());
        }
        env
    }

    /// Hashes and describes the local build on a worker thread.
    pub fn refresh_local(&mut self) {
        self.local_hash = None;
        self.local_desc.clear();
        if !self.local_ok() {
            return;
        }
        let exe = PathBuf::from(self.config.local_exe.trim());
        let tx = self.tx.clone();
        std::thread::spawn(move || {
            let _ = tx.send(Message::Local(runtime::file_hash(&exe), cli::describe(&exe)));
        });
    }

    pub fn pick_local_exe(&mut self) {
        let tx = self.tx.clone();
        let start = PathBuf::from(self.config.local_exe.trim());
        std::thread::spawn(move || {
            let mut d = rfd::FileDialog::new().set_title("Select a victory.exe build").add_filter("victory.exe", &["exe"]);
            if let Some(dir) = start.parent().filter(|d| d.is_dir()) {
                d = d.set_directory(dir);
            }
            let _ = tx.send(Message::PickedExe(d.pick_file()));
        });
    }

    pub fn pick_local_cache(&mut self) {
        let tx = self.tx.clone();
        std::thread::spawn(move || {
            let _ = tx.send(Message::PickedCache(rfd::FileDialog::new().set_title("Select the local build's cache folder").pick_folder()));
        });
    }

    /// Environment for the runtime's own tools (saves, crashes): the game and the cache.
    fn tool_env(&self) -> Vec<(&'static str, String)> {
        let game = PathBuf::from(&self.config.game_path);
        let mut env = presets::stable_env(&game, &self.victory_cache());
        env.retain(|(k, _)| matches!(*k, "VICTORY_GAME_DIR" | "VICTORY_CACHE_DIR"));
        let toml = runtime::data_dir().join("victory.toml");
        if toml.is_file() {
            env.push(("VICTORY_CONFIG", toml.display().to_string()));
        }
        env
    }

    pub fn run_verify(&mut self) {
        if self.verify_busy || !self.game_ok {
            return;
        }
        self.verify_busy = true;
        let dir = PathBuf::from(&self.config.game_path);
        let tx = self.tx.clone();
        std::thread::spawn(move || {
            let _ = tx.send(Message::Verified(verify::run(&dir)));
        });
    }

    pub fn refresh_saves(&mut self) {
        if self.saves_busy {
            return;
        }
        self.saves_busy = true;
        let (exe, env, tx) = (self.victory_path(), self.tool_env(), self.tx.clone());
        std::thread::spawn(move || {
            let r = cli::run(&exe, &["saves".into(), "list".into()], &env).map(|t| cli::parse_saves(&t));
            let _ = tx.send(Message::Saves(r));
        });
    }

    pub fn list_backups(&mut self, slot: &str) {
        let (exe, env, tx, slot) = (self.victory_path(), self.tool_env(), self.tx.clone(), slot.to_string());
        std::thread::spawn(move || {
            let r = cli::run(&exe, &["saves".into(), "restore".into(), slot.clone()], &env).map(|t| cli::parse_backups(&t));
            let _ = tx.send(Message::Backups(slot, r));
        });
    }

    /// `victory saves <args>` (backup = copy to a new slot, restore = a container's backup).
    pub fn saves_command(&mut self, args: Vec<String>) {
        if self.saves_busy {
            return;
        }
        self.saves_busy = true;
        self.saves_msg = format!("Running victory saves {}…", args.join(" "));
        let (exe, env, tx) = (self.victory_path(), self.tool_env(), self.tx.clone());
        std::thread::spawn(move || {
            let mut a = vec!["saves".to_string()];
            a.extend(args);
            let _ = tx.send(Message::SavesDone(cli::run(&exe, &a, &env)));
        });
    }

    pub fn refresh_crashes(&mut self) {
        if self.crashes_busy {
            return;
        }
        self.crashes_busy = true;
        let (exe, env, tx) = (self.victory_path(), self.tool_env(), self.tx.clone());
        std::thread::spawn(move || {
            let r = match cli::run(&exe, &["crashes".into(), "list".into()], &env) {
                Ok(t) => Ok(t.lines().filter(|l| !l.trim().is_empty()).map(str::to_string).collect()),
                Err(e) if cli::unknown_command(&e) => {
                    // builds before fx-stable: read the crash folders and the old reports directly
                    let mut v = cli::crashes_from_folder(&cli::crash_dir());
                    if let Ok(rd) = std::fs::read_dir(saves::reports_dir()) {
                        let mut z: Vec<String> = rd.flatten().map(|e| e.file_name().to_string_lossy().into_owned()).filter(|n| n.ends_with(".zip")).collect();
                        z.sort();
                        v.extend(z.into_iter().map(|n| format!("{n}  (crash report)")));
                    }
                    if v.is_empty() { Err("This runtime build has no `crashes` command and no crash folders were found.".into()) } else { Ok(v) }
                }
                Err(e) => Err(e),
            };
            let _ = tx.send(Message::Crashes(r));
        });
    }

    /// A text bundle for an issue: versions, preset, build, verify result, the readout, the
    /// crash list and the log tail, with personal folders removed.
    pub fn crash_report_text(&self) -> String {
        let mut t = format!("ScarlettPort launcher v{VERSION}\nPreset: {}\nRuntime: {}\nGPU: {}\n", presets::label(&self.config.preset), self.build_label(), self.gpu_name);
        if let Some(v) = &self.verify {
            t += &format!("Game files: {} problem(s){}\n", v.problems(), if v.pack_problem { ", pack\\ incomplete or split" } else { "" });
        }
        if let Some(p) = self.perf.as_ref().and_then(perf::Perf::line) {
            t += &format!("Last run: {p}\n");
        }
        t += "\nCrashes:\n";
        match &self.crashes {
            Some(Ok(v)) => v.iter().for_each(|l| t += &format!("  {l}\n")),
            Some(Err(e)) => t += &format!("  {e}\n"),
            None => t += "  (not listed)\n",
        }
        t += "\nLog (last 150 lines):\n";
        let lines: Vec<String> = if self.logs.is_empty() { self.last_log.clone() } else { self.logs.iter().map(|(l, _)| l.clone()).collect() };
        for l in &lines[lines.len().saturating_sub(150)..] {
            t += l;
            t.push('\n');
        }
        report::scrub(&t, &[(self.config.game_path.as_str(), "<game>"), (self.config.cache_dir.as_str(), "<cache>")])
    }

    fn spawn(&mut self, mut cmd: Command, kind: RunKind) {
        cmd.stdout(Stdio::piped()).stderr(Stdio::piped()).stdin(Stdio::null());
        #[cfg(windows)]
        {
            use std::os::windows::process::CommandExt;
            // CREATE_NO_WINDOW (output goes to the Logs page), plus BELOW_NORMAL_PRIORITY_CLASS for builds
            let low = if kind == RunKind::Play { 0 } else { 0x0000_4000 };
            cmd.creation_flags(0x0800_0000 | low);
        }
        let mut child = match cmd.spawn() {
            Ok(c) => c,
            Err(e) => {
                self.status = format!("Could not start the engine: {e}");
                self.log(self.status.clone(), LogKind::Error);
                return;
            }
        };
        let stop = Arc::new(AtomicBool::new(false));
        self.stop = Some(stop.clone());
        self.running = Some(kind);
        let tx = self.tx.clone();
        for stream in [child.stdout.take().map(|s| Box::new(s) as Box<dyn Read + Send>), child.stderr.take().map(|s| Box::new(s) as Box<dyn Read + Send>)]
            .into_iter()
            .flatten()
        {
            let tx = tx.clone();
            std::thread::spawn(move || split_lines(stream, |l| {
                let _ = tx.send(Message::Log(l, LogKind::Out));
            }));
        }
        std::thread::spawn(move || {
            let code = loop {
                if stop.load(Ordering::Relaxed) {
                    let _ = child.kill();
                    break child.wait().ok().and_then(|s| s.code());
                }
                match child.try_wait() {
                    Ok(Some(s)) => break s.code(),
                    Ok(None) => std::thread::sleep(std::time::Duration::from_millis(150)),
                    Err(_) => break None,
                }
            };
            std::thread::sleep(std::time::Duration::from_millis(300)); // let the readers drain
            let _ = tx.send(Message::Ended(kind, code));
        });
    }

    pub fn game_complete(&self) -> bool {
        self.game_ok && !self.game_checks.is_empty() && game::missing(&self.game_checks).is_empty()
    }

    /// The hash the shaders were built for and the hash of the selected engine.
    fn shader_hashes(&self) -> (&str, Option<&str>) {
        if self.world_selected() {
            let built = if self.local_selected() { &self.config.local_shaders_hash } else { &self.config.world_shaders_hash };
            (built, self.effective_world_hash())
        } else {
            (&self.config.shaders_hash, self.runtime_hash.as_deref())
        }
    }

    pub fn stage(&self) -> flow::Stage {
        let (built_hash, runtime_hash) = self.shader_hashes();
        flow::stage(&flow::Inputs {
            game_complete: self.game_complete(),
            precompiling: self.running == Some(RunKind::Precompile),
            built_hash,
            runtime_hash,
            inline_shaders: false,
        })
    }

    pub fn world(&self) -> flow::World {
        if self.world_selected() {
            // World pipelines are a Classic feature; the World engine records its own as it runs.
            return if self.stage() == flow::Stage::Ready { flow::World::Done } else { flow::World::Waiting };
        }
        let len = self.journal.and_then(|(_, l)| l);
        flow::world(self.stage(), self.running == Some(RunKind::Pipelines), len, self.config.world_len)
    }

    /// True when shaders were built, but by a different engine build than the one beside the launcher.
    pub fn shaders_stale(&self) -> bool {
        let (built, now) = self.shader_hashes();
        !built.is_empty() && now.is_some_and(|h| h != built)
    }

    pub fn stop_run(&mut self) {
        if let Some(s) = &self.stop {
            s.store(true, Ordering::Relaxed);
            self.status = "Stopping…".into();
        }
    }

    /// Loads the header picture from the game folder when the folder changed (`art::from_game`).
    fn refresh_header(&mut self, ctx: &egui::Context) {
        let want = if self.game_ok { Some(self.config.game_path.clone()) } else { None };
        if self.header_for == want {
            return;
        }
        self.header_for = want.clone();
        self.header = want.and_then(|g| art::from_game(std::path::Path::new(&g))).map(|img| ctx.load_texture("game-art", img, egui::TextureOptions::LINEAR));
    }

    /// `--screenshot=FILE` renders the window to a PNG (the launcher's own framebuffer, so
    /// nothing in front of it can end up in the picture), then exits.
    fn screenshot(&mut self, ctx: &egui::Context) {
        let busy = self.verify_busy || self.saves_busy || self.crashes_busy || self.local_desc.is_empty() && self.local_ok();
        let Some((path, frames)) = &mut self.shot else { return };
        *frames += 1;
        if *frames == 90 && busy {
            *frames = 89; // wait for the page's background work (up to the cap below)
            self.shot_wait += 1;
        }
        if *frames == 90 || self.shot_wait > 2400 {
            self.shot_wait = 0;
            ctx.send_viewport_cmd(egui::ViewportCommand::Screenshot(egui::UserData::default()));
        }
        let image = ctx.input(|i| {
            i.events.iter().find_map(|e| match e {
                egui::Event::Screenshot { image, .. } => Some(image.clone()),
                _ => None,
            })
        });
        if let Some(img) = image {
            let ppp = ctx.pixels_per_point();
            let _ = ppp;
            let [w, h] = img.size;
            if let Some(buf) = image::RgbaImage::from_raw(w as u32, h as u32, img.as_raw().to_vec()) {
                let _ = buf.save(&*path);
            }
            ctx.send_viewport_cmd(egui::ViewportCommand::Close);
        }
        ctx.request_repaint();
    }

    fn pump(&mut self, ctx: &egui::Context) {
        if self.journal.is_none_or(|(t, _)| t.elapsed().as_secs() >= 2) {
            let len = if self.game_ok { game::journal_len(std::path::Path::new(&self.config.game_path)) } else { None };
            self.journal = Some((Instant::now(), len));
        }
        if self.cache_free.is_none_or(|(t, _)| t.elapsed().as_secs() >= 3) {
            self.cache_free = Some((Instant::now(), game::free_space(std::path::Path::new(self.config.cache_dir.trim()))));
        }
        while let Ok(m) = self.rx.try_recv() {
            match m {
                Message::Log(t, k) => {
                    parse_line(&t, &self.tx);
                    if self.running == Some(RunKind::Play) {
                        if let Some((_, f)) = &mut self.run_log {
                            use std::io::Write;
                            let _ = writeln!(f, "{t}");
                        }
                    }
                    self.log(t, k);
                }
                Message::Fps(f) => {
                    self.fps = Some(f);
                    if self.running == Some(RunKind::Play) {
                        self.status = format!("Running  ·  {f:.1} fps");
                    }
                }
                Message::Shader { done, total, secs } => {
                    self.shader = Some((done, total, secs));
                    self.eta_secs = self.eta.remaining_secs(done, total, secs);
                }
                Message::World { done, left } => self.world_progress = Some((done, left)),
                Message::Pipelines(n) => self.pipelines = Some(n),
                Message::Ended(kind, code) => {
                    self.running = None;
                    self.stop = None;
                    self.fps = None;
                    if code == Some(0) && kind == RunKind::Pipelines {
                        if let Some(l) = game::journal_len(std::path::Path::new(&self.config.game_path)) {
                            self.config.world_len = l;
                            self.save();
                        }
                    }
                    if kind == RunKind::Play && code != Some(0) {
                        let since = self.play_since.unwrap_or(std::time::UNIX_EPOCH);
                        self.crash_report = saves::fresh_report(&saves::reports_dir(), since);
                    }
                    if kind == RunKind::Play {
                        self.tuning_at_start = None;
                        if let Some((path, f)) = self.run_log.take() {
                            drop(f);
                            let lines = perf::tail(&path, 200_000);
                            let p = perf::parse(lines.iter().map(String::as_str), self.run_tod);
                            self.last_log = lines[lines.len().saturating_sub(400)..].to_vec();
                            if let Some(l) = p.line() {
                                self.log(format!("Performance: {l}"), LogKind::Info);
                            }
                            self.perf = Some(p);
                        }
                    }
                    let note = match code {
                        Some(0) => "Finished.".to_string(),
                        Some(c) => format!("Stopped (exit code {c})."),
                        None => "Stopped.".to_string(),
                    };
                    self.status = if self.crash_report.is_some() { "The game crashed - a report was saved.".to_string() } else { note.clone() };
                    let what = if kind == RunKind::Play { "Game" } else { "World pipeline build" };
                    self.log(format!("{what} ended. {note}"), if code == Some(0) { LogKind::Info } else { LogKind::Error });
                }
                Message::Build(p) => self.build = Some(p),
                Message::BuildEnded(r) => {
                    self.running = None;
                    self.stop = None;
                    match r {
                        Ok(note) => {
                            if self.world_selected() && self.local_selected() {
                                self.config.local_shaders_hash = self.local_hash.clone().unwrap_or_default();
                            } else if self.world_selected() {
                                self.config.world_shaders_hash = self.world_hash.clone().unwrap_or_default();
                            } else {
                                self.config.shaders_hash = self.runtime_hash.clone().unwrap_or_default();
                            }
                            self.save();
                            self.log(format!("Shader build ended. {note}"), LogKind::Info);
                            self.status = note;
                        }
                        Err(e) => {
                            self.log(format!("Shader build ended. {e}"), LogKind::Error);
                            self.status = e;
                        }
                    }
                }
                Message::RuntimeReady(r) => match r {
                    Ok(found) => {
                        self.runtime_note = found.dir.display().to_string();
                        self.world_dir = found.world.is_some().then(|| found.dir.clone());
                        self.world_hash = found.world;
                        self.runtime_hash = found.classic;
                        self.runtime_dir = Some(found.dir);
                    }
                    Err(e) => {
                        self.runtime_note = e;
                        self.log(self.runtime_note.clone(), LogKind::Error);
                    }
                },
                Message::UpdateFound(r) => match r {
                    Ok(Some(rel)) => {
                        self.log(format!("Update available: {} ({})", rel.version, rel.page), LogKind::Info);
                        self.update = Some(rel);
                    }
                    Ok(None) => self.update_note = format!("You have the newest launcher (v{VERSION})."),
                    Err(e) => self.update_note = format!("Could not check for updates: {e}"),
                },
                Message::UpdateProgress(got, total) => {
                    self.update_note = if total > 0 { format!("Downloading… {} of {}", game::human(got), game::human(total)) } else { format!("Downloading… {}", game::human(got)) };
                }
                Message::UpdateDone(r) => {
                    self.update_busy = false;
                    self.update_note = match r {
                        Ok(v) => format!("{v} downloaded and verified. It is used the next time you start the launcher (this version is kept for a rollback)."),
                        Err(e) => format!("Update failed: {e}"),
                    };
                    self.log(self.update_note.clone(), LogKind::Info);
                }
                Message::Picked(p) => {
                    self.folder_busy = false;
                    if let Some(p) = p {
                        self.use_folder(p);
                        if self.page == Page::Welcome && self.game_ok {
                            self.wizard_step = 1;
                            self.run_verify();
                        }
                    }
                }
                Message::PickedExe(p) => {
                    if let Some(p) = p {
                        self.config.local_exe = p.display().to_string();
                        self.config.runtime_mode = LOCAL.into();
                        self.save();
                        self.refresh_local();
                    }
                }
                Message::PickedCache(p) => {
                    if let Some(p) = p {
                        self.config.local_cache = p.display().to_string();
                        self.save();
                    }
                }
                Message::Local(hash, desc) => {
                    self.local_hash = hash;
                    self.local_desc = desc;
                }
                Message::Verified(r) => {
                    self.verify_busy = false;
                    self.log(format!("Verify game files: {} problem(s){}", r.problems(), if r.pack_problem { "; pack\\ is incomplete or split" } else { "" }), if r.problems() > 0 { LogKind::Error } else { LogKind::Info });
                    self.verify = Some(r);
                }
                Message::Saves(r) => {
                    self.saves_busy = false;
                    self.saves = Some(r);
                }
                Message::Backups(slot, r) => self.backups = Some((slot, r)),
                Message::SavesDone(r) => {
                    self.saves_busy = false;
                    self.saves_msg = match r {
                        Ok(t) => t.lines().filter(|l| !l.trim().is_empty()).last().unwrap_or("Done.").trim().to_string(),
                        Err(e) => format!("Failed: {e}"),
                    };
                    self.log(format!("Saves: {}", self.saves_msg), LogKind::Info);
                    let slot = self.backups.as_ref().map(|(s, _)| s.clone());
                    self.refresh_saves();
                    if let Some(s) = slot {
                        self.list_backups(&s);
                    }
                }
                Message::Crashes(r) => {
                    self.crashes_busy = false;
                    self.crashes = Some(r);
                }
            }
            ctx.request_repaint();
        }
    }
}

/// Splits a byte stream into lines on \n and \r (progress lines are rewritten in place with \r).
pub fn split_lines(mut stream: Box<dyn Read + Send>, mut each: impl FnMut(String)) {
    let mut buf = Vec::new();
    let mut chunk = [0u8; 4096];
    let mut flush = |buf: &mut Vec<u8>| {
        if !buf.is_empty() {
            let text = String::from_utf8_lossy(buf).trim_end().to_string();
            buf.clear();
            if !text.trim().is_empty() {
                each(text);
            }
        }
    };
    while let Ok(n) = stream.read(&mut chunk) {
        if n == 0 {
            break;
        }
        for &b in &chunk[..n] {
            if b == b'\n' || b == b'\r' {
                flush(&mut buf);
            } else {
                buf.push(b);
            }
        }
    }
    flush(&mut buf);
}

/// Pulls progress out of runtime output: `  2600/26503 (718 s)`, `pipelines: 75 made`,
/// `... 27.2 fps over the last 300`.
fn parse_line(line: &str, tx: &Sender<Message>) {
    let t = line.trim();
    if let Some((done, total, secs)) = flow::parse_shader(t) {
        let _ = tx.send(Message::Shader { done, total, secs });
    }
    if let Some((done, left)) = flow::parse_pipelines_progress(t) {
        let _ = tx.send(Message::World { done, left });
    }
    if let Some(i) = t.find("pipelines:") {
        let n: String = t[i + 10..].trim_start().chars().take_while(|c| c.is_ascii_digit()).collect();
        if let Ok(n) = n.parse() {
            let _ = tx.send(Message::Pipelines(n));
        }
    }
    if let Some(f) = engine::parse_fps(t) {
        let _ = tx.send(Message::Fps(f));
    }
}

impl eframe::App for Launcher {
    fn clear_color(&self, _visuals: &egui::Visuals) -> [f32; 4] {
        // opaque: a transparent window showed whatever was behind it when frames were late
        // (another D3D12 app holding the GPU); BG_000
        [14. / 255., 15. / 255., 18. / 255., 1.]
    }

    fn update(&mut self, ctx: &egui::Context, _frame: &mut eframe::Frame) {
        self.pump(ctx);
        self.refresh_header(ctx);
        self.update_presence();
        self.draw(ctx);
        self.screenshot(ctx);
    }

    fn on_exit(&mut self, _gl: Option<&eframe::glow::Context>) {
        self.stop_run();
        std::thread::sleep(std::time::Duration::from_millis(250));
    }
}

fn icon() -> egui::IconData {
    match image::load_from_memory(include_bytes!("../assets/logo.png")).map(|i| i.to_rgba8()) {
        Ok(i) => egui::IconData { width: i.width(), height: i.height(), rgba: i.into_raw() },
        Err(_) => egui::IconData::default(),
    }
}

fn main() -> eframe::Result {
    // A staged update (or a rollback) is swapped in at start; the new launcher then starts instead.
    if let Some(exe) = update::apply_pending() {
        if Command::new(&exe).args(std::env::args().skip(1)).spawn().is_ok() {
            return Ok(());
        }
    }
    let options = eframe::NativeOptions {
        viewport: egui::ViewportBuilder::default()
            .with_inner_size([1180., 660.])
            .with_icon(icon())
            .with_resizable(false)
            .with_decorations(false)
            .with_transparent(false),
        ..Default::default()
    };
    eframe::run_native("ScarlettPort", options, Box::new(|cc| Ok(Box::new(Launcher::new(cc)))))
}

#[cfg(test)]
mod tests {
    use super::*;

    fn parse(s: &str) -> Vec<String> {
        let (tx, rx) = channel();
        parse_line(s, &tx);
        rx.try_iter()
            .map(|m| match m {
                Message::Shader { done, total, secs } => format!("shader {done}/{total} {secs}"),
                Message::Pipelines(n) => format!("pipes {n}"),
                Message::Fps(f) => format!("fps {f}"),
                _ => String::new(),
            })
            .collect()
    }

    #[test]
    fn parses_progress() {
        assert_eq!(parse("  2600/26503 (718 s)"), vec!["shader 2600/26503 718"]);
        assert_eq!(parse("pipelines: 75 made"), vec!["pipes 75"]);
        assert_eq!(parse("D3D12.X: 600 frames presented, 27.2 fps over the last 300"), vec!["fps 27.2"]);
        assert!(parse("hello world").is_empty());
    }
}
