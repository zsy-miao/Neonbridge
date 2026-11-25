//! Why draws bind null descriptors (TUXBOX_NULL_CAUSES=0 off; idea from
//! Xbox2Windows 47dfb267/d9d8b46a, MIT, re-derived for inline records). Each
//! null bound into an active table slot counts under exactly one cause, tried
//! in this order, so the causes add up to the total:
//! - entry: the table entry was never written, or its record is malformed;
//! - buffer: the table address/length is outside the heap (whole table fails);
//! - token: the record names a source no descriptor arena owns;
//! - wrapper: the source arena exists but the view there was rewritten or freed;
//! - dimension: always 0 here, as unwritten slots get the null kind the shader
//!   reads (null_kinds), so no view of the wrong dimension is bound.
//! Inactive slots (deliberate identity nulls) are not counted.
use std::sync::atomic::{AtomicU64, Ordering};

#[derive(Clone, Copy)]
pub(crate) enum Cause {
    Entry = 0,
    Buffer = 1,
    Token = 2,
    Wrapper = 3,
}

static COUNTS: [AtomicU64; 4] = [const { AtomicU64::new(0) }; 4];

fn enabled() -> bool {
    static ON: std::sync::LazyLock<bool> =
        std::sync::LazyLock::new(|| std::env::var_os("TUXBOX_NULL_CAUSES").is_none_or(|v| v != "0"));
    *ON
}

pub(crate) fn note(cause: Cause, count: u64) {
    if count != 0 && enabled() {
        COUNTS[cause as usize].fetch_add(count, Ordering::Relaxed);
    }
}

/// Counts since the previous summary, or "" when off or nothing was null.
pub(crate) fn summary() -> String {
    if !enabled() {
        return String::new();
    }
    let c: Vec<u64> = COUNTS.iter().map(|c| c.swap(0, Ordering::Relaxed)).collect();
    let total: u64 = c.iter().sum();
    if total == 0 {
        return " | null descriptors bound: 0".into();
    }
    format!(" | null descriptors bound: {total} (entry {} buffer {} token {} wrapper {} dimension 0)", c[0], c[1], c[2], c[3])
}
