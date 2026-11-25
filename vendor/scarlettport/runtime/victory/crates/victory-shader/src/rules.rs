//! Translator rules: what a translation used, and a fingerprint of each.
//!
//! The translated-shader cache used to be keyed by one digest of every source file of this
//! crate, so any edit threw away every cached translation. A *rule* is one named part of the
//! translator (a pass, a feature, one lowered instruction); while a shader is translated the
//! rules it went through are noted ([`record`]), and each rule has a fingerprint made from
//! just the sources it consists of ([`fingerprint`]). A cache entry keeps the fingerprints of
//! the rules its shader used, so an edit to a rule the shader never touched leaves the entry
//! valid.
//!
//! Where the rules' sources are:
//! - [`GROUPS`]: whole files (`spill.rs`, `emit/wave.rs`, ...) that belong to one feature;
//! - regions inside a file, marked by `// rule-region: begin <rule>` ... `// rule-region: end`
//!   lines (`build.rs` digests their text under that name, and leaves it out of the file's
//!   own digest). A name may have several regions. Regions do not nest;
//! - [`CORE`]: every other source (the shared infrastructure and the unmarked rest of each
//!   file). Every translation uses it, so an edit there makes every entry suspect.
//!
//! Files only tests build (`*_tests.rs`, `emit/host.rs`) are in no rule.
//!
//! A fingerprint is a *hint* about what an edit can change, not a proof: a helper shared by
//! two rules, or a rule that is not noted where it is used, leaves an entry looking valid
//! after the translator changed its output. The cache therefore never trusts a fingerprint
//! alone to say a translation is *right*, only to say it needs no second look: entries whose
//! fingerprints moved are translated again in the background and compared
//! (`victory-gpu`'s `shader_cache`), and `victory warm --incremental --verify-all` does so
//! for every entry whatever its fingerprints say. Mark a region only around code no other
//! rule's output depends on.

use std::cell::RefCell;
use std::collections::BTreeSet;

include!(concat!(env!("OUT_DIR"), "/rule_hashes.rs"));

/// The rule every translation uses: all sources not in another rule.
pub const CORE: &str = "core";

/// Bumped by hand when something outside this crate's sources changes what a translation
/// makes for every shader (the target format, a dependency's output): every entry then
/// becomes suspect.
pub const CORE_EPOCH: u32 = 1;

/// A rule made of whole source files, as paths under `src/`.
pub struct Group {
    pub rule: &'static str,
    pub files: &'static [&'static str],
}

/// The rules that are whole files. A rule is noted where the translator takes the path the
/// file holds (see the `note` calls), so a shader that never does keeps its entry through
/// edits to the file.
pub const GROUPS: &[Group] = &[
    Group { rule: "pass:spill", files: &["spill.rs"] },
    Group { rule: "pass:structure", files: &["structure.rs"] },
    Group { rule: "pass:resource", files: &["resource.rs"] },
    Group { rule: "pass:io", files: &["io.rs"] },
    Group { rule: "pass:threading", files: &["emit/threading.rs"] },
    Group { rule: "feature:wave", files: &["emit/wave.rs", "emit/own_bits.rs"] },
    Group { rule: "feature:bvh", files: &["bvh.rs", "emit/intersect.rs"] },
    Group { rule: "feature:link", files: &["emit/link.rs"] },
];

/// One rule's fingerprint, or `None` if this translator has no such rule (an entry that used
/// it is then suspect).
pub fn fingerprint(rule: &str) -> Option<[u8; 32]> {
    fingerprint_in(rule, FILES, REGIONS, GROUPS)
}

/// [`fingerprint`] over the given digests (`files`: path, digest; `regions`: rule, digest).
pub fn fingerprint_in(
    rule: &str,
    files: &[(&str, &str)],
    regions: &[(&str, &str)],
    groups: &[Group],
) -> Option<[u8; 32]> {
    let mut h = blake3::Hasher::new();
    h.update(b"victory-shader rule\0");
    h.update(rule.as_bytes());
    h.update(&[0]);
    if rule == CORE {
        h.update(&CORE_EPOCH.to_le_bytes());
        for (path, digest) in files {
            if !groups.iter().any(|g| g.files.contains(path)) {
                h.update(path.as_bytes());
                h.update(digest.as_bytes());
            }
        }
    } else if let Some(group) = groups.iter().find(|g| g.rule == rule) {
        for path in group.files {
            let (_, digest) = files.iter().find(|(p, _)| p == path)?;
            h.update(path.as_bytes());
            h.update(digest.as_bytes());
        }
    } else {
        let (_, digest) = regions.iter().find(|(name, _)| *name == rule)?;
        h.update(digest.as_bytes());
    }
    Some(*h.finalize().as_bytes())
}

thread_local! {
    static USED: RefCell<Option<BTreeSet<&'static str>>> = const { RefCell::new(None) };
}

/// Runs `f` (a translation) and returns what it gave with the rules it used, [`CORE`] first.
/// Translations run on one thread, so a note is that translation's.
pub fn record<T>(f: impl FnOnce() -> T) -> (T, Vec<&'static str>) {
    // Puts the outer recording back, with what this one used, even if `f` unwinds.
    struct Restore(Option<BTreeSet<&'static str>>);
    impl Drop for Restore {
        fn drop(&mut self) {
            let inner = USED.take();
            let mut outer = self.0.take();
            if let (Some(outer), Some(inner)) = (&mut outer, inner) {
                outer.extend(inner);
            }
            USED.set(outer);
        }
    }
    let restore = Restore(USED.replace(Some(BTreeSet::from([CORE]))));
    let out = f();
    let used = USED.with_borrow(|u| u.clone().unwrap_or_default());
    drop(restore);
    let mut rules: Vec<&'static str> = used.into_iter().filter(|r| *r != CORE).collect();
    rules.insert(0, CORE);
    (out, rules)
}

/// The translation on this thread used `rule` (a name of [`GROUPS`]). Nothing when no
/// translation is being recorded.
pub fn note(rule: &'static str) {
    USED.with_borrow_mut(|u| {
        if let Some(u) = u {
            u.insert(rule);
        }
    });
}

/// The translation used the instruction `op` of `kind`: noted if a region named `kind:op`
/// exists (the instruction's lowering is marked), else it is part of [`CORE`].
pub fn note_op(kind: &str, op: &str) {
    if USED.with_borrow(Option::is_none) {
        return;
    }
    if let Some((name, _)) = REGIONS.iter().find(|(name, _)| region_is_for(name, kind, op)) {
        note(name);
    }
}

/// Whether the region `name` is the lowering of `op` of `kind`: `kind:op`, or `kind:prefix*`
/// for a region that holds the lowering of every instruction that starts with `prefix`.
fn region_is_for(name: &str, kind: &str, op: &str) -> bool {
    let Some(pattern) = name.strip_prefix(kind).and_then(|r| r.strip_prefix(':')) else {
        return false;
    };
    match pattern.strip_suffix('*') {
        Some(prefix) => op.starts_with(prefix),
        None => pattern == op,
    }
}

/// The rule names with a region of their own, for the entry points that note them.
pub fn region_names() -> impl Iterator<Item = &'static str> {
    REGIONS.iter().map(|(name, _)| *name)
}

#[cfg(test)]
mod tests {
    use super::*;

    const GROUPS_T: &[Group] = &[
        Group { rule: "pass:a", files: &["a.rs"] },
        Group { rule: "feature:b", files: &["x/b1.rs", "x/b2.rs"] },
    ];
    const FILES_T: &[(&str, &str)] = &[("a.rs", "d1"), ("c.rs", "d2"), ("x/b1.rs", "d3"), ("x/b2.rs", "d4")];
    const REGIONS_T: &[(&str, &str)] = &[("valu:v_ldexp_f32", "r1"), ("valu:v_frexp", "r2")];

    fn fp(rule: &str, files: &[(&str, &str)], regions: &[(&str, &str)]) -> Option<[u8; 32]> {
        fingerprint_in(rule, files, regions, GROUPS_T)
    }

    #[test]
    fn a_fingerprint_follows_only_its_own_sources() {
        let all = ["core", "pass:a", "feature:b", "valu:v_ldexp_f32", "valu:v_frexp"];
        let base: Vec<_> = all.iter().map(|r| fp(r, FILES_T, REGIONS_T).unwrap()).collect();
        // the same sources, the same fingerprints
        for (r, b) in all.iter().zip(&base) {
            assert_eq!(fp(r, FILES_T, REGIONS_T).as_ref(), Some(b));
        }
        let moved = |files: &[(&str, &str)], regions: &[(&str, &str)]| -> Vec<&str> {
            all.iter()
                .zip(&base)
                .filter(|(r, b)| fp(r, files, regions).as_ref() != Some(*b))
                .map(|(r, _)| *r)
                .collect()
        };
        // an edit to a file of a group moves that rule only
        assert_eq!(moved(&[("a.rs", "NEW"), FILES_T[1], FILES_T[2], FILES_T[3]], REGIONS_T), ["pass:a"]);
        assert_eq!(moved(&[FILES_T[0], FILES_T[1], ("x/b2.rs", "NEW"), FILES_T[3]], REGIONS_T), ["feature:b"]);
        // an edit to an unmarked file, or the unmarked part of one, moves core only
        assert_eq!(moved(&[FILES_T[0], ("c.rs", "NEW"), FILES_T[2], FILES_T[3]], REGIONS_T), ["core"]);
        // a region moves its rule only
        assert_eq!(moved(FILES_T, &[("valu:v_ldexp_f32", "NEW"), REGIONS_T[1]]), ["valu:v_ldexp_f32"]);
        assert_eq!(moved(FILES_T, &[REGIONS_T[0], ("valu:v_frexp", "NEW")]), ["valu:v_frexp"]);
        // a new unmarked file is core's (the safe side): it moves core, and nothing else
        let more = [FILES_T[0], FILES_T[1], FILES_T[2], FILES_T[3], ("new.rs", "d9")];
        assert_eq!(moved(&more, REGIONS_T), ["core"]);
    }

    #[test]
    fn a_rule_this_translator_does_not_have_has_no_fingerprint() {
        assert_eq!(fp("valu:v_gone", FILES_T, REGIONS_T), None);
        assert_eq!(fp("feature:gone", FILES_T, REGIONS_T), None);
        // a group whose file is gone from the tree
        assert_eq!(fp("pass:a", &FILES_T[1..], REGIONS_T), None);
    }

    #[test]
    fn a_region_names_the_instructions_it_lowers() {
        assert!(region_is_for("valu:v_ldexp_f32", "valu", "v_ldexp_f32"));
        assert!(!region_is_for("valu:v_ldexp_f32", "valu", "v_ldexp_f32x"));
        assert!(!region_is_for("valu:v_ldexp_f32", "salu", "v_ldexp_f32"));
        assert!(region_is_for("valu:v_frexp_*", "valu", "v_frexp_mant_f32"));
        assert!(region_is_for("valu:v_frexp_*", "valu", "v_frexp_exp_i32_f32"));
        assert!(!region_is_for("valu:v_frexp_*", "valu", "v_ldexp_f32"));
    }

    #[test]
    fn rules_are_recorded_for_the_translation_that_ran_them() {
        // nothing recorded outside `record`
        note("pass:io");
        let ((), used) = record(|| {
            note("pass:spill");
            note("pass:spill");
            note("feature:wave");
        });
        assert_eq!(used, ["core", "feature:wave", "pass:spill"]);
        // recordings nest: the inner one has its own, and the outer sees them too
        let (inner, outer) = record(|| {
            note("pass:io");
            record(|| note("pass:structure")).1
        });
        assert_eq!(inner, ["core", "pass:structure"]);
        assert_eq!(outer, ["core", "pass:io", "pass:structure"]);
        let ((), after) = record(|| ());
        assert_eq!(after, ["core"]);
    }

    #[test]
    fn this_translators_own_rules_all_have_fingerprints() {
        assert!(fingerprint(CORE).is_some());
        for g in GROUPS {
            assert!(fingerprint(g.rule).is_some(), "{} names a file that is not in the tree", g.rule);
            for f in g.files {
                assert!(FILES.iter().any(|(p, _)| p == f), "{f} of {} is not a source of the crate", g.rule);
            }
        }
        for name in region_names() {
            assert!(fingerprint(name).is_some());
        }
        assert!(fingerprint("no such rule").is_none());
        // test-only files are in no rule
        assert!(FILES.iter().all(|(p, _)| !p.ends_with("_tests.rs") && *p != "emit/host.rs"));
    }
}
