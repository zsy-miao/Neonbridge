//! Lighter graphics settings before each run (LMIII progress notes §27, the
//! idea of their apply-settings.ps1). The title writes its own settings back to
//! `settings.xml` in its persistent local storage when it exits, which puts
//! 3840x2160, ray-traced dynamic shadows, motion blur, SSR water reflections
//! and particle shadows back on. Each of those multiplies the pipelines the
//! title creates and the video memory it uses. Before each run the launcher
//! sets them to 1920x1080 and off. NEONBRIDGE_SAFE_SETTINGS=0 keeps the
//! user's own settings.
use std::fs;
use std::path::{Path, PathBuf};

/// (element, forced value). `resolutionFullscreen` is handled on its own.
const FORCED: &[(&str, &str)] = &[
    ("rtDynamicShadows", "false"),
    ("motionBlur", "false"),
    ("waterReflectionSSR", "false"),
    ("shadowParticleShadows", "false"),
    ("ssroSampleCount", "1"),
];

/// The settings file the title reads, under the folder xgameruntime gives as
/// its persistent local storage (`%LOCALAPPDATA%\TuxBox\PersistentLocalStorage\<title id>`).
fn settings_path(game: &Path) -> Option<PathBuf> {
    let config = fs::read_to_string(game.join("MicrosoftGame.Config")).ok()?;
    let (_, rest) = config.split_once("<TitleId>")?;
    let (id, _) = rest.split_once('<')?;
    let id = u32::from_str_radix(id.trim(), 16).ok()?;
    let base = std::env::var("LOCALAPPDATA").ok()?;
    Some(Path::new(&base).join(format!(r"TuxBox\PersistentLocalStorage\{id:08X}\settings\settings.xml")))
}

/// Replaces the value of the first `<name value="..."` at or after `from`;
/// returns the old value when it differed.
fn set(text: &mut String, from: usize, name: &str, value: &str) -> Option<String> {
    let tag = format!("<{name} value=\"");
    let start = text[from..].find(&tag)? + from + tag.len();
    let end = start + text[start..].find('"')?;
    let old = text[start..end].to_owned();
    if old == value {
        return None;
    }
    text.replace_range(start..end, value);
    Some(old)
}

/// Forces the lighter settings; prints each value it changed.
pub fn apply(game: &Path) {
    if std::env::var("NEONBRIDGE_SAFE_SETTINGS").as_deref() == Ok("0") {
        println!("[launch] settings: left as the title saved them (NEONBRIDGE_SAFE_SETTINGS=0)");
        return;
    }
    let Some(path) = settings_path(game) else { return };
    let Ok(mut text) = fs::read_to_string(&path) else {
        println!("[launch] settings: {} not there yet; the title writes it on its first exit", path.display());
        return;
    };
    let mut changed = Vec::new();
    if let Some(at) = text.find("<resolutionFullscreen>") {
        for (name, value) in [("width", "1920"), ("height", "1080")] {
            if let Some(old) = set(&mut text, at, name, value) {
                changed.push(format!("fullscreen {name} {old} -> {value}"));
            }
        }
    }
    for (name, value) in FORCED {
        if let Some(old) = set(&mut text, 0, name, value) {
            changed.push(format!("{name} {old} -> {value}"));
        }
    }
    if changed.is_empty() {
        println!("[launch] settings: already 1080p with RT shadows, motion blur, SSR water and particle shadows off (NEONBRIDGE_SAFE_SETTINGS=0 keeps the title's)");
        return;
    }
    match fs::write(&path, &text) {
        Ok(()) => println!("[launch] settings: {} (NEONBRIDGE_SAFE_SETTINGS=0 keeps the title's)", changed.join(", ")),
        Err(e) => println!("[launch] settings: {} not written: {e}", path.display()),
    }
}

#[cfg(test)]
mod tests {
    #[test]
    fn sets_only_differing_values() {
        let mut text = String::from(r#"<a value="1" /><resolutionFullscreen><width value="3840" /><height value="2160" />"#);
        assert_eq!(super::set(&mut text, 0, "a", "1"), None);
        let at = text.find("<resolutionFullscreen>").unwrap();
        assert_eq!(super::set(&mut text, at, "width", "1920").as_deref(), Some("3840"));
        assert!(text.contains(r#"<width value="1920" />"#));
    }
}
