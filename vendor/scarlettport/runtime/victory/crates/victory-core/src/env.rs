//! Environment switches: every `VICTORY_*` variable can also be given as
//! `SCARLETT_*` (the `VICTORY_` name wins when both are set).

use std::env::VarError;
use std::ffi::OsString;

fn alias(name: &str) -> Option<String> {
    name.strip_prefix("VICTORY_").map(|rest| format!("SCARLETT_{rest}"))
}

/// `std::env::var`, also trying the `SCARLETT_` spelling of a `VICTORY_` name.
pub fn var(name: &str) -> Result<String, VarError> {
    match std::env::var(name) {
        Err(VarError::NotPresent) => match alias(name) {
            Some(a) => std::env::var(a),
            None => Err(VarError::NotPresent),
        },
        other => other,
    }
}

/// `std::env::var_os`, also trying the `SCARLETT_` spelling.
pub fn var_os(name: &str) -> Option<OsString> {
    std::env::var_os(name).or_else(|| alias(name).and_then(|a| std::env::var_os(a)))
}

/// The variable's value when it is set (the name `env(name)` of the spec).
pub fn env(name: &str) -> Option<String> {
    var(name).ok()
}

/// At startup, before any thread: every `SCARLETT_*` variable is copied to
/// its `VICTORY_*` name unless that is set, so code reading `std::env`
/// directly sees the alias too.
pub fn alias_scarlett() {
    let pairs: Vec<(OsString, OsString)> = std::env::vars_os()
        .filter_map(|(k, v)| {
            let rest = k.to_str()?.strip_prefix("SCARLETT_")?;
            let name = OsString::from(format!("VICTORY_{rest}"));
            std::env::var_os(&name).is_none().then_some((name, v))
        })
        .collect();
    for (k, v) in pairs {
        // SAFETY: called first thing in `main`, before other threads exist.
        unsafe { std::env::set_var(k, v) };
    }
}
