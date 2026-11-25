//! One-click hardware report: GPU and driver, CPU, RAM, Windows build, runtime version, the last
//! run's frame-rate lines and the last 200 log lines (user name and paths scrubbed), opened as a
//! prefilled "Hardware report" issue (issue-template URL parameters, no token).
use crate::REPO_URL;

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
    let fps: Vec<f32> = logs.iter().filter_map(|l| crate::engine::parse_fps(l)).filter(|f| *f > 0.).collect();
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

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn scrubs_names_and_paths() {
        let s = scrub(r"loading D:\Dumps\MyGame\x.rpf from E:\Cache\victory", &[(r"D:\Dumps\MyGame", "<game>"), (r"E:\Cache", "<cache>")]);
        assert_eq!(s, r"loading <game>\x.rpf from <cache>\victory");
        assert_eq!(replace_ci("Path C:\\USERS\\Bob\\x", "c:\\users\\bob", "~"), "Path ~\\x");
    }

    #[test]
    fn url_and_summary() {
        let u = issue_url(&[("gpu", "RTX 3060 & co".into())]);
        assert!(u.contains("template=hardware_report.yml"));
        assert!(u.ends_with("gpu=RTX%203060%20%26%20co"));
        let logs = vec!["D3D12.X: 600 frames presented, 4.0 fps over the last 300".to_string(), "frame p99 410 ms".into(), "x".into()];
        let s = fps_summary(&logs);
        assert!(s.contains("average 4.0") && s.contains("p99"));
    }
}
