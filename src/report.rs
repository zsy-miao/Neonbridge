//! Copied from ScarlettPort (ZigZagBoi1/ScarlettPort, launcher/src/report.rs at 925f90a, GPL-3.0) with credit;
//! REPO_URL is Neonbridge's and parse_fps comes from its engine.rs.
#![allow(dead_code)]
//! One-click hardware report: GPU and driver, CPU, RAM, Windows build, runtime version, the last
//! run's frame-rate lines and the last 200 log lines (user name and paths scrubbed), opened as a
//! prefilled "Hardware report" issue (issue-template URL parameters, no token).
const REPO_URL: &str = "https://github.com/DesignWithTommy/Neonbridge";

#[derive(Default, Debug, Clone)]
pub struct System {
    pub gpu: String,
    pub driver: String,
    pub cpu: String,
    pub ram_gb: u64,
    pub os: String,
}

#[cfg(windows)]
fn reg_str(root: windows_sys::Win32::System::Registry::HKEY, key: &str, value: &str) -> Option<String> {
    use windows_sys::Win32::System::Registry::{RegGetValueW, RRF_RT_REG_DWORD, RRF_RT_REG_SZ};
    let wide = |s: &str| s.encode_utf16().chain([0]).collect::<Vec<u16>>();
    let (k, v) = (wide(key), wide(value));
    let mut buf = vec![0u16; 512];
    let mut size = (buf.len() * 2) as u32;
    // SAFETY: NUL-terminated names and a buffer of `size` bytes.
    let r = unsafe { RegGetValueW(root, k.as_ptr(), v.as_ptr(), RRF_RT_REG_SZ, std::ptr::null_mut(), buf.as_mut_ptr().cast(), &mut size) };
    if r == 0 {
        let n = (size as usize / 2).saturating_sub(1);
        return Some(String::from_utf16_lossy(&buf[..n]).trim().to_string());
    }
    let mut d = 0u32;
    let mut size = 4u32;
    // SAFETY: as above, a DWORD.
    let r = unsafe { RegGetValueW(root, k.as_ptr(), v.as_ptr(), RRF_RT_REG_DWORD, std::ptr::null_mut(), (&raw mut d).cast(), &mut size) };
    (r == 0).then(|| d.to_string())
}

/// Reads the machine's details from the registry and the memory status.
#[cfg(windows)]
pub fn system() -> System {
    use windows_sys::Win32::System::Registry::HKEY_LOCAL_MACHINE as HKLM;
    let mut s = System::default();
    // Display adapters: the first one that is not Microsoft's software adapter.
    for i in 0..10 {
        let key = format!(r"SYSTEM\CurrentControlSet\Control\Class\{{4d36e968-e325-11ce-bfc1-08002be10318}}\{i:04}");
        let Some(name) = reg_str(HKLM, &key, "DriverDesc") else { continue };
        if name.contains("Microsoft") || name.contains("Virtual") {
            continue;
        }
        s.driver = reg_str(HKLM, &key, "DriverVersion").unwrap_or_default();
        s.gpu = name;
        break;
    }
    s.cpu = reg_str(HKLM, r"HARDWARE\DESCRIPTION\System\CentralProcessor\0", "ProcessorNameString").unwrap_or_default();
    let cv = r"SOFTWARE\Microsoft\Windows NT\CurrentVersion";
    let build = reg_str(HKLM, cv, "CurrentBuild").unwrap_or_default();
    let ubr = reg_str(HKLM, cv, "UBR").map(|u| format!(".{u}")).unwrap_or_default();
    let name = if build.parse::<u32>().is_ok_and(|b| b >= 22000) { "Windows 11" } else { "Windows 10" };
    s.os = format!("{name} {} (build {build}{ubr})", reg_str(HKLM, cv, "DisplayVersion").unwrap_or_default());
    {
        use windows_sys::Win32::System::SystemInformation::{GlobalMemoryStatusEx, MEMORYSTATUSEX};
        let mut m: MEMORYSTATUSEX = unsafe { std::mem::zeroed() };
        m.dwLength = std::mem::size_of::<MEMORYSTATUSEX>() as u32;
        // SAFETY: a correctly sized, zeroed MEMORYSTATUSEX.
        if unsafe { GlobalMemoryStatusEx(&mut m) } != 0 {
            s.ram_gb = (m.ullTotalPhys + (512 << 20)) >> 30;
        }
    }
    s
}

#[cfg(not(windows))]
pub fn system() -> System {
    System::default()
}

/// Replaces the user name, the profile folder and the given folders (game, cache) with placeholders.
pub fn scrub(text: &str, folders: &[(&str, &str)]) -> String {
    let mut out = text.to_string();
    let mut subs: Vec<(String, String)> = folders.iter().filter(|(p, _)| p.len() > 3).map(|(p, r)| (p.to_string(), r.to_string())).collect();
    if let Some(home) = std::env::var_os("USERPROFILE") {
        subs.push((home.to_string_lossy().into_owned(), "%USERPROFILE%".into()));
    }
    for (from, to) in &subs {
        out = replace_ci(&out, from, to);
        out = replace_ci(&out, &from.replace('\\', "/"), to);
    }
    if let Some(user) = std::env::var_os("USERNAME").map(|u| u.to_string_lossy().into_owned()).filter(|u| u.len() >= 2) {
        out = replace_ci(&out, &user, "<user>");
    }
    out
}

fn replace_ci(text: &str, from: &str, to: &str) -> String {
    if from.is_empty() {
        return text.to_string();
    }
    let lower = text.to_lowercase();
    let needle = from.to_lowercase();
    if lower.len() != text.len() {
        return text.replace(from, to);
    }
    let mut out = String::with_capacity(text.len());
    let mut i = 0;
    while let Some(j) = lower[i..].find(&needle) {
        out.push_str(&text[i..i + j]);
        out.push_str(to);
        i += j + needle.len();
    }
    out.push_str(&text[i..]);
    out
}

/// The last run's frame-rate summary from the log: the fps lines and any p99 / freeze lines.
pub fn fps_summary(logs: &[String]) -> String {
    let fps: Vec<f32> = logs.iter().filter_map(|l| parse_fps(l)).filter(|f| *f > 0.).collect();
    let mut out = Vec::new();
    if !fps.is_empty() {
        let avg = fps.iter().sum::<f32>() / fps.len() as f32;
        let min = fps.iter().cloned().fold(f32::MAX, f32::min);
        let max = fps.iter().cloned().fold(0., f32::max);
        out.push(format!("{} fps samples: average {avg:.1}, min {min:.1}, max {max:.1}, last {:.1}", fps.len(), fps[fps.len() - 1]));
    }
    for l in logs.iter().filter(|l| {
        let t = l.to_ascii_lowercase();
        t.contains("p99") || t.contains("freeze") || t.contains("hitch")
    }).rev().take(8).collect::<Vec<_>>().into_iter().rev()
    {
        out.push(l.clone());
    }
    if out.is_empty() { "No frame-rate lines in this session's log.".into() } else { out.join("\n") }
}

pub fn percent_encode(s: &str) -> String {
    let mut out = String::with_capacity(s.len() * 3);
    for b in s.bytes() {
        match b {
            b'A'..=b'Z' | b'a'..=b'z' | b'0'..=b'9' | b'-' | b'_' | b'.' | b'~' => out.push(b as char),
            _ => out.push_str(&format!("%{b:02X}")),
        }
    }
    out
}

/// Longest issue URL browsers and GitHub accept reliably.
pub const MAX_URL: usize = 7500;

/// The prefilled "Hardware report" issue URL for these fields (template ids of
/// `.github/ISSUE_TEMPLATE/hardware_report.yml`).
pub fn issue_url(fields: &[(&str, String)]) -> String {
    let mut url = format!("{REPO_URL}/issues/new?template=hardware_report.yml&title={}", percent_encode("[Hardware] "));
    for (k, v) in fields {
        url.push('&');
        url.push_str(k);
        url.push('=');
        url.push_str(&percent_encode(v));
    }
    url
}

/// The whole report as plain text (for the clipboard when it is too long for a URL).
pub fn as_text(fields: &[(&str, String)]) -> String {
    fields.iter().map(|(k, v)| format!("### {k}\n\n{v}\n")).collect::<Vec<_>>().join("\n")
}


pub fn parse_fps(line: &str) -> Option<f32> {
    let t = line.to_ascii_lowercase();
    let i = t.find("fps")?;
    let after = &t[i + 3..];
    if after.starts_with([':', '=']) {
        let n: String = after[1..].trim_start().chars().take_while(|c| c.is_ascii_digit() || *c == '.').collect();
        return n.parse().ok();
    }
    let before = t[..i].trim_end();
    let n: String = before.chars().rev().take_while(|c| c.is_ascii_digit() || *c == '.').collect::<Vec<_>>().into_iter().rev().collect();
    n.parse().ok()
}

/// Neonbridge: `neonbridge --report [--runtime <name>]`: the machine, the runtime and its
/// last run (frame-rate summary and the last 200 log lines, user name and folders scrubbed),
/// opened as a prefilled GitHub issue in the browser. Nothing is sent until the user submits it.
pub fn open(runtime: &str, game: Option<&std::path::Path>) {
    let s = system();
    let log_path = crate::runtimes::cache_of(runtime).join("last-run.log");
    let lines: Vec<String> = std::fs::read_to_string(&log_path).unwrap_or_default().lines().map(str::to_owned).collect();
    let tail = lines[lines.len().saturating_sub(200)..].join("\n");
    let game_s = game.map(|g| g.display().to_string()).unwrap_or_default();
    let pack = crate::runtimes::pack_dir().display().to_string();
    let folders = [(game_s.as_str(), "<game>"), (pack.as_str(), "<6ixpack>")];
    let mut fields = vec![
        ("gpu", format!("{} (driver {})", s.gpu, s.driver)),
        ("cpu", s.cpu.clone()),
        ("ram", format!("{} GB", s.ram_gb)),
        ("os", s.os.clone()),
        ("runtime", format!("{runtime}: {}", crate::runtimes::about(runtime))),
        ("fps", fps_summary(&lines)),
        ("log", scrub(&tail, &folders)),
    ];
    let mut url = issue_url(&fields);
    // Fewer log lines until the link fits; the full report still goes to report.txt.
    for n in [100, 50, 25, 12] {
        if url.len() <= MAX_URL {
            break;
        }
        fields.last_mut().unwrap().1 = scrub(&lines[lines.len().saturating_sub(n)..].join("\n"), &folders);
        url = issue_url(&fields);
    }
    if url.len() > MAX_URL {
        let file = crate::runtimes::pack_dir().join("report.txt");
        let _ = std::fs::write(&file, scrub(&as_text(&fields), &folders));
        fields.last_mut().unwrap().1 = format!("(too long for the link: paste the contents of {} here)", scrub(&file.display().to_string(), &folders));
        url = issue_url(&fields);
        println!("[report] full report written to {}", file.display());
    }
    if std::env::var("NEONBRIDGE_REPORT_PRINT").is_ok_and(|v| v.trim() == "1") {
        println!("{url}");
        return;
    }
    println!("[report] opening a prefilled issue in the browser (nothing is sent until you submit it)");
    let _ = std::process::Command::new("rundll32").args(["url.dll,FileProtocolHandler", &url]).spawn();
}
