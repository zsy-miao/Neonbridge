//! Play presets: Stable (the verified C1 setup, the same environment as play.ps1 without -Old),
//! Experimental (C1 plus the fx-cand2 switches judged safe) and Custom (the launcher's own
//! World options). Pure functions, unit tested.
use std::path::Path;

pub const STABLE: &str = "stable";
pub const EXPERIMENTAL: &str = "experimental";
pub const CUSTOM: &str = "custom";
pub const ALL: [&str; 3] = [STABLE, EXPERIMENTAL, CUSTOM];

/// The runtime build the bundled runtime folder is made from (see the repository's runtime/).
pub const BUNDLED_BUILD: &str = "fx-cand bce072d";
/// The build the Experimental switches were merged into (fx-cand2); older builds ignore them.
pub const EXPERIMENTAL_BUILD: &str = "fx-cand2";

/// C1, promoted 2026-11-25: the precompiled graphics pipelines and the verified speed switches.
pub const C1: [&str; 9] = [
    "VICTORY_PRECOMPILED_GRAPHICS",
    "VICTORY_READFIX",
    "VICTORY_COPY_BACK_ONLY_READBACK",
    "VICTORY_NARROW_ROOT",
    "VICTORY_NARROW_VIEWS",
    "VICTORY_NARROW_BVH",
    "VICTORY_PREP_BARRIERS",
    "VICTORY_PREP_TABLES",
    "VICTORY_PREP_REUSE",
];

/// Variables play.ps1 clears before a plain run (so nothing left in the launcher's own
/// environment changes the run).
pub const CLEARED: [&str; 8] = [
    "VICTORY_TEXTURES",
    "VICTORY_HDR",
    "VICTORY_PROFILE",
    "VICTORY_FAST_BOOT",
    "VICTORY_CAPTURE",
    "VICTORY_CAPTURE_EVERY",
    "VICTORY_PRESS_FILE",
    "VICTORY_RENDER_SCALE",
];

/// fx-cand2 switches Experimental adds to C1: the GPU step of the A/B plan (C1 is GPU-bound)
/// and the world-entry crash guard (2 clean runs against 3 crashes upstream).
pub const EXPERIMENTAL_ON: [(&str, &str); 3] = [("VICTORY_INDIRECT_WINDOW", "1"), ("VICTORY_INDIRECT_KEEP", "1"), ("VICTORY_TABLED_SKIP_FRAMES", "2")];

/// fx-cand2 switches left out of Experimental, and why.
pub const EXPERIMENTAL_OFF: [(&str, &str); 6] = [
    ("VICTORY_INDIRECT_DIRECT", "can hang the GPU"),
    ("VICTORY_SHADER_OPT", "needs its own cache per value"),
    ("VICTORY_WIDE_PAGE_SPLIT", "changes the translation, own cache"),
    ("VICTORY_SAFE_COPY / SKIP_SAME / WRITE_BESIDE / FILL_FREE", "not measured yet, one at a time"),
    ("VICTORY_COPY_FETCH / QUEUE2", "A/B against each other, not stacked"),
    ("VICTORY_EARLY_PIPELINES=0", "diagnostic only"),
];

pub fn label(preset: &str) -> &'static str {
    match preset {
        EXPERIMENTAL => "Experimental",
        CUSTOM => "Custom",
        _ => "Stable",
    }
}

pub fn blurb(preset: &str) -> &'static str {
    match preset {
        EXPERIMENTAL => "C1 plus the safe fx-cand2 switches (indirect window and keep, tabled skip frames). Own cache.",
        CUSTOM => "Your World options from Settings (ray tracing, stand-in light, debug view).",
        _ => "The verified C1 setup: Series S profile, the game's upscaler, precise syncs, the speed switches.",
    }
}

/// Exactly play.ps1's environment for a plain run (no -Old and no other switch): game, cache,
/// slot, upscaler, watchdog, relay, pipeline wait, log level, precise syncs, then C1.
pub fn stable_env(game: &Path, victory_cache: &Path) -> Vec<(&'static str, String)> {
    let mut v = vec![
        ("VICTORY_GAME_DIR", game.display().to_string()),
        ("VICTORY_CACHE_DIR", victory_cache.display().to_string()),
        ("VICTORY_SAVE_SLOT", "default".to_string()),
        ("VICTORY_OWN_UPSCALER", "1".to_string()),
        ("VICTORY_NO_WATCHDOG", "1".to_string()),
        ("VICTORY_RELAY", "2".to_string()),
        ("VICTORY_PIPELINE_WAIT_MS", "0".to_string()),
        ("VICTORY_LOG", "warn".to_string()),
        ("VICTORY_SYNC_PRECISE", "1".to_string()),
        ("VICTORY_READ_SETTLE_PRECISE", "1".to_string()),
    ];
    v.extend(C1.iter().map(|k| (*k, "1".to_string())));
    v
}

pub fn experimental_env(game: &Path, victory_cache: &Path) -> Vec<(&'static str, String)> {
    let mut v = stable_env(game, victory_cache);
    v.extend(EXPERIMENTAL_ON.iter().map(|(k, x)| (*k, x.to_string())));
    v
}

/// The cache a preset uses: Experimental gets its own folder (fx-cand2's cache key includes
/// every switch the translator reads).
pub fn cache_for(preset: &str, base: &Path) -> std::path::PathBuf {
    if preset == EXPERIMENTAL {
        let name = base.file_name().map(|n| format!("{}-experimental", n.to_string_lossy())).unwrap_or_else(|| "experimental".into());
        base.with_file_name(name)
    } else {
        base.to_path_buf()
    }
}

/// Sets `k=v`, replacing an earlier value of the same variable.
pub fn set(env: &mut Vec<(&'static str, String)>, k: &'static str, v: String) {
    env.retain(|(n, _)| *n != k);
    env.push((k, v));
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::path::PathBuf;

    fn get(v: &[(&'static str, String)], k: &str) -> Option<String> {
        v.iter().find(|(n, _)| *n == k).map(|(_, x)| x.clone())
    }

    #[test]
    fn stable_matches_play_ps1() {
        let e = stable_env(Path::new("D:/dump"), Path::new("F:/c"));
        assert_eq!(e.len(), 10 + 9);
        for (k, v) in [
            ("VICTORY_SAVE_SLOT", "default"),
            ("VICTORY_OWN_UPSCALER", "1"),
            ("VICTORY_NO_WATCHDOG", "1"),
            ("VICTORY_RELAY", "2"),
            ("VICTORY_PIPELINE_WAIT_MS", "0"),
            ("VICTORY_LOG", "warn"),
            ("VICTORY_SYNC_PRECISE", "1"),
            ("VICTORY_READ_SETTLE_PRECISE", "1"),
        ] {
            assert_eq!(get(&e, k).as_deref(), Some(v), "{k}");
        }
        for k in C1 {
            assert_eq!(get(&e, k).as_deref(), Some("1"), "{k}");
        }
        for k in CLEARED {
            assert!(get(&e, k).is_none());
        }
    }

    #[test]
    fn experimental_adds_only_safe_switches() {
        let e = experimental_env(Path::new("D:/dump"), Path::new("F:/c"));
        assert_eq!(get(&e, "VICTORY_INDIRECT_KEEP").as_deref(), Some("1"));
        assert_eq!(get(&e, "VICTORY_TABLED_SKIP_FRAMES").as_deref(), Some("2"));
        assert!(get(&e, "VICTORY_INDIRECT_DIRECT").is_none());
        assert!(get(&e, "VICTORY_SHADER_OPT").is_none());
        assert_eq!(cache_for(EXPERIMENTAL, Path::new("F:/S/cache-fast")), PathBuf::from("F:/S/cache-fast-experimental"));
        assert_eq!(cache_for(STABLE, Path::new("F:/S/cache-fast")), PathBuf::from("F:/S/cache-fast"));
    }

    #[test]
    fn set_replaces() {
        let mut e = vec![("A", "1".to_string())];
        set(&mut e, "A", "2".into());
        assert_eq!(e, vec![("A", "2".to_string())]);
    }
}
