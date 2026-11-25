//! Why a run ended: after every run the launcher reads the run's log for the
//! known ways a runtime or the game stops and writes one plain answer to
//! `<cache>\last-crash.txt` (also printed, and put in `--report`).
//! NEONBRIDGE_CRASH_EXPLAIN=0 turns it off.
//!
//! What it knows, newest evidence first:
//! - an unhandled exception (its code named: the game's own breakpoint, a bad
//!   memory access, a stack overflow, a fail-fast, a C++ throw) and where;
//! - what the runtime logged just before it: a D3D12.X method it does not
//!   implement ("method not implemented object=... slot=0x..."), or a feature
//!   the title went without;
//! - the graphics device lost or hung (DXGI_ERROR_DEVICE_REMOVED / HUNG);
//! - a Rust panic in the runtime (its message and place);
//! - Windows out of page file (error 1455) or out of memory;
//! - and, failing all of those, the last line the run wrote.

/// How many log lines before the exception count as "just before".
const BEFORE: usize = 400;

fn code_name(code: &str) -> &'static str {
    match code.to_ascii_uppercase().trim_start_matches("0X") {
        "80000003" => "a breakpoint: the game stopped itself on purpose (its own check failed)",
        "C0000005" => "an access violation: code read or wrote memory it does not own",
        "C00000FD" => "a stack overflow",
        "C0000409" => "a fail-fast (stack buffer overrun or __fastfail)",
        "C000001D" => "an illegal CPU instruction",
        "C0000094" => "an integer divide by zero",
        "E06D7363" => "an uncaught C++ exception",
        "C0000374" => "heap corruption",
        _ => "an unhandled exception",
    }
}

fn after<'a>(line: &'a str, key: &str) -> Option<&'a str> {
    line.find(key).map(|i| line[i + key.len()..].trim())
}

/// Plain value of `key=...` in a log line (quotes removed).
fn field(line: &str, key: &str) -> Option<String> {
    let v = after(line, &format!("{key}="))?;
    let v = v.trim_start_matches('"');
    Some(v.chars().take_while(|c| !matches!(c, '"' | ' ' | ',')).collect())
}

/// The reason the run ended, if its log shows one.
pub fn explain(lines: &[String]) -> Option<String> {
    let mut out = Vec::new();
    let exc = lines.iter().rposition(|l| l.contains("unhandled exception 0x"));
    if let Some(i) = exc {
        let l = &lines[i];
        let rest = after(l, "unhandled exception ").unwrap_or("");
        let code = rest.split_whitespace().next().unwrap_or("");
        let at = after(rest, "at ").unwrap_or("?");
        out.push(format!("The game process ended on {} ({code}) at {at}.", code_name(code)));
        let from = i.saturating_sub(BEFORE);
        let near = &lines[from..i];
        if let Some(m) = near.iter().rev().find(|l| l.contains("method not implemented")) {
            let obj = field(m, "object").unwrap_or_else(|| "?".into());
            let slot = field(m, "slot").unwrap_or_else(|| "?".into());
            let n = u32::from_str_radix(slot.trim_start_matches("0x"), 16).map(|s| format!(" (method {})", s / 8)).unwrap_or_default();
            let caller = field(m, "caller").unwrap_or_default();
            out.push(format!(
                "Just before it, the game called a {obj} method the runtime does not implement: slot {slot}{n}, from the game's code at {caller}. The likely cause: the call failed, and the game's own check stopped it."
            ));
        }
        if let Some(m) = near.iter().rev().find(|l| l.contains("the title goes without")) {
            out.push(format!("Also just before: {}", m.split(" WARN ").last().unwrap_or(m).chars().take(160).collect::<String>()));
        }
        if near.iter().any(|l| l.contains("connection blocked") && l.contains("backtrace")) || lines[i..].iter().any(|l| l.contains("connection blocked") && l.contains("backtrace")) {
            out.push("The game then tried to send its crash report to its developer; the runtime blocked it (no network).".into());
        }
    }
    if let Some(l) = lines.iter().rev().find(|l| l.contains("DEVICE_REMOVED") || l.contains("DEVICE_HUNG") || l.contains("0x887A0005") || l.contains("0x887A0006") || l.to_ascii_lowercase().contains("device lost") || l.to_ascii_lowercase().contains("device removed")) {
        out.push(format!("The graphics device was lost: {}", l.chars().take(200).collect::<String>()));
    }
    if let Some(i) = lines.iter().rposition(|l| l.contains("panicked at")) {
        let msg = lines.get(i + 1).map(|s| s.trim()).unwrap_or("");
        out.push(format!("The runtime panicked: {} {msg}", lines[i].trim()));
    }
    if lines.iter().any(|l| l.contains("1455") && l.to_ascii_lowercase().contains("paging")) {
        out.push("Windows ran out of page file (error 1455): make the page file bigger.".into());
    } else if lines.iter().any(|l| l.contains("E_OUTOFMEMORY") || l.to_ascii_lowercase().contains("out of memory")) {
        out.push("Something ran out of memory (E_OUTOFMEMORY / out of memory in the log).".into());
    }
    (!out.is_empty()).then(|| out.join("\n"))
}

/// After a run: writes `last-crash.txt` beside the runtime's other files (or removes
/// a stale one), prints it, and returns it.
pub fn after_run(name: &str, cache: &std::path::Path, lines: &[String]) -> Option<String> {
    let file = cache.join("last-crash.txt");
    if std::env::var("NEONBRIDGE_CRASH_EXPLAIN").is_ok_and(|v| v.trim() == "0") {
        return None;
    }
    // The runtime stands in front of the game's own crash handler
    // (kernel.rs SetUnhandledExceptionFilter) and writes its report to
    // <save>\<title>\crashes.log: add this run's report (written in the last 10 min).
    let mut why = explain(lines);
    if let Some(t) = recent_crashes_log() {
        why = Some(format!("{}\nThe runtime's crash report (crashes.log, from the game's crash handler):\n{t}", why.unwrap_or_default()).trim_start().to_string());
    }
    match why {
        Some(why) => {
            println!("[crash] {name}: why the run ended:");
            for l in why.lines() {
                println!("[crash]   {l}");
            }
            let _ = std::fs::write(&file, &why);
            Some(why)
        }
        None => {
            let _ = std::fs::remove_file(&file);
            None
        }
    }
}

/// The tail of the newest `crashes.log` in the family save folder, if written in the last 10 min.
fn recent_crashes_log() -> Option<String> {
    let save = std::path::PathBuf::from(std::env::var("LOCALAPPDATA").ok()?).join("6ixpack").join("save");
    let newest = std::fs::read_dir(save).ok()?.flatten()
        .map(|d| d.path().join("crashes.log"))
        .filter_map(|p| Some((p.metadata().ok()?.modified().ok()?, p)))
        .max()?;
    if newest.0.elapsed().ok()? > std::time::Duration::from_secs(600) {
        return None;
    }
    let text = std::fs::read_to_string(&newest.1).ok()?;
    let all: Vec<&str> = text.lines().collect();
    Some(all[all.len().saturating_sub(20)..].join("\n"))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn names_the_missing_method_before_a_breakpoint() {
        let log: Vec<String> = [
            "2026 WARN victory_gpu::object: method not implemented object=\"device\" slot=0x330 caller=\"+0x75962\"",
            "VIctory: first-chance exception 0x80000003 at victory-image-b99f.dll+0x56abfa5",
            "VIctory: unhandled exception 0x80000003 at victory-image-b99f.dll+0x56abfa5",
            "INFO victory_hle::modules::net: HTTP connection blocked destination=submit.backtrace.io:443",
        ]
        .iter()
        .map(|s| s.to_string())
        .collect();
        let why = explain(&log).unwrap();
        assert!(why.contains("breakpoint"));
        assert!(why.contains("slot 0x330 (method 102)"));
        assert!(why.contains("+0x75962"));
        assert!(why.contains("crash report"));
    }

    #[test]
    fn quiet_when_nothing_went_wrong() {
        assert!(explain(&["INFO 30 fps".to_string()]).is_none());
    }
}
