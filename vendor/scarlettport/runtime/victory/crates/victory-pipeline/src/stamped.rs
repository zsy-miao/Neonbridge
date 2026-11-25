//! Stamped entries: converted data that says which parts of its converter it was made with,
//! so that a change to the converter only costs the entries it can have changed.
//!
//! An entry is found by a key that has nothing of the converter's build in it ([`context_key`]:
//! the settings that change what is made; [`entry_key`]: those and the input). The entry keeps
//! with its payload
//! - a *rule stamp*: the name and fingerprint of each converter rule the input went through
//!   (for translated shaders, `victory_shader::rules`), and
//! - the converter *source id* it was last verified with.
//!
//! Reading an entry says where it [`stands`](standing) with the converter of this build:
//! - [`Standing::Verified`]: made or checked by this very converter. Use it as it is.
//! - [`Standing::Unchanged`]: another converter build, but every rule it went through has the
//!   fingerprint it had. Nothing it used changed: use it, and note that this build agrees
//!   ([`revalidate`] does that without converting).
//! - [`Standing::Suspect`]: a rule it used changed or is gone. Use it for now (the caller
//!   decides) and [`revalidate`]: convert again and compare. An output that did not change
//!   keeps its entry (restamped); one that did replaces it, and only then is it a change.
//!
//! The comparison is on the payload bytes, so "the payload changed" is exactly "whatever is
//! keyed by the payload's content (a compiled pipeline, say) is stale", and identical output
//! keeps all of that.

use std::path::PathBuf;

use crate::{Cache, CacheKey, Converter};

/// A converter rule's name and fingerprint.
pub type Rules = Vec<(String, [u8; 32])>;

const MAGIC: &[u8; 4] = b"VSE2";

/// One entry as stored.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Entry {
    /// The converter source this entry was made or last verified with.
    pub verified: String,
    /// The rules the conversion went through, with their fingerprints when it ran.
    pub rules: Rules,
    /// What was made.
    pub payload: Vec<u8>,
}

impl Entry {
    pub fn encode(&self) -> Vec<u8> {
        let mut out = MAGIC.to_vec();
        let text = |out: &mut Vec<u8>, t: &str| {
            out.extend((t.len() as u16).to_le_bytes());
            out.extend(t.as_bytes());
        };
        text(&mut out, &self.verified);
        out.extend((self.rules.len() as u16).to_le_bytes());
        for (name, fingerprint) in &self.rules {
            text(&mut out, name);
            out.extend(fingerprint);
        }
        out.extend((self.payload.len() as u64).to_le_bytes());
        out.extend(&self.payload);
        // (the digest of all of it: a damaged stamp is as much a damaged entry as a damaged payload)
        let digest = blake3::hash(&out);
        out.extend(digest.as_bytes());
        out
    }

    /// `None` for anything that is not an entry as [`Entry::encode`] makes it, or is damaged
    /// (checked against the digest stored at its end).
    pub fn decode(bytes: &[u8]) -> Option<Entry> {
        struct Reader<'a>(&'a [u8]);
        impl<'a> Reader<'a> {
            fn take(&mut self, n: usize) -> Option<&'a [u8]> {
                let (head, rest) = self.0.split_at_checked(n)?;
                self.0 = rest;
                Some(head)
            }
            fn u16(&mut self) -> Option<usize> {
                Some(u16::from_le_bytes(self.take(2)?.try_into().ok()?) as usize)
            }
            fn text(&mut self) -> Option<String> {
                let n = self.u16()?;
                String::from_utf8(self.take(n)?.to_vec()).ok()
            }
            fn digest(&mut self) -> Option<[u8; 32]> {
                self.take(32)?.try_into().ok()
            }
        }
        let (bytes, digest) = bytes.split_at_checked(bytes.len().checked_sub(32)?)?;
        if blake3::hash(bytes).as_bytes() != digest {
            return None;
        }
        let mut r = Reader(bytes);
        if r.take(4)? != MAGIC {
            return None;
        }
        let verified = r.text()?;
        let n = r.u16()?;
        let mut rules = Vec::with_capacity(n.min(64));
        for _ in 0..n {
            let name = r.text()?;
            rules.push((name, r.digest()?));
        }
        let len = usize::try_from(u64::from_le_bytes(r.take(8)?.try_into().ok()?)).ok()?;
        let payload = r.take(len)?;
        r.0.is_empty().then(|| Entry {
            verified,
            rules,
            payload: payload.to_vec(),
        })
    }
}

/// Where an entry stands with the converter of this build.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Standing {
    Verified,
    Unchanged,
    Suspect,
}

/// `entry`'s standing with the converter whose source id is `source` and whose rules have the
/// fingerprints `current` gives (`None`: no such rule).
pub fn standing(entry: &Entry, source: &str, current: &dyn Fn(&str) -> Option<[u8; 32]>) -> Standing {
    if entry.verified == source {
        Standing::Verified
    } else if entry.rules.iter().all(|(name, fp)| current(name).as_ref() == Some(fp)) {
        Standing::Unchanged
    } else {
        Standing::Suspect
    }
}

struct Kind;

impl Converter for Kind {
    fn id(&self) -> &'static str {
        "shader.translation"
    }
    fn version(&self) -> u32 {
        // The entry format ([`MAGIC`]); the converter's own versions are in the rules.
        2
    }
    fn convert(&self, _: &[u8]) -> anyhow::Result<Vec<u8>> {
        unreachable!("entries are made by `revalidate` and `store`")
    }
}

/// The key of the settings a conversion runs under: `schema` (the stored format's version)
/// and the named values that change what is made. Nothing of the converter's build is in it:
/// that is what the entries' rule stamps are for. Names and values do not run into each other.
pub fn context_key(schema: u32, settings: &[(&str, &[u8])]) -> [u8; 32] {
    let mut h = blake3::Hasher::new();
    h.update(&schema.to_le_bytes());
    for (name, value) in settings {
        for part in [name.as_bytes(), value] {
            h.update(&(part.len() as u64).to_le_bytes());
            h.update(part);
        }
    }
    *h.finalize().as_bytes()
}

/// The key of the entry for `input` under the settings of `context` ([`context_key`]).
pub fn entry_key(context: &[u8; 32], input: &[u8]) -> CacheKey {
    CacheKey::new(&Kind, &[&context[..], input].concat())
}

/// The entry stored under `key`, if there is one that reads back whole.
pub fn read(cache: &Cache, key: &CacheKey) -> Option<Entry> {
    cache.read(key).and_then(|b| Entry::decode(&b))
}

/// Stores a fresh conversion's `payload`, made by the converter `source` with `rules`.
pub fn store(cache: &Cache, key: &CacheKey, source: &str, rules: Rules, payload: Vec<u8>) -> anyhow::Result<PathBuf> {
    cache.write(
        key,
        &Entry {
            verified: source.to_owned(),
            rules,
            payload,
        }
        .encode(),
    )
}

/// What [`revalidate`] found.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Revalidated {
    /// No rule the entry used changed: restamped without converting.
    Restamped,
    /// Converted again: the same payload. Restamped with the rules it went through now.
    Same,
    /// Converted again: another payload, which replaced the entry.
    Changed,
    /// Converting again failed (or the entry could not be written): the entry stays as it was.
    Failed,
}

/// Brings `entry` (read from `key`) up to the converter of this build: restamps it if no rule
/// it used changed, else runs `convert` again and compares. The old entry is kept until a
/// different payload replaces it. Gives what was found and the payload to use now (the new
/// one if the conversion made another, else the entry's).
pub fn revalidate(
    cache: &Cache,
    key: &CacheKey,
    entry: Entry,
    source: &str,
    current: &dyn Fn(&str) -> Option<[u8; 32]>,
    convert: impl FnOnce() -> Result<(Vec<u8>, Rules), String>,
) -> (Revalidated, Vec<u8>) {
    let standing = standing(&entry, source, current);
    revalidate_as(cache, key, entry, source, standing, convert)
}

/// Converts again whatever the entry's stamp says, and compares ([`Context::verify_all`]).
pub fn reconvert(
    cache: &Cache,
    key: &CacheKey,
    entry: Entry,
    source: &str,
    convert: impl FnOnce() -> Result<(Vec<u8>, Rules), String>,
) -> (Revalidated, Vec<u8>) {
    revalidate_as(cache, key, entry, source, Standing::Suspect, convert)
}

/// [`revalidate`] for an entry whose `standing` is already decided (or overruled).
fn revalidate_as(
    cache: &Cache,
    key: &CacheKey,
    entry: Entry,
    source: &str,
    standing: Standing,
    convert: impl FnOnce() -> Result<(Vec<u8>, Rules), String>,
) -> (Revalidated, Vec<u8>) {
    let put = |entry: &Entry| cache.write(key, &entry.encode()).is_ok();
    match standing {
        Standing::Verified => (Revalidated::Same, entry.payload),
        Standing::Unchanged => {
            let restamped = Entry {
                verified: source.to_owned(),
                ..entry
            };
            let found = if put(&restamped) { Revalidated::Restamped } else { Revalidated::Failed };
            (found, restamped.payload)
        }
        Standing::Suspect => match convert() {
            Err(_) => (Revalidated::Failed, entry.payload),
            Ok((payload, rules)) => {
                let same = payload == entry.payload;
                let fresh = Entry {
                    verified: source.to_owned(),
                    rules,
                    payload,
                };
                match (put(&fresh), same) {
                    (false, _) => (Revalidated::Failed, entry.payload),
                    (true, true) => (Revalidated::Same, fresh.payload),
                    (true, false) => (Revalidated::Changed, fresh.payload),
                }
            }
        },
    }
}

/// When entries that stand [`Suspect`](Standing::Suspect) or [`Unchanged`](Standing::Unchanged)
/// are brought up to date.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Mode {
    /// Used as they are now; the caller revalidates them later ([`How::Pending`]), off the
    /// path of the run.
    Lazy,
    /// Revalidated before use: the caller waits for the comparison.
    Eager,
    /// Used as they are, and never revalidated.
    Trust,
}

/// What [`serve`] runs under.
pub struct Context<'a> {
    /// The converter's source id (see [`Entry::verified`]).
    pub source: &'a str,
    /// The converter's rule fingerprints.
    pub current: &'a dyn Fn(&str) -> Option<[u8; 32]>,
    pub mode: Mode,
    /// Treat every entry as suspect whatever its stamp says (the rule fingerprints are a hint:
    /// this is the check on them).
    pub verify_all: bool,
}

/// How [`serve`] came by its payload.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum How {
    /// An entry this converter made or verified.
    Hit,
    /// An entry used as it is that [`Mode::Lazy`] wants revalidated by the caller.
    Pending,
    /// An entry used as it is, under [`Mode::Trust`].
    Trusted,
    /// No entry: converted and stored.
    New,
    /// Revalidated first ([`Mode::Eager`]).
    Revalidated(Revalidated),
}

/// What an entry was served as.
pub struct Served {
    pub payload: Vec<u8>,
    pub how: How,
}

/// The payload for `key`: from the entry if there is one that reads back, brought up to date as
/// `ctx.mode` says, else made by `convert` and stored. An `Err` is a conversion that failed
/// with no entry to fall back on (nothing is stored then).
pub fn serve(
    cache: &Cache,
    key: &CacheKey,
    ctx: &Context<'_>,
    convert: impl FnOnce() -> Result<(Vec<u8>, Rules), String>,
) -> Result<Served, String> {
    let Some(entry) = read(cache, key) else {
        let (payload, rules) = convert()?;
        // (an entry that cannot be written costs the next run a conversion: not this one)
        let _ = store(cache, key, ctx.source, rules, payload.clone());
        return Ok(Served { payload, how: How::New });
    };
    let standing = if ctx.verify_all {
        Standing::Suspect
    } else {
        standing(&entry, ctx.source, ctx.current)
    };
    Ok(match (standing, ctx.mode) {
        (Standing::Verified, _) => Served { payload: entry.payload, how: How::Hit },
        (_, Mode::Trust) => Served { payload: entry.payload, how: How::Trusted },
        (_, Mode::Lazy) => Served { payload: entry.payload, how: How::Pending },
        (standing, Mode::Eager) => {
            let (found, payload) = revalidate_as(cache, key, entry, ctx.source, standing, convert);
            Served { payload, how: How::Revalidated(found) }
        }
    })
}

#[cfg(test)]
mod tests {
    use super::*;

    fn fp(n: u8) -> [u8; 32] {
        [n; 32]
    }

    fn entry(source: &str, rules: &[(&str, u8)], payload: &[u8]) -> Entry {
        Entry {
            verified: source.to_owned(),
            rules: rules.iter().map(|(n, f)| ((*n).to_owned(), fp(*f))).collect(),
            payload: payload.to_vec(),
        }
    }

    /// The fingerprints of a translator that has these rules.
    fn translator(rules: &'static [(&'static str, u8)]) -> impl Fn(&str) -> Option<[u8; 32]> {
        move |name| rules.iter().find(|(n, _)| *n == name).map(|(_, f)| fp(*f))
    }

    #[test]
    fn entries_come_back_whole_or_not_at_all() {
        let e = entry("src A", &[("core", 1), ("valu:v_ldexp_f32", 2)], b"a translation");
        assert_eq!(Entry::decode(&e.encode()), Some(e.clone()));
        let bytes = e.encode();
        // damage anywhere in it, a cut, an extension, another format
        for at in [0, 5, 20, bytes.len() / 2, bytes.len() - 1] {
            let mut bad = bytes.clone();
            bad[at] ^= 1;
            assert_eq!(Entry::decode(&bad), None, "a flipped byte at {at}");
        }
        assert_eq!(Entry::decode(&bytes[..bytes.len() - 1]), None);
        assert_eq!(Entry::decode(&[&bytes[..], b"x"].concat()), None);
        assert_eq!(Entry::decode(b"VSH1 old format"), None);
        assert_eq!(Entry::decode(b""), None);
    }

    #[test]
    fn the_key_is_made_of_the_settings_and_the_input_and_nothing_else() {
        let ctx = |schema, loop_limit: &[u8], wide: &[u8]| {
            context_key(schema, &[("loop limit", loop_limit), ("VICTORY_WAVE_WIDE", wide)])
        };
        let base = ctx(1, b"4096", b"");
        // the same, the same
        assert_eq!(base, ctx(1, b"4096", b""));
        assert_eq!(entry_key(&base, b"console shader"), entry_key(&base, b"console shader"));
        // each setting, the schema, and the input change it
        assert_ne!(base, ctx(2, b"4096", b""));
        assert_ne!(base, ctx(1, b"16", b""));
        assert_ne!(base, ctx(1, b"4096", b"1"));
        assert_ne!(entry_key(&base, b"console shader"), entry_key(&base, b"another shader"));
        assert_ne!(entry_key(&base, b"console shader"), entry_key(&ctx(1, b"16", b""), b"console shader"));
        // a name and a value do not run into each other
        assert_ne!(
            context_key(1, &[("ab", b"c".as_slice())]),
            context_key(1, &[("a", b"bc".as_slice())])
        );
        assert_ne!(
            context_key(1, &[("a", b"".as_slice()), ("b", b"".as_slice())]),
            context_key(1, &[("a", b"b".as_slice())])
        );
    }

    #[test]
    fn standing_follows_the_rules_the_entry_used() {
        const T1: &[(&str, u8)] = &[("core", 1), ("pass:io", 2), ("valu:v_ldexp_f32", 3), ("valu:v_frexp_*", 4)];
        let e = entry("src 1", &[("core", 1), ("valu:v_ldexp_f32", 3)], b"x");
        assert_eq!(standing(&e, "src 1", &translator(T1)), Standing::Verified);
        // another source; an edit to rules the shader did not use (io, frexp)
        const T2: &[(&str, u8)] = &[("core", 1), ("pass:io", 9), ("valu:v_ldexp_f32", 3), ("valu:v_frexp_*", 9)];
        assert_eq!(standing(&e, "src 2", &translator(T2)), Standing::Unchanged);
        // a rule it used moved
        const T3: &[(&str, u8)] = &[("core", 1), ("pass:io", 2), ("valu:v_ldexp_f32", 9)];
        assert_eq!(standing(&e, "src 3", &translator(T3)), Standing::Suspect);
        // core moved: every entry is suspect
        const T4: &[(&str, u8)] = &[("core", 9), ("valu:v_ldexp_f32", 3)];
        assert_eq!(standing(&e, "src 4", &translator(T4)), Standing::Suspect);
        // a rule it used is gone
        const T5: &[(&str, u8)] = &[("core", 1)];
        assert_eq!(standing(&e, "src 5", &translator(T5)), Standing::Suspect);
    }

    fn setup() -> (tempfile::TempDir, Cache, CacheKey) {
        let dir = tempfile::tempdir().unwrap();
        let cache = Cache::open(dir.path().join("c")).unwrap();
        let key = entry_key(&context_key(1, &[]), b"a shader");
        (dir, cache, key)
    }

    const NEVER: fn() -> Result<(Vec<u8>, Rules), String> = || panic!("converted when it had no need to");

    #[test]
    fn an_unrelated_change_costs_a_restamp_and_no_conversion() {
        // (the entry used core and a; b changed)
        const T2: &[(&str, u8)] = &[("core", 1), ("a", 2), ("b", 9)];
        let (_dir, cache, key) = setup();
        store(&cache, &key, "src 1", entry("", &[("core", 1), ("a", 2)], b"").rules, b"out".to_vec()).unwrap();
        let e = read(&cache, &key).unwrap();
        assert_eq!(revalidate(&cache, &key, e, "src 2", &translator(T2), NEVER).0, Revalidated::Restamped);
        let e = read(&cache, &key).unwrap();
        assert_eq!((e.verified.as_str(), e.payload.as_slice()), ("src 2", &b"out"[..]));
        // and it is then this build's
        assert_eq!(standing(&e, "src 2", &translator(T2)), Standing::Verified);
        assert_eq!(revalidate(&cache, &key, e, "src 2", &translator(T2), NEVER).0, Revalidated::Same);
    }

    #[test]
    fn a_related_change_converts_again_and_only_a_different_output_replaces_the_entry() {
        const T2: &[(&str, u8)] = &[("core", 1), ("a", 9)];
        let (_dir, cache, key) = setup();
        let stamp = entry("", &[("core", 1), ("a", 2)], b"").rules;
        store(&cache, &key, "src 1", stamp.clone(), b"out".to_vec()).unwrap();
        let old = read(&cache, &key).unwrap();
        let new_stamp = entry("", &[("core", 1), ("a", 9)], b"").rules;

        // the same output: kept, restamped with the rules it went through now
        let (same, payload) = revalidate(&cache, &key, old, "src 2", &translator(T2), || Ok((b"out".to_vec(), new_stamp.clone())));
        assert_eq!((same, payload.as_slice()), (Revalidated::Same, &b"out"[..]));
        let e = read(&cache, &key).unwrap();
        assert_eq!((e.verified.as_str(), e.payload.as_slice(), &e.rules), ("src 2", &b"out"[..], &new_stamp));

        // a different output replaces it
        let old = e;
        const T3: &[(&str, u8)] = &[("core", 1), ("a", 10)];
        let (changed, payload) = revalidate(&cache, &key, old, "src 3", &translator(T3), || {
            Ok((b"different".to_vec(), entry("", &[("core", 1), ("a", 10)], b"").rules))
        });
        assert_eq!((changed, payload.as_slice()), (Revalidated::Changed, &b"different"[..]));
        assert_eq!(read(&cache, &key).unwrap().payload, b"different");
    }

    #[test]
    fn an_entry_whose_conversion_now_fails_is_kept() {
        const T2: &[(&str, u8)] = &[("core", 2)];
        let (_dir, cache, key) = setup();
        store(&cache, &key, "src 1", entry("", &[("core", 1)], b"").rules, b"out".to_vec()).unwrap();
        let old = read(&cache, &key).unwrap();
        let (r, payload) = revalidate(&cache, &key, old.clone(), "src 2", &translator(T2), || Err("unsupported".to_owned()));
        assert_eq!((r, payload.as_slice()), (Revalidated::Failed, &b"out"[..]));
        assert_eq!(read(&cache, &key), Some(old));
    }

    #[test]
    fn a_damaged_entry_reads_as_missing() {
        let (_dir, cache, key) = setup();
        store(&cache, &key, "src 1", Vec::new(), b"out".to_vec()).unwrap();
        cache.write(&key, b"VSE2 broken").unwrap();
        assert_eq!(read(&cache, &key), None);
    }

    fn ctx<'a>(source: &'a str, current: &'a dyn Fn(&str) -> Option<[u8; 32]>, mode: Mode) -> Context<'a> {
        Context { source, current, mode, verify_all: false }
    }

    fn made(payload: &'static [u8], rules: &'static [(&'static str, u8)]) -> impl FnOnce() -> Result<(Vec<u8>, Rules), String> {
        move || Ok((payload.to_vec(), entry("", rules, b"").rules))
    }

    #[test]
    fn a_shader_is_converted_once_and_then_served_from_its_entry() {
        const T1: &[(&str, u8)] = &[("core", 1)];
        let (_dir, cache, key) = setup();
        let t = translator(T1);
        let first = serve(&cache, &key, &ctx("src 1", &t, Mode::Lazy), made(b"out", T1)).unwrap();
        assert_eq!((first.payload.as_slice(), first.how), (&b"out"[..], How::New));
        let again = serve(&cache, &key, &ctx("src 1", &t, Mode::Lazy), NEVER).unwrap();
        assert_eq!((again.payload.as_slice(), again.how), (&b"out"[..], How::Hit));
        // a conversion that fails with no entry stores nothing
        let other = entry_key(&context_key(1, &[]), b"another");
        assert!(serve(&cache, &other, &ctx("src 1", &t, Mode::Lazy), || Err("no".to_owned())).is_err());
        assert!(read(&cache, &other).is_none());
    }

    #[test]
    fn another_converter_build_is_served_the_entry_and_revalidated_as_the_mode_says() {
        // rules: a used by the shader, b not
        const T1: &[(&str, u8)] = &[("core", 1), ("a", 2), ("b", 3)];
        const UNRELATED: &[(&str, u8)] = &[("core", 1), ("a", 2), ("b", 9)];
        const RELATED: &[(&str, u8)] = &[("core", 1), ("a", 7), ("b", 3)];
        let used: &[(&str, u8)] = &[("core", 1), ("a", 2)];
        let seed = || {
            let (dir, cache, key) = setup();
            serve(&cache, &key, &ctx("src 1", &translator(T1), Mode::Lazy), made(b"out", used)).unwrap();
            (dir, cache, key)
        };

        // an unrelated edit: Lazy hands it to the caller's background pass, Eager restamps it
        // with no conversion, and it is this build's from then on
        let (_d, cache, key) = seed();
        let t = translator(UNRELATED);
        let lazy = serve(&cache, &key, &ctx("src 2", &t, Mode::Lazy), NEVER).unwrap();
        assert_eq!((lazy.payload.as_slice(), lazy.how), (&b"out"[..], How::Pending));
        let trusted = serve(&cache, &key, &ctx("src 2", &t, Mode::Trust), NEVER).unwrap();
        assert_eq!(trusted.how, How::Trusted);
        let eager = serve(&cache, &key, &ctx("src 2", &t, Mode::Eager), NEVER).unwrap();
        assert_eq!(eager.how, How::Revalidated(Revalidated::Restamped));
        assert_eq!(serve(&cache, &key, &ctx("src 2", &t, Mode::Eager), NEVER).unwrap().how, How::Hit);

        // an edit to a rule it used: converted again; the same output keeps the entry
        let (_d, cache, key) = seed();
        let t = translator(RELATED);
        let lazy = serve(&cache, &key, &ctx("src 2", &t, Mode::Lazy), NEVER).unwrap();
        assert_eq!((lazy.payload.as_slice(), lazy.how), (&b"out"[..], How::Pending));
        let same = serve(&cache, &key, &ctx("src 2", &t, Mode::Eager), made(b"out", &[("core", 1), ("a", 7)])).unwrap();
        assert_eq!((same.payload.as_slice(), same.how), (&b"out"[..], How::Revalidated(Revalidated::Same)));
        assert_eq!(serve(&cache, &key, &ctx("src 2", &t, Mode::Eager), NEVER).unwrap().how, How::Hit);

        // ... and another output replaces it, and is what is served
        let (_d, cache, key) = seed();
        let changed = serve(&cache, &key, &ctx("src 2", &t, Mode::Eager), made(b"new", &[("core", 1), ("a", 7)])).unwrap();
        assert_eq!((changed.payload.as_slice(), changed.how), (&b"new"[..], How::Revalidated(Revalidated::Changed)));
        let after = serve(&cache, &key, &ctx("src 2", &t, Mode::Lazy), NEVER).unwrap();
        assert_eq!((after.payload.as_slice(), after.how), (&b"new"[..], How::Hit));

        // a conversion that now fails serves the old payload and keeps the entry
        let (_d, cache, key) = seed();
        let failed = serve(&cache, &key, &ctx("src 2", &t, Mode::Eager), || Err("unsupported".to_owned())).unwrap();
        assert_eq!((failed.payload.as_slice(), failed.how), (&b"out"[..], How::Revalidated(Revalidated::Failed)));
        assert_eq!(read(&cache, &key).unwrap().payload, b"out");
    }

    #[test]
    fn verifying_all_converts_entries_that_look_current() {
        const T1: &[(&str, u8)] = &[("core", 1)];
        let (_dir, cache, key) = setup();
        let t = translator(T1);
        serve(&cache, &key, &ctx("src 1", &t, Mode::Lazy), made(b"out", T1)).unwrap();
        let all = |mode| Context { source: "src 1", current: &t, mode, verify_all: true };
        // the translator's output moved under an entry whose fingerprints did not (a rule
        // that is not noted where it is used): only this check finds it
        let found = serve(&cache, &key, &all(Mode::Eager), made(b"moved", T1)).unwrap();
        assert_eq!((found.payload.as_slice(), found.how), (&b"moved"[..], How::Revalidated(Revalidated::Changed)));
        let steady = serve(&cache, &key, &all(Mode::Eager), made(b"moved", T1)).unwrap();
        assert_eq!(steady.how, How::Revalidated(Revalidated::Same));
        let lazy = serve(&cache, &key, &all(Mode::Lazy), NEVER).unwrap();
        assert_eq!(lazy.how, How::Pending);
        let entry = read(&cache, &key).unwrap();
        assert_eq!(reconvert(&cache, &key, entry, "src 1", made(b"again", T1)).0, Revalidated::Changed);
    }
}
