//! A per-run record of every pipeline the title did not get: failed creations
//! and empty stand-ins, with their guest shader digests and how often each
//! happened. The digests name the recorded shader requests, so the refusal
//! reasons beside the runtime cache say why.
//!
//! Shader bytes are only hashed for a pipeline that is recorded. The file is
//! rewritten every two seconds while it changes, so a crash loses little.

use std::cell::{Cell, RefCell};
use std::collections::HashMap;
use std::path::{Path, PathBuf};
use std::sync::{LazyLock, Mutex, Once};

/// Distinct rows kept. Further new rows are counted together.
const LIMIT: usize = 20_000;

/// Translation lookups kept per pipeline. A mesh pipeline needs a handful.
const KEYS: usize = 16;

thread_local! {
    static STAGES: RefCell<Vec<(&'static str, usize, usize)>> = const { RefCell::new(Vec::new()) };
    static LOOKUPS: RefCell<Vec<String>> = const { RefCell::new(Vec::new()) };
    static STAND_IN: Cell<bool> = const { Cell::new(false) };
}

#[derive(Default)]
struct Record {
    rows: HashMap<String, u64>,
    dropped: u64,
    changed: bool,
}

impl Record {
    fn add(&mut self, row: String) {
        if let Some(count) = self.rows.get_mut(&row) {
            *count += 1;
        } else if self.rows.len() < LIMIT {
            self.rows.insert(row, 1);
        } else {
            self.dropped += 1;
        }
        self.changed = true;
    }

    /// Tab-separated, most frequent first.
    fn render(&self) -> String {
        let mut rows: Vec<_> = self.rows.iter().collect();
        rows.sort_by(|a, b| b.1.cmp(a.1).then_with(|| a.0.cmp(b.0)));
        let mut text = String::from("count\toutcome\tpath\tstages\n");
        for (row, count) in rows {
            text.push_str(&format!("{count}\t{row}\n"));
        }
        if self.dropped != 0 {
            text.push_str(&format!("{}\tnot listed\t-\t-\n", self.dropped));
        }
        text
    }
}

static RECORD: LazyLock<Mutex<Record>> = LazyLock::new(Default::default);

/// Start describing the pipeline this thread is about to create.
pub(crate) fn begin() {
    STAGES.with_borrow_mut(Vec::clear);
    LOOKUPS.with_borrow_mut(Vec::clear);
    STAND_IN.set(false);
}

fn push_note(item: String) {
    LOOKUPS.with_borrow_mut(|lookups| {
        if lookups.len() < KEYS && !lookups.contains(&item) {
            lookups.push(item);
        }
    });
}

/// Note a translation this thread asked for. The key names the recorded
/// request and its cached output or refusal reason; it differs from the guest
/// shader digest when a root signature is part of the request.
pub(crate) fn lookup(stage: &str, key: &str, found: bool) {
    let kind = if found { "used" } else { "missing" };
    push_note(format!("{stage}.{kind}={key}"));
}

/// Note why a pipeline stage was refused locally before reaching the host driver.
pub(crate) fn refusal(stage: &str, reason: &str) {
    push_note(format!("{stage}.refused={reason}"));
}

/// Note one guest shader. The bytes must stay readable until `finish`.
pub(crate) fn stage(name: &'static str, code: *const u8, bytes: usize) {
    if !code.is_null() && bytes != 0 && bytes <= 64 << 20 {
        STAGES.with_borrow_mut(|stages| stages.push((name, code.addr(), bytes)));
    }
}

/// The pipeline being created on this thread was given an empty stand-in.
pub(crate) fn stand_in() {
    STAND_IN.set(true);
}

fn row(
    path: &str,
    status: i32,
    stand_in: bool,
    stages: &[(&str, Option<String>)],
    lookups: &[String],
) -> Option<String> {
    let outcome = if status < 0 {
        format!("{:#010x}", status as u32)
    } else if stand_in {
        "stand-in".to_string()
    } else {
        return None;
    };
    let stages: Vec<_> = stages
        .iter()
        .map(|(name, digest)| format!("{name}={}", digest.as_deref().unwrap_or("unknown")))
        .chain(lookups.iter().cloned())
        .collect();
    let stages = if stages.is_empty() { "-".to_string() } else { stages.join(" ") };
    Some(format!("{outcome}\t{path}\t{stages}"))
}

/// Record the pipeline if it failed or got a stand-in. Returns the file's path
/// the first time anything is recorded.
///
/// # Safety
/// Every pointer given to `stage` since `begin` must still be readable.
pub(crate) unsafe fn finish(
    path: &str,
    status: i32,
    digest: impl Fn(&[u8]) -> Option<String>,
) -> Option<PathBuf> {
    let stand_in = STAND_IN.replace(false);
    let lookups = LOOKUPS.take();
    if status >= 0 && !stand_in {
        STAGES.with_borrow_mut(Vec::clear);
        return None;
    }
    let stages: Vec<_> = STAGES
        .take()
        .into_iter()
        .map(|(name, code, bytes)| {
            // SAFETY: the caller keeps the guest shader readable until here.
            let code = unsafe { std::slice::from_raw_parts(code as *const u8, bytes) };
            (name, digest(code))
        })
        .collect();
    let row = row(path, status, stand_in, &stages, &lookups)?;
    RECORD.lock().unwrap_or_else(|e| e.into_inner()).add(row);
    static WRITER: Once = Once::new();
    let mut started = None;
    WRITER.call_once(|| {
        let file = file_path();
        started = Some(file.clone());
        let _ = std::thread::Builder::new()
            .name("tuxbox-failure-log".into())
            .spawn(move || {
                loop {
                    std::thread::sleep(std::time::Duration::from_secs(2));
                    write_if_changed(&RECORD, &file);
                }
            });
    });
    started
}

fn file_path() -> PathBuf {
    let seconds = std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .map_or(0, |age| age.as_secs());
    std::env::temp_dir()
        .join("tuxbox_shader_cache")
        .join("pipeline-failures")
        .join(format!("{seconds}-{}.tsv", std::process::id()))
}

fn write_if_changed(record: &Mutex<Record>, file: &Path) {
    let text = {
        let mut record = record.lock().unwrap_or_else(|e| e.into_inner());
        if !record.changed {
            return;
        }
        record.changed = false;
        record.render()
    };
    let pending = file.with_extension("tmp");
    let written = file
        .parent()
        .map_or(Ok(()), std::fs::create_dir_all)
        .and_then(|()| std::fs::write(&pending, text))
        .and_then(|()| std::fs::rename(&pending, file));
    if written.is_err() {
        // Try again on the next pass.
        record.lock().unwrap_or_else(|e| e.into_inner()).changed = true;
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn only_failures_and_stand_ins_make_rows() {
        let stages = [("MS", Some("aa".to_string())), ("PS", None)];
        assert_eq!(row("slot86", 0, false, &stages, &[]), None);
        assert_eq!(
            row("slot86", 0x8000_4001_u32 as i32, false, &stages, &["MS.missing=bb".into()]).unwrap(),
            "0x80004001\tslot86\tMS=aa PS=unknown MS.missing=bb"
        );
        assert_eq!(row("slot62", 0, true, &[], &[]).unwrap(), "stand-in\tslot62\t-");
    }

    #[test]
    fn repeats_are_counted_and_sorted_by_frequency() {
        let mut record = Record::default();
        record.add("b".into());
        record.add("a".into());
        record.add("a".into());
        assert_eq!(record.render(), "count\toutcome\tpath\tstages\n2\ta\n1\tb\n");
    }

    #[test]
    fn rows_past_the_limit_are_still_counted() {
        let mut record = Record::default();
        for index in 0..LIMIT + 3 {
            record.add(index.to_string());
        }
        record.add("0".into());
        assert_eq!(record.rows.len(), LIMIT);
        assert_eq!(record.dropped, 3);
        assert!(record.render().ends_with("3\tnot listed\t-\t-\n"));
    }

    #[test]
    fn a_failed_pipeline_reaches_the_file() {
        let code = *b"shader";
        begin();
        stage("CS", code.as_ptr(), code.len());
        lookup("CS", "key", true);
        lookup("CS", "key", true);
        // SAFETY: `code` is alive across the call.
        let first = unsafe { finish("slot62", -1, |bytes| Some(format!("len{}", bytes.len()))) };
        assert!(first.is_some());
        let file = std::env::temp_dir().join(format!("tuxbox-failures-{}.tsv", std::process::id()));
        write_if_changed(&RECORD, &file);
        let text = std::fs::read_to_string(&file).unwrap();
        let _ = std::fs::remove_file(&file);
        assert!(text.contains("1\t0xffffffff\tslot62\tCS=len6 CS.used=key\n"), "{text}");
    }

    #[test]
    fn wave_refusal_records_stage_reason() {
        let code = *b"shader";
        begin();
        stage("CS", code.as_ptr(), code.len());
        lookup("CS", "key", true);
        refusal("CS", "host-lacks-wave64");
        stand_in();
        // SAFETY: `code` stays live across `finish`.
        let _ = unsafe { finish("slot62", 0, |bytes| Some(format!("len{}", bytes.len()))) };
        let rendered = RECORD.lock().unwrap_or_else(|e| e.into_inner()).render();
        assert!(
            rendered.contains("stand-in\tslot62\tCS=len6 CS.used=key CS.refused=host-lacks-wave64\n"),
            "{rendered}"
        );
    }
}
