//! Engine choice and the mapping from launcher settings to the engines' command lines and
//! environment, plus parsing of their shader-build output (pure functions, unit tested).
use crate::Config;
use std::path::{Path, PathBuf};

pub const WORLD: &str = "world";
pub const CLASSIC: &str = "classic";

pub const WORLD_LABEL: &str = "World (furthest: 3D world + gameplay, slow)";
pub const CLASSIC_LABEL: &str = "Classic (menus at 30 fps)";

/// Distinct shader programs `victory warm` translates for the supported title (measured).
pub const WARM_TOTAL: u64 = 25_889;
/// Graphics records in the title's precompiled pipeline cache (measured; refined as chunks finish).
pub const GRAPHICS_TOTAL: u64 = 25_471;
/// Records per `victory precompile-graphics` run: each run saves the pipeline library when it
/// ends, so a stop loses at most one chunk.
pub const CHUNK: u64 = 1000;
/// The runtime vendored here decodes rectangle-list pipelines right (the tile-grid fix), so play
/// runs set VICTORY_PRECOMPILED_GRAPHICS=1 (as play-victory.ps1 does) and the graphics-pipeline
/// phase of the shader build runs. Both follow this one constant.
pub const PRECOMPILED_GRAPHICS: bool = true;

/// The other verified speed settings (play-victory.ps1 sets the same ones, besides the precise
/// syncs and settles): the read-back fix, copy-back only for read-backs, narrow root signatures,
/// views and BVH, and the prepared barriers, tables and reuse.
pub const SPEED_SETTINGS: [&str; 8] = [
    "VICTORY_READFIX",
    "VICTORY_COPY_BACK_ONLY_READBACK",
    "VICTORY_NARROW_ROOT",
    "VICTORY_NARROW_VIEWS",
    "VICTORY_NARROW_BVH",
    "VICTORY_PREP_BARRIERS",
    "VICTORY_PREP_TABLES",
    "VICTORY_PREP_REUSE",
];

/// True when the config selects the World engine (anything else is Classic).
pub fn is_world(c: &Config) -> bool {
    c.engine == WORLD
}

/// The World engine's cache folder (translated shaders, pipeline library) inside the launcher's cache.
pub fn victory_cache(cache_dir: &str) -> PathBuf {
    Path::new(cache_dir.trim()).join("victory")
}

/// Arguments after the executable: `run "<game exe>" --execute [--console series-s]`.
pub fn world_args(c: &Config, game: &Path) -> Vec<String> {
    let mut a = vec!["run".to_string(), game.join(crate::GAME_EXE).display().to_string(), "--execute".to_string()];
    if c.console_s {
        a.push("--console".into());
        a.push("series-s".into());
    }
    a
}

/// `warm "<game folder>" --jobs N`: translate every shader into the cache and make the compute pipelines.
pub fn warm_args(game: &Path, jobs: usize) -> Vec<String> {
    vec!["warm".into(), game.display().to_string(), "--jobs".into(), jobs.max(1).to_string()]
}

/// `precompile-graphics "<shaders.rpf>" --jobs N --skip S --limit L`.
pub fn graphics_args(game: &Path, jobs: usize, skip: u64, limit: u64) -> Vec<String> {
    vec![
        "precompile-graphics".into(),
        game.join("shaders.rpf").display().to_string(),
        "--jobs".into(),
        jobs.max(1).to_string(),
        "--skip".into(),
        skip.to_string(),
        "--limit".into(),
        limit.to_string(),
    ]
}

/// Environment every World engine process gets: the game and the cache.
pub fn base_env(game: &Path, cache_dir: &str) -> Vec<(&'static str, String)> {
    vec![("VICTORY_GAME_DIR", game.display().to_string()), ("VICTORY_CACHE_DIR", victory_cache(cache_dir).display().to_string())]
}

/// Environment for a World play run (display settings are added separately). The defaults are
/// play-victory.ps1's.
pub fn world_env(c: &Config, game: &Path, cache_dir: &str) -> Vec<(&'static str, String)> {
    let flag = |b: bool| if b { "1" } else { "0" }.to_string();
    let mut v = base_env(game, cache_dir);
    v.extend([
        ("VICTORY_OWN_UPSCALER", "1".to_string()),
        ("VICTORY_NO_WATCHDOG", "1".to_string()),
        ("VICTORY_RELAY", "2".to_string()),
        ("VICTORY_SYNC_PRECISE", "1".to_string()),
        ("VICTORY_READ_SETTLE_PRECISE", "1".to_string()),
        ("VICTORY_PIPELINE_WAIT_MS", "0".to_string()),
        ("VICTORY_RAYTRACING", flag(c.raytracing)),
        ("VICTORY_FALLBACK_LIGHT", flag(c.fallback_light)),
    ]);
    if PRECOMPILED_GRAPHICS {
        v.push(("VICTORY_PRECOMPILED_GRAPHICS", "1".to_string()));
    }
    v.extend(SPEED_SETTINGS.iter().map(|k| (*k, "1".to_string())));
    if c.debug_view {
        v.push(("VICTORY_DEBUG_PS", "1".to_string()));
        v.push(("VICTORY_SHOW_TARGET", "2560x1440-f28".to_string()));
    }
    if c.auto_menu {
        v.push(("VICTORY_AUTO_INPUT", "1".to_string()));
    }
    v
}

/// Parallel jobs for a build step: at most one per logical core but one (the PC stays usable), and
/// no more than free RAM allows at `per_job_mb` each. At least one.
pub fn jobs(cores: usize, free_mb: Option<u64>, per_job_mb: u64) -> usize {
    let by_cpu = cores.saturating_sub(1).max(1);
    let by_ram = free_mb.map_or(by_cpu, |f| (f.saturating_sub(2048) / per_job_mb.max(1)) as usize);
    by_cpu.min(by_ram).max(1)
}

/// `victory warm` progress: `   123 s  4000 shaders` -> 4000.
pub fn parse_warm(line: &str) -> Option<u64> {
    let t = line.trim();
    let rest = t.strip_suffix(" shaders")?;
    rest.rsplit(' ').next()?.parse().ok()
}

/// `victory warm`'s summary: `warm  25889 translated, 0 refused; ...` -> (translated, refused).
pub fn parse_warm_end(line: &str) -> Option<(u64, u64)> {
    let t = line.trim().strip_prefix("warm")?.trim_start();
    let (a, rest) = t.split_once(" translated, ")?;
    let (b, _) = rest.split_once(" refused")?;
    Some((a.trim().parse().ok()?, b.trim().parse().ok()?))
}

/// `victory precompile-graphics` progress: `   40 s  500 records: 480 made, ...` -> 500.
pub fn parse_graphics(line: &str) -> Option<u64> {
    let t = line.trim();
    let (head, _) = t.split_once(" records:")?;
    if t.starts_with("precompile-graphics") {
        return None;
    }
    head.rsplit(' ').next()?.parse().ok()
}

/// `victory precompile-graphics`'s summary: `precompile-graphics  800 records: ...` -> 800.
pub fn parse_graphics_end(line: &str) -> Option<u64> {
    let t = line.trim().strip_prefix("precompile-graphics")?.trim_start();
    t.split_once(" records:")?.0.trim().parse().ok()
}

/// Frame rate from a log line: `12.5 fps`, `fps: 12.5` or `fps=12.5`. None without a rate.
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

#[cfg(test)]
mod tests {
    use super::*;

    fn get(v: &[(&'static str, String)], k: &str) -> Option<String> {
        v.iter().find(|(n, _)| *n == k).map(|(_, x)| x.clone())
    }

    #[test]
    fn default_is_world_with_series_s() {
        let c = Config::default();
        assert!(is_world(&c));
        assert!(c.console_s && !c.raytracing && c.fallback_light && !c.debug_view);
    }

    #[test]
    fn world_args_series_x_and_s() {
        let g = PathBuf::from("D:/game");
        let mut c = Config::default();
        c.console_s = false;
        let a = world_args(&c, &g);
        assert_eq!(a[0], "run");
        assert!(a[1].ends_with(crate::GAME_EXE));
        assert_eq!(a.len(), 3);
        c.console_s = true;
        assert_eq!(&world_args(&c, &g)[3..], ["--console", "series-s"]);
    }

    #[test]
    fn world_env_mapping() {
        let g = PathBuf::from("D:/game");
        let mut c = Config::default();
        let e = world_env(&c, &g, "E:/ScarlettCache");
        assert_eq!(get(&e, "VICTORY_CACHE_DIR").map(PathBuf::from), Some(PathBuf::from("E:/ScarlettCache").join("victory")));
        assert_eq!(get(&e, "VICTORY_RELAY").as_deref(), Some("2"));
        assert_eq!(get(&e, "VICTORY_PIPELINE_WAIT_MS").as_deref(), Some("0"));
        assert_eq!(get(&e, "VICTORY_PRECOMPILED_GRAPHICS").is_some(), PRECOMPILED_GRAPHICS);
        for k in SPEED_SETTINGS {
            assert_eq!(get(&e, k).as_deref(), Some("1"), "{k}");
        }
        assert!(get(&e, "VICTORY_SHADER_LOOP_LIMIT").is_none());
        assert!(get(&e, "VICTORY_AUTO_INPUT").is_none());
        c.auto_menu = true;
        c.raytracing = true;
        let e = world_env(&c, &g, "E:/c");
        assert_eq!(get(&e, "VICTORY_AUTO_INPUT").as_deref(), Some("1"));
        assert_eq!(get(&e, "VICTORY_RAYTRACING").as_deref(), Some("1"));
    }

    #[test]
    fn build_commands() {
        let g = PathBuf::from("D:/game");
        assert_eq!(warm_args(&g, 0), ["warm", "D:/game", "--jobs", "1"]);
        let a = graphics_args(&g, 4, 2000, 1000);
        assert_eq!(a[0], "precompile-graphics");
        assert!(a[1].ends_with("shaders.rpf"));
        assert_eq!(&a[2..], ["--jobs", "4", "--skip", "2000", "--limit", "1000"]);
    }

    #[test]
    fn jobs_are_bounded_by_cpu_and_ram() {
        assert_eq!(jobs(12, None, 1024), 11);
        assert_eq!(jobs(12, Some(2048 + 3 * 1536), 1536), 3);
        assert_eq!(jobs(12, Some(100), 1536), 1);
        assert_eq!(jobs(1, Some(64_000), 1024), 1);
    }

    #[test]
    fn parses_build_output() {
        assert_eq!(parse_warm("   123 s  4000 shaders"), Some(4000));
        assert_eq!(parse_warm("warm         25889 translated, 0 refused; 9000 compute pipelines, 0 refused; 171 s"), None);
        assert_eq!(parse_warm_end("warm         25889 translated, 3 refused; 9000 compute pipelines, 0 refused; 171 s"), Some((25889, 3)));
        assert_eq!(parse_graphics("    40 s  500 records: 480 made, 0 failed, 20 duplicates, 0 unmapped, 0 undecoded"), Some(500));
        let end = "precompile-graphics  471 records: 460 pipelines made, 1 failed, 0 left out, 10 duplicates, 0 with a shader not in the archive, 0 undecoded; 30 s";
        assert_eq!(parse_graphics(end), None);
        assert_eq!(parse_graphics_end(end), Some(471));
    }

    #[test]
    fn fps_parsing() {
        assert_eq!(parse_fps("D3D12.X: 600 frames presented, 27.2 fps over the last 300"), Some(27.2));
        assert_eq!(parse_fps("fps: 12.5"), Some(12.5));
        assert_eq!(parse_fps("nothing"), None);
    }
}
