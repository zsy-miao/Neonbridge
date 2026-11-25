//! `victory saves`: list, create, copy, delete and restore save slots (see
//! `victory_core::saves` and `victory_core::backup`). Deleting moves a slot
//! to the title's `trash`; restoring brings back one of the rolling backups
//! (the last ten per container) the runtime makes at each save.

use std::path::PathBuf;

use anyhow::{Result, bail};
use clap::Subcommand;
use victory_core::{backup, saves};

#[derive(Subcommand)]
pub enum Saves {
    /// List the slots of each title, with their users, containers and backups.
    List,
    /// Create an empty slot (a fresh game).
    New { name: String },
    /// Copy a slot to a new one.
    Copy { from: String, to: String },
    /// Move a slot to the trash folder (nothing is erased).
    Delete { name: String },
    /// Bring back a backup of a container as its newest version. Without a
    /// container name, lists what can be restored and changes nothing. Do it
    /// while the game is not running. The version replaced is backed up
    /// first, so a restore can be undone the same way.
    Restore {
        /// The slot (`default` for the one a plain run uses).
        slot: String,
        /// The container, e.g. `SettingsProfileSettings`, `Player`, `SARC0015`.
        container: Option<String>,
        /// The backup id (from the listing); the newest if omitted.
        #[arg(long)]
        backup: Option<String>,
        /// The user folder (XUID in hex) when several have the container.
        #[arg(long)]
        user: Option<String>,
    },
}

/// Title folders to act on: `--title` (8 hex digits) or every one present.
fn targets(data: &std::path::Path, title: Option<&str>) -> Result<Vec<PathBuf>> {
    if let Some(t) = title {
        return Ok(vec![data.join(t.to_uppercase())]);
    }
    let all = saves::titles(data);
    if all.is_empty() {
        bail!("no title data under {}; run the game once or pass --title <ID>", data.display());
    }
    Ok(all)
}

/// `(xuid hex, gamertag)` of the local profiles file (or its last good
/// copy), if it can be read.
fn profiles() -> Vec<(String, String)> {
    let Some(base) = std::env::var_os("LOCALAPPDATA") else {
        return Vec::new();
    };
    let file = std::path::Path::new(&base).join("VIctory").join("profiles.txt");
    let valid = |d: &[u8]| std::str::from_utf8(d).is_ok();
    let text = victory_core::atomic::read_good(&file, valid)
        .map(|(d, _)| String::from_utf8_lossy(&d).into_owned())
        .unwrap_or_default();
    text.lines()
        .filter(|l| !l.trim().is_empty() && !l.trim_start().starts_with('#'))
        .filter_map(|l| {
            let (id, name) = l.trim().split_once(char::is_whitespace)?;
            let id = id.trim_start_matches("0x").to_uppercase();
            Some((format!("{id:0>16}"), name.trim().to_owned()))
        })
        .collect()
}

fn age(ms: u64) -> String {
    let now = std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .map_or(0, |d| d.as_millis() as u64);
    let s = now.saturating_sub(ms) / 1000;
    match s {
        0..=119 => format!("{s}s ago"),
        120..=7199 => format!("{}m ago", s / 60),
        7200..=172_799 => format!("{}h ago", s / 3600),
        _ => format!("{}d ago", s / 86400),
    }
}

fn print_slot(title: &std::path::Path, slot: &str, known: &[(String, String)]) -> Result<()> {
    let p = saves::resolve(title, slot)?;
    let note = if p.legacy { " (pre-slot location)" } else { "" };
    println!("  {slot}{note}");
    for provider in backup::inspect(&p.saves) {
        let tags: Vec<&str> = known
            .iter()
            .filter(|(id, _)| id.eq_ignore_ascii_case(&provider.user))
            .map(|(_, n)| n.as_str())
            .collect();
        let who = if tags.is_empty() {
            "not in profiles.txt".to_owned()
        } else {
            tags.join(", ")
        };
        println!("    user {} ({who}), config {}", provider.user, provider.config);
        if provider.containers.is_empty() {
            println!("      no containers");
        }
        for c in &provider.containers {
            let state = match c.version {
                Some(v) => format!("v{v}, {} bytes", c.bytes),
                None => "deleted, backups only".to_owned(),
            };
            println!("      {} ({state}, {} backups)", c.name, c.backups.len());
        }
    }
    Ok(())
}

/// Lists or restores in one title; false if the slot or container is not there.
fn restore(
    title: &std::path::Path,
    slot: &str,
    container: Option<&str>,
    id: Option<&str>,
    user: Option<&str>,
) -> Result<bool> {
    if !saves::list(title).iter().any(|s| s == slot) {
        return Ok(false);
    }
    let p = saves::resolve(title, slot)?;
    let mut found = Vec::new();
    for provider in backup::inspect(&p.saves) {
        if user.is_some_and(|u| !u.eq_ignore_ascii_case(&provider.user)) {
            continue;
        }
        for c in provider.containers {
            if container.is_none_or(|n| n == c.name) {
                found.push((provider.user.clone(), c));
            }
        }
    }
    let Some(name) = container else {
        for (user, c) in &found {
            println!("  user {user}: {}", c.name);
            for b in &c.backups {
                println!("    {}  v{}  {} bytes  {}", b.id, b.version, b.bytes, age(b.unix_ms));
            }
        }
        return Ok(true);
    };
    let [(user, c)] = found.as_slice() else {
        if found.is_empty() {
            return Ok(false);
        }
        let users: Vec<&str> = found.iter().map(|(u, _)| u.as_str()).collect();
        bail!(
            "container {name:?} exists for several users ({}); pick one with --user",
            users.join(", ")
        );
    };
    let chosen = match id {
        Some(id) => c.backups.iter().find(|b| b.id == id),
        None => c.backups.first(),
    };
    let Some(chosen) = chosen else {
        bail!(
            "no backup {} of {name:?} for user {user}; run `saves restore {slot}` to list them",
            id.unwrap_or("(any)")
        );
    };
    let n = backup::restore(&c.dir, chosen, backup::KEEP)?;
    println!(
        "  restored {name} (user {user}) from backup {} (was v{}) as v{n}",
        chosen.id, chosen.version
    );
    Ok(true)
}

pub fn run(cmd: Saves, title: Option<String>) -> Result<()> {
    let cfg = victory_core::Config::load()?;
    let titles = targets(&cfg.data_dir, title.as_deref())?;
    let known = profiles();
    let mut acted = false;
    for t in titles {
        let id = t.file_name().map(|n| n.to_string_lossy().into_owned()).unwrap_or_default();
        match &cmd {
            Saves::List => {
                println!("title {id}:");
                for s in saves::list(&t) {
                    print_slot(&t, &s, &known)?;
                }
            }
            Saves::New { name } => {
                saves::create(&t, name)?;
                println!("{id}: created slot {name}");
            }
            Saves::Copy { from, to } => {
                saves::copy(&t, from, to)?;
                println!("{id}: copied {from} to {to}");
            }
            Saves::Delete { name } => {
                let secs = std::time::SystemTime::now()
                    .duration_since(std::time::UNIX_EPOCH)
                    .map_or(0, |d| d.as_secs());
                let dest = saves::delete(&t, name, secs)?;
                println!("{id}: moved slot {name} to {}", dest.display());
            }
            Saves::Restore { slot, container, backup, user } => {
                println!("title {id}, slot {slot}:");
                acted |= restore(&t, slot, container.as_deref(), backup.as_deref(), user.as_deref())?;
            }
        }
    }
    if let Saves::Restore { slot, container, .. } = &cmd
        && !acted
    {
        match container {
            Some(c) => bail!("no container {c:?} in slot {slot:?} of any title"),
            None => bail!("no slot {slot:?} in any title"),
        }
    }
    Ok(())
}
