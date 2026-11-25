//! Global host state (set once at host start; defaults in unit tests).

use std::sync::OnceLock;

use crate::config::{Boot, Config};

static BOOT: OnceLock<Boot> = OnceLock::new();
static DEFAULT_CONFIG: OnceLock<Config> = OnceLock::new();

pub fn set_boot(b: Boot) {
    let _ = BOOT.set(b);
}

pub fn boot() -> Option<&'static Boot> {
    BOOT.get()
}

pub fn config() -> &'static Config {
    match BOOT.get() {
        Some(b) => &b.config,
        None => DEFAULT_CONFIG.get_or_init(Config::default),
    }
}

/// Folder for title data (saves, PLS, temp…) – a temp folder when no boot record.
pub fn data_dir() -> std::path::PathBuf {
    match BOOT.get() {
        Some(b) => b.data_dir.clone(),
        None => std::env::temp_dir().join("xcl_unit_data"),
    }
}
