//! How tests say they did not run.
//!
//! A test that lacks a prerequisite (a compiler, a device) returns early
//! and the harness reports `ok`, which reads as a pass. [`skip`] makes the
//! gap loud: every skip prints a `VICTORY TEST SKIPPED:` line, and with
//! `VICTORY_STRICT_TESTS=1` it fails the test instead, for machines where
//! the prerequisites are expected to be there.

/// Reports that the running test is not exercising what it claims to.
///
/// Panics with `VICTORY_STRICT_TESTS=1`; otherwise prints and returns, and
/// the caller returns or continues as before.
pub fn skip(reason: &str) {
    if strict() {
        panic!("VICTORY TEST SKIPPED (VICTORY_STRICT_TESTS=1): {reason}");
    }
    eprintln!("VICTORY TEST SKIPPED: {reason}");
}

fn strict() -> bool {
    std::env::var("VICTORY_STRICT_TESTS").is_ok_and(|v| v == "1")
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn skip_is_quiet_about_failing_unless_strict() {
        if strict() {
            assert!(std::panic::catch_unwind(|| skip("x")).is_err());
            return;
        }
        skip("x");
    }
}
