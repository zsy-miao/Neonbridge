//! A per-run record of every stream pipeline the title did not get: refused
//! creations and null stand-ins, with their guest shader digests and how
//! often each happened, written as `pipeline-failures.tsv` beside the run's
//! shader requests (most frequent first). The digests name the requests, so
//! the translator's refusal reasons say why. Rewritten every 2 s while it
//! changes, so a crash loses little. Only failed pipelines are recorded:
//! their digests are already taken for the stage report.
//!
//! TuxBox 3f2a717 failures.rs (GPL-3.0, credit to its authors).
//! TUXBOX_PIPELINE_FAILURES=0 records nothing.

use std::collections::HashMap;
use std::sync::Mutex;

/// Distinct rows kept; further new rows are counted together.
const LIMIT: usize = 20_000;

#[derive(Default)]
struct Record {
    rows: HashMap<String, u64>,
    dropped: u64,
    changed: bool,
}

static RECORD: Mutex<Option<Record>> = Mutex::new(None);

fn render(record: &Record) -> String {
    let mut rows: Vec<_> = record.rows.iter().collect();
    rows.sort_by(|a, b| b.1.cmp(a.1).then_with(|| a.0.cmp(b.0)));
    let mut text = String::from("count\toutcome\tpath\tstages\n");
    for (row, count) in rows {
        text.push_str(&format!("{count}\t{row}\n"));
    }
    if record.dropped != 0 {
        text.push_str(&format!("{}\tnot listed\t-\t-\n", record.dropped));
    }
    text
}

fn file() -> std::path::PathBuf {
    let base = crate::device::shader_cache::root();
    match std::env::var("TUXBOX_SHADER_SESSION") {
        Ok(name) if name.starts_with("run-") && name.bytes().all(|c| c.is_ascii_alphanumeric() || matches!(c, b'-' | b'_')) => {
            base.join(name).join("pipeline-failures.tsv")
        }
        _ => base.join(format!("pipeline-failures-{}.tsv", std::process::id())),
    }
}

/// Records one pipeline: `status` its failure (or the refusal it got a
/// stand-in for, with `stand_in`), `stages` the stage digests.
pub(in crate::device) fn record(path: &str, status: i32, stand_in: bool, stages: &[Option<(String, bool)>; 8]) {
    if crate::env_toggle!("TUXBOX_PIPELINE_FAILURES") == Some("0") {
        return;
    }
    const NAMES: [&str; 8] = ["VS", "PS", "DS", "HS", "GS", "CS", "AS", "MS"];
    let outcome = if stand_in { "stand-in".to_string() } else { format!("{:#010x}", status as u32) };
    let named: Vec<_> = NAMES.iter().zip(stages).filter_map(|(n, s)| s.as_ref().map(|(d, _)| format!("{n}={d}"))).collect();
    let row = format!("{outcome}\t{path}\t{}", if named.is_empty() { "-".to_string() } else { named.join(" ") });
    let first = {
        let mut guard = RECORD.lock().unwrap_or_else(|e| e.into_inner());
        let first = guard.is_none();
        let record = guard.get_or_insert_with(Default::default);
        if let Some(count) = record.rows.get_mut(&row) {
            *count += 1;
        } else if record.rows.len() < LIMIT {
            record.rows.insert(row, 1);
        } else {
            record.dropped += 1;
        }
        record.changed = true;
        first
    };
    if first {
        let path = file();
        crate::diagnostics::log_line(format_args!(
            "pipeline failures are recorded in {} (TUXBOX_PIPELINE_FAILURES=0 off)", path.display()));
        let _ = std::thread::Builder::new().name("TuxBox pipeline failures".into()).spawn(move || loop {
            let text = {
                let mut guard = RECORD.lock().unwrap_or_else(|e| e.into_inner());
                match guard.as_mut() {
                    Some(record) if record.changed => {
                        record.changed = false;
                        Some(render(record))
                    }
                    _ => None,
                }
            };
            if let Some(text) = text {
                if let Some(dir) = path.parent() {
                    let _ = std::fs::create_dir_all(dir);
                }
                let pending = path.with_extension("tsv.tmp");
                if std::fs::write(&pending, text).is_ok() {
                    let _ = std::fs::rename(&pending, &path);
                }
            }
            std::thread::sleep(std::time::Duration::from_secs(2));
        });
    }
}
