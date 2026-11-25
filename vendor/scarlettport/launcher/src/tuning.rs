//! Runtime features added on 2026-11-24/25 (full textures, HDR, PlayStation pads, rumble, keyboard
//! hints, frame cap, upscaler and frame generation): the launcher's settings for them, the
//! environment variables they set and the `victory.toml` keys they write. A setting at its
//! default sets nothing, so the Stable preset stays exactly play.ps1's environment.
use serde::{Deserialize, Serialize};
use std::path::Path;

#[derive(Serialize, Deserialize, Clone, PartialEq, Debug)]
#[serde(default)]
pub struct Tuning {
    pub full_textures: bool,
    pub hdr: bool,
    pub pad_ps: bool,
    pub rumble: bool,
    pub rumble_strength: f32,
    /// "off", "auto" or "always".
    pub kbm_hints: String,
    /// 0 = no cap (the runtime's own pacing), else 30 or 60.
    pub frame_cap: u32,
    /// "" = the runtime's default, else "title", "stretch" or "fsr".
    pub upscaler: String,
    /// native | quality | balanced | performance | ultra
    pub quality: String,
    pub sharpness: f32,
    pub framegen: bool,
    /// Adds `victory_gpu::host=info` to the log level so the run log has frame times
    /// (the performance readout). Off keeps the log at play.ps1's `warn`.
    pub perf_log: bool,
}

impl Default for Tuning {
    fn default() -> Self {
        Tuning {
            full_textures: false,
            hdr: false,
            pad_ps: false,
            rumble: true,
            rumble_strength: 1.0,
            kbm_hints: "auto".into(),
            frame_cap: 0,
            upscaler: String::new(),
            quality: "quality".into(),
            sharpness: 0.2,
            framegen: false,
            perf_log: false,
        }
    }
}

pub const HINTS: [&str; 3] = ["off", "auto", "always"];
pub const UPSCALERS: [&str; 4] = ["", "title", "stretch", "fsr"];
pub const QUALITIES: [&str; 5] = ["native", "quality", "balanced", "performance", "ultra"];
pub const CAPS: [u32; 3] = [0, 30, 60];

/// FidelityFX DLLs the runtime loads for FSR (either the loader or an MIT-built API DLL).
pub const FSR_DLLS: [&str; 2] = ["amd_fidelityfx_loader_dx12.dll", "amd_fidelityfx_dx12.dll"];

pub fn fsr_available(runtime_dir: &Path) -> bool {
    FSR_DLLS.iter().any(|d| runtime_dir.join(d).is_file())
}

fn num(x: f32) -> String {
    let s = format!("{x:.2}");
    let s = s.trim_end_matches('0').trim_end_matches('.');
    if s.contains('.') { s.to_string() } else { format!("{s}.0") }
}

impl Tuning {
    /// Environment variables for the settings that differ from the default. `fsr` says whether
    /// the FidelityFX DLL is present (FSR and frame generation are not asked for without it).
    pub fn env(&self, fsr: bool) -> Vec<(&'static str, String)> {
        let d = Tuning::default();
        let mut v = Vec::new();
        if self.full_textures {
            v.push(("VICTORY_TEXTURES", "full".to_string()));
        }
        if self.hdr {
            v.push(("VICTORY_HDR", "1".to_string()));
        }
        if self.pad_ps {
            v.push(("VICTORY_PAD_PS", "1".to_string()));
        }
        if !self.rumble {
            v.push(("VICTORY_RUMBLE", "0".to_string()));
        } else if (self.rumble_strength - d.rumble_strength).abs() > 0.001 {
            v.push(("VICTORY_RUMBLE_STRENGTH", num(self.rumble_strength)));
        }
        if self.kbm_hints != d.kbm_hints {
            v.push(("VICTORY_KBM_HINTS", self.kbm_hints.clone()));
        }
        if self.frame_cap > 0 {
            v.push(("VICTORY_PACE", "1".to_string()));
            v.push(("VICTORY_PACE_FPS", self.frame_cap.to_string()));
        }
        if !self.upscaler.is_empty() && (self.upscaler != "fsr" || fsr) {
            v.push(("VICTORY_UPSCALER", self.upscaler.clone()));
            if self.upscaler == "fsr" {
                v.push(("VICTORY_UPSCALE_QUALITY", self.quality.clone()));
                v.push(("VICTORY_UPSCALE_SHARPNESS", num(self.sharpness)));
                v.push(("VICTORY_FRAMEGEN", if self.framegen { "1" } else { "0" }.to_string()));
            }
        }
        v
    }

    /// `(section, key, TOML value)` for `victory.toml`; every managed key is written, so a
    /// setting turned back to its default is written back too.
    pub fn toml_keys(&self, fsr: bool) -> Vec<(&'static str, &'static str, String)> {
        let q = |s: &str| format!("\"{s}\"");
        let mut v = vec![
            ("display", "hdr", self.hdr.to_string()),
            ("game", "rumble", self.rumble.to_string()),
            ("game", "rumble_strength", num(self.rumble_strength)),
            ("game", "kbm_hints", q(&self.kbm_hints)),
            ("pace", "pace", (self.frame_cap > 0).to_string()),
        ];
        if self.frame_cap > 0 {
            v.push(("pace", "fps", self.frame_cap.to_string()));
        }
        if !self.upscaler.is_empty() && (self.upscaler != "fsr" || fsr) {
            v.push(("upscale", "upscaler", q(&self.upscaler)));
            v.push(("upscale", "quality", q(&self.quality)));
            v.push(("upscale", "sharpness", num(self.sharpness)));
            v.push(("upscale", "framegen", (self.framegen && fsr).to_string()));
        }
        v
    }
}

/// Sets `keys` in TOML `text`, keeping every other line (comments, `[controls]`, what the F1
/// overlay saved). A key is replaced in its section, or added at the section's end; a missing
/// section is appended.
pub fn merge_toml(text: &str, keys: &[(&str, &str, String)]) -> String {
    let mut lines: Vec<String> = text.lines().map(str::to_string).collect();
    let header = |l: &str| {
        let t = l.trim();
        (t.starts_with('[') && !t.starts_with("[[")).then(|| t.trim_start_matches('[').split(']').next().unwrap_or("").trim().to_string())
    };
    for (section, key, value) in keys {
        let start = lines.iter().position(|l| header(l).as_deref() == Some(*section));
        let line = format!("{key} = {value}");
        match start {
            Some(s) => {
                let end = lines[s + 1..].iter().position(|l| header(l).is_some()).map_or(lines.len(), |e| s + 1 + e);
                let hit = (s + 1..end).find(|&i| {
                    let t = lines[i].trim_start();
                    t.split('=').next().is_some_and(|k| k.trim() == *key) && t.contains('=')
                });
                match hit {
                    Some(i) => lines[i] = line,
                    None => {
                        // after the section's last non-blank line
                        let mut at = end;
                        while at > s + 1 && lines[at - 1].trim().is_empty() {
                            at -= 1;
                        }
                        lines.insert(at, line);
                    }
                }
            }
            None => {
                if lines.last().is_some_and(|l| !l.trim().is_empty()) {
                    lines.push(String::new());
                }
                lines.push(format!("[{section}]"));
                lines.push(line);
            }
        }
    }
    let mut out = lines.join("\n");
    out.push('\n');
    out
}

/// Writes the managed keys into `path` (created when missing).
pub fn write_toml(path: &Path, keys: &[(&str, &str, String)]) -> std::io::Result<()> {
    let old = std::fs::read_to_string(path).unwrap_or_else(|_| "# Written by the ScarlettPort launcher (Settings, Features). Other lines are kept.\n".into());
    if let Some(d) = path.parent() {
        std::fs::create_dir_all(d)?;
    }
    std::fs::write(path, merge_toml(&old, keys))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn defaults_set_nothing() {
        assert!(Tuning::default().env(true).is_empty());
    }

    #[test]
    fn env_mapping() {
        let t = Tuning { full_textures: true, hdr: true, pad_ps: true, rumble_strength: 1.5, kbm_hints: "off".into(), frame_cap: 30, upscaler: "fsr".into(), framegen: true, ..Tuning::default() };
        let e = t.env(true);
        let get = |k: &str| e.iter().find(|(n, _)| *n == k).map(|(_, v)| v.as_str());
        assert_eq!(get("VICTORY_TEXTURES"), Some("full"));
        assert_eq!(get("VICTORY_HDR"), Some("1"));
        assert_eq!(get("VICTORY_PAD_PS"), Some("1"));
        assert_eq!(get("VICTORY_RUMBLE_STRENGTH"), Some("1.5"));
        assert_eq!(get("VICTORY_KBM_HINTS"), Some("off"));
        assert_eq!(get("VICTORY_PACE_FPS"), Some("30"));
        assert_eq!(get("VICTORY_UPSCALER"), Some("fsr"));
        assert_eq!(get("VICTORY_FRAMEGEN"), Some("1"));
        // without the DLL, FSR is not asked for
        assert!(t.env(false).iter().all(|(k, _)| !k.starts_with("VICTORY_UPSCAL") && *k != "VICTORY_FRAMEGEN"));
        let off = Tuning { rumble: false, ..Tuning::default() };
        assert_eq!(off.env(false), vec![("VICTORY_RUMBLE", "0".to_string())]);
    }

    #[test]
    fn toml_merge_keeps_other_lines() {
        let text = "[paths]\ngame_dir = 'D:\\g'\n\n[controls]\nmouse_sensitivity = 2.0\n\n[game]\nrumble = true\n";
        let keys = Tuning { rumble: false, frame_cap: 60, ..Tuning::default() }.toml_keys(false);
        let out = merge_toml(text, &keys);
        assert!(out.contains("game_dir = 'D:\\g'"));
        assert!(out.contains("mouse_sensitivity = 2.0"));
        assert!(out.contains("rumble = false"));
        assert!(!out.contains("rumble = true"));
        assert!(out.contains("[pace]\npace = true\nfps = 60"));
        assert!(out.contains("[display]\nhdr = false"));
        // idempotent
        assert_eq!(merge_toml(&out, &keys), out);
    }
}
