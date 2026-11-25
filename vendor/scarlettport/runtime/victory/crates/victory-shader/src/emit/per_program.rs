//! Switches that pick a translation per program, by the hash a run's log
//! names the shader by: the first 16 hex digits of the BLAKE3 of the
//! container as the title passes it (`hash=` in "shader translated" and
//! "shader does not translate", `#hash` in `victory translate --list`).
//!
//! The one so far is for the wave64 form "without skips, scalars followed"
//! ([`crate::ir::without_skips`] with `deep`), the exact form tried for a
//! console wave64 compute program on a host of 32-lane waves before the
//! approximate per-half-wave ones. On an RTX 4080 (2026-11-24) it lost
//! walls, roofs and floors (about 5,000 draws a frame); off for every
//! program (`VICTORY_WAVE_DEEP_SKIPS=0`) those came back (about 6,900) and
//! motion looked wrong. So some of the programs that take it are wrong in
//! it and others better: it is chosen per program.
//! - `VICTORY_WAVE_DEEP_SKIPS=0`: off for every program, as before;
//! - `VICTORY_WAVE_DEEP_SKIPS_NOT=<hash>,...`: off for the listed programs;
//! - `VICTORY_WAVE_DEEP_SKIPS_ONLY=<hash>,...`: off for every program not
//!   listed.
//!
//! A program's decision is part of its shader cache key (`victory-gpu`'s
//! `shader_cache`), not the lists: changing a list translates again only
//! the programs whose decision it changes.

use std::cell::RefCell;
use std::sync::OnceLock;

use crate::dxbc::Container;
use crate::native::HwStage;

thread_local! {
    /// The hash of the container being translated on this thread ([`for_program`]).
    static PROGRAM: RefCell<Option<String>> = const { RefCell::new(None) };
}

/// Runs `f`, the translation of the container whose hash is `hash` (see the
/// module notes): the per-program switches and `VICTORY_WAVE_WHY`'s
/// `WAVE64-RETRY` lines see it.
pub fn for_program<T>(hash: &str, f: impl FnOnce() -> T) -> T {
    struct Restore(Option<String>);
    impl Drop for Restore {
        fn drop(&mut self) {
            PROGRAM.set(self.0.take());
        }
    }
    let _restore = Restore(PROGRAM.replace(Some(hash.to_owned())));
    f()
}

/// The hash [`for_program`] set on this thread, if any.
pub(super) fn current() -> Option<String> {
    PROGRAM.with_borrow(Clone::clone)
}

/// Whether the deep-skips form may be tried for the program whose hash is
/// `hash` (`None`: not known, as for a translation outside
/// [`for_program`]), by the switches in the module notes.
pub fn deep_skips_allowed(hash: Option<&str>) -> bool {
    struct Switches {
        on: bool,
        not: Option<Vec<String>>,
        only: Option<Vec<String>>,
    }
    static SWITCHES: OnceLock<Switches> = OnceLock::new();
    let s = SWITCHES.get_or_init(|| {
        let list = |name| std::env::var(name).ok().map(|v| parse_hash_list(&v));
        Switches {
            on: std::env::var_os("VICTORY_WAVE_DEEP_SKIPS").is_none_or(|v| v != "0"),
            not: list("VICTORY_WAVE_DEEP_SKIPS_NOT"),
            only: list("VICTORY_WAVE_DEEP_SKIPS_ONLY"),
        }
    });
    decide(s.on, s.not.as_deref(), s.only.as_deref(), hash)
}

/// Whether the wave64 forms, and so the deep-skips switches, can matter for
/// the program of `container`: a console wave64 compute program, on a host
/// of 32-lane waves. Cheap (no translation): a superset of the programs
/// that take the deep-skips form.
pub fn wave64_retried(container: &[u8]) -> bool {
    super::lower::host_max_wave() == 32
        && Container::parse(container)
            .ok()
            .and_then(|c| super::main_program(&c).ok())
            .is_some_and(|p| p.stage == HwStage::Cs && p.wave64)
}

/// The hashes of a `_NOT` or `_ONLY` list: separated by commas, semicolons
/// or white space, each with or without the `#` the offline listing puts
/// before it, in lower case and cut to the 16 digits a hash has. Pure, for
/// testing.
fn parse_hash_list(value: &str) -> Vec<String> {
    value
        .split(|c: char| c == ',' || c == ';' || c.is_whitespace())
        .map(|h| h.trim_start_matches('#').to_ascii_lowercase())
        .map(|h| h.chars().take(16).collect::<String>())
        .filter(|h| !h.is_empty())
        .collect()
}

/// Whether `list` names the program `hash`: an entry names every program
/// whose hash begins with it.
fn listed(list: &[String], hash: Option<&str>) -> bool {
    hash.is_some_and(|hash| {
        let hash = hash.to_ascii_lowercase();
        list.iter().any(|entry| hash.starts_with(entry.as_str()))
    })
}

/// [`deep_skips_allowed`] on the switches' values. Pure, for testing.
fn decide(on: bool, not: Option<&[String]>, only: Option<&[String]>, hash: Option<&str>) -> bool {
    on && !not.is_some_and(|l| listed(l, hash)) && only.is_none_or(|l| listed(l, hash))
}

#[cfg(test)]
mod tests {
    use super::*;

    const A: &str = "0123456789abcdef";
    const B: &str = "fedcba9876543210";

    fn list(v: &str) -> Vec<String> {
        parse_hash_list(v)
    }

    #[test]
    fn lists_split_on_commas_and_space_and_take_the_listing_form() {
        assert_eq!(list(""), Vec::<String>::new());
        assert_eq!(list(" , ;"), Vec::<String>::new());
        assert_eq!(list(A), vec![A]);
        assert_eq!(list(&format!("{A},{B}")), vec![A, B]);
        assert_eq!(list(&format!(" #{A} ;\t{B} ,")), vec![A, B]);
        assert_eq!(list("#0123456789ABCDEF"), vec![A], "upper case");
        // The whole BLAKE3 is cut to the hash a log shows.
        assert_eq!(list(&format!("{A}{B}{A}{B}")), vec![A]);
        // A shorter entry stays short: it names every hash that begins with it.
        assert_eq!(list("01234567"), vec!["01234567"]);
    }

    #[test]
    fn without_lists_the_global_switch_decides() {
        for hash in [Some(A), Some(B), None] {
            assert!(decide(true, None, None, hash));
            assert!(!decide(false, None, None, hash));
        }
    }

    #[test]
    fn not_keeps_the_listed_programs_out() {
        let not = list(&format!("#{B}, 01234567"));
        assert!(!decide(true, Some(&not), None, Some(A)), "by its first digits");
        assert!(!decide(true, Some(&not), None, Some(B)));
        assert!(!decide(true, Some(&not), None, Some(&B.to_ascii_uppercase())));
        assert!(!decide(true, Some(&not), None, Some("0123456700000000")));
        assert!(decide(true, Some(&not), None, Some("1111111111111111")));
        assert!(decide(true, Some(&not), None, None), "an unknown program is not listed");
        assert!(decide(true, Some(&[]), None, Some(A)), "an empty list keeps everything");
        assert!(!decide(false, Some(&not), None, Some("1111111111111111")), "=0 still wins");
    }

    #[test]
    fn only_lets_just_the_listed_programs_in() {
        let only = list(A);
        assert!(decide(true, None, Some(&only), Some(A)));
        assert!(!decide(true, None, Some(&only), Some(B)));
        assert!(!decide(true, None, Some(&only), None), "an unknown program is not listed");
        assert!(!decide(true, None, Some(&[]), Some(A)), "an empty list lets none in");
        assert!(!decide(false, None, Some(&only), Some(A)), "=0 still wins");
        // Both: listed in ONLY and not in NOT.
        let both = list(&format!("{A},{B}"));
        assert!(!decide(true, Some(&only), Some(&both), Some(A)));
        assert!(decide(true, Some(&only), Some(&both), Some(B)));
    }

    #[test]
    fn the_program_hash_is_seen_inside_its_translation_only() {
        assert_eq!(current(), None);
        let inner = for_program(A, || {
            let nested = for_program(B, current);
            (current(), nested)
        });
        assert_eq!(inner, (Some(A.to_owned()), Some(B.to_owned())));
        assert_eq!(current(), None);
        let _ = std::panic::catch_unwind(|| for_program(A, || panic!("a translation that fails")));
        assert_eq!(current(), None, "restored after a panic too");
    }

    #[test]
    fn what_is_no_container_is_not_retried() {
        for lanes in [0, 32, 64] {
            let was = super::super::lower::TEST_HOST_WAVE.replace(Some(lanes));
            assert!(!wave64_retried(b"not a container"), "{lanes}");
            assert!(!wave64_retried(b"DXBC"), "{lanes}");
            super::super::lower::TEST_HOST_WAVE.set(was);
        }
    }
}
