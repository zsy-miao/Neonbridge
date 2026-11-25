//! Translated shaders kept on disk (decision 0003): under the configured
//! cache folder, keyed by the container's content and by the VIctory build
//! that translated it (the BLAKE3 of the translator's sources), so a rebuilt
//! translator never serves an old translation. Entries go through
//! `victory-pipeline`'s content-addressed cache, which writes them
//! atomically; an entry that does not read back is translated again.
//!
//! The entry is our own encoding of [`HostShader`]: a field added to it
//! must be added to `encode` and `decode` too (entries of older builds
//! are ignored anyway).

use std::collections::HashSet;
use std::path::PathBuf;
use std::sync::atomic::{AtomicBool, AtomicU64, Ordering};
use std::sync::{Mutex, OnceLock};

pub use victory_pipeline::Cache;
use victory_pipeline::{CacheKey, Converter};
use victory_shader::emit::{HostShader, StaticSampler, UserData};
use victory_shader::resource::RootIndexedView;
use victory_shader::{HwStage, RootType};

static CACHE: OnceLock<Cache> = OnceLock::new();
static HITS: AtomicU64 = AtomicU64::new(0);
static MISSES: AtomicU64 = AtomicU64::new(0);

/// Keeps translated shaders in `dir` from now on.
pub fn use_shader_cache(dir: PathBuf) {
    match Cache::open(&dir) {
        Ok(cache) => {
            let _ = CACHE.set(cache);
            let _ = USED_DIR.set(dir);
        }
        Err(e) => tracing::warn!(error = %e, "no shader cache"),
    }
}

/// The containers runs translated, one per line by the hash the log names
/// them by (`hash=`), in the cache folder: `victory translate --used-only`
/// warms just those.
pub const USED_FILE: &str = "used-shaders.txt";
static USED_DIR: OnceLock<PathBuf> = OnceLock::new();

/// Adds `container` to the used list if no run listed it yet.
fn note_used(container: &[u8]) {
    use std::io::Write;
    static USED: Mutex<Option<(HashSet<String>, Option<std::fs::File>)>> = Mutex::new(None);
    let Some(dir) = USED_DIR.get() else {
        return;
    };
    let mut used = USED.lock().unwrap_or_else(|e| e.into_inner());
    let (listed, file) = used.get_or_insert_with(|| {
        let path = dir.join(USED_FILE);
        let listed = std::fs::read_to_string(&path)
            .unwrap_or_default()
            .lines()
            .map(str::to_owned)
            .collect();
        let file = std::fs::OpenOptions::new()
            .create(true)
            .append(true)
            .open(&path)
            .ok();
        (listed, file)
    });
    let hex = short_hash(container);
    if let Some(f) = file
        && !listed.contains(&hex)
    {
        // shortcut: a failed append only leaves the shader to the full warm-up.
        let _ = writeln!(f, "{hex}");
        listed.insert(hex);
    }
}

/// A container's hash as the log names it: the first 16 hex digits of its
/// BLAKE3.
pub fn short_hash(container: &[u8]) -> String {
    blake3::hash(container).to_hex()[..16].to_owned()
}

/// The containers in the used list in `dir`, `None` if there is none yet.
pub fn used(dir: &std::path::Path) -> Option<HashSet<String>> {
    let text = std::fs::read_to_string(dir.join(USED_FILE)).ok()?;
    let hash = |l: &str| l.len() == 16 && l.bytes().all(|b| b.is_ascii_hexdigit());
    Some(text.lines().map(str::trim).filter(|l| hash(l)).map(str::to_owned).collect())
}

/// Translates `container` into `cache` as a run of this executable would,
/// unless its entry is there already (`victory translate --write-cache`):
/// `Ok(true)` if it was translated now.
pub fn warm(cache: &Cache, container: &[u8]) -> Result<bool, String> {
    let translate = |c: &[u8]| victory_shader::emit::translate(c).map_err(|e| e.to_string());
    warm_with(cache, build(), last(), container, translate)
}

fn warm_with(
    cache: &Cache,
    build: &[u8; 32],
    last: &[u8; 32],
    container: &[u8],
    translate: impl Fn(&[u8]) -> Result<HostShader, String>,
) -> Result<bool, String> {
    let input = [&build[..], container].concat();
    if cache.path_for(&CacheKey::new(&KeySpec, &input)).is_file() {
        return Ok(false);
    }
    // As [`cached`] stores a miss, without reading the entry back: nothing
    // here uses the shader, and opening the file just written cost an
    // eighth of a cold warm-up. (The failure reads as the converter's.)
    let shader = translate(container)
        .map_err(|e| format!("converter {} v{}: {e}", KeySpec.id(), KeySpec.version()))?;
    cache
        .replace(&KeySpec, &input, &encode(&shader))
        .map_err(|e| format!("{e:#}"))?;
    remember(cache, last, container, &input);
    Ok(true)
}

/// Shaders found in the cache and translated since the start.
pub(crate) fn counts() -> (u64, u64) {
    (HITS.load(Ordering::Relaxed), MISSES.load(Ordering::Relaxed))
}

/// Version of [`encode`] / [`decode`]'s on-disk format: bump it whenever it
/// changes (the translator's source hash does not cover this file).
const CACHE_SCHEMA_VERSION: u32 = 1;

/// What identifies this build: the BLAKE3 of the translator's sources (or of
/// our executable with `VICTORY_SHADER_CACHE_KEY=exe`) and of the
/// translation settings it runs with (the loop limit, which a diagnostics
/// build can change at run time). A rebuild that leaves `victory-shader`
/// alone, or a diagnostics build beside a release one, keeps the cache.
fn build() -> &'static [u8; 32] {
    static BUILD: OnceLock<[u8; 32]> = OnceLock::new();
    BUILD.get_or_init(|| {
        let exe = if std::env::var_os("VICTORY_SHADER_CACHE_KEY").is_some_and(|v| v == "exe") {
            std::env::current_exe().and_then(std::fs::read)
        } else {
            Ok([&CACHE_SCHEMA_VERSION.to_le_bytes()[..], victory_shader::SOURCE_HASH.as_bytes()].concat())
        };
        let limit = victory_shader::emit::loop_limit();
        if limit != victory_shader::emit::DEFAULT_LOOP_LIMIT {
            tracing::warn!(
                limit,
                "VICTORY_SHADER_LOOP_LIMIT: translated loops stop sooner"
            );
        }
        match exe {
            Ok(mut bytes) => {
                bytes.extend(settings());
                *blake3::hash(&bytes).as_bytes()
            }
            Err(e) => {
                // Without it no entry can be trusted: a fresh key per run.
                tracing::warn!(error = %e, "cannot read our executable; shader cache unused");
                let now = std::time::SystemTime::now()
                    .duration_since(std::time::UNIX_EPOCH)
                    .unwrap_or_default();
                *blake3::hash(&now.as_nanos().to_le_bytes()).as_bytes()
            }
        }
    })
}

/// The translation settings besides the executable that a build's entries
/// depend on.
fn settings() -> Vec<u8> {
    let mut bytes = victory_shader::emit::loop_limit().to_le_bytes().to_vec();
    // Which group barriers wait is part of the translation too.
    bytes.push(victory_shader::emit::group_sync());
    // So are the switches that pick the form of a wave64 program:
    // without them here a run with `VICTORY_WAVE_WIDE=0` was
    // served the translations an earlier run of the same
    // executable had cached with the switch unset.
    for name in [
        "VICTORY_WAVE_WIDE",
        "VICTORY_WAVE_DEEP_SKIPS",
        "VICTORY_WAVE_GROUP_BARRIERS",
        "VICTORY_WAVE_OWN_BITS",
        "VICTORY_F16_NEAREST",
        // Whether shaders mark the heap slots they take.
        "VICTORY_HEAP_USAGE",
        // Whether the ray-tracing node test always misses.
        "VICTORY_RT_MISS",
    ] {
        bytes.extend_from_slice(name.as_bytes());
        bytes.push(b'=');
        if let Some(value) = std::env::var_os(name) {
            bytes.extend_from_slice(value.as_encoded_bytes());
        }
        bytes.push(0);
    }
    // And which clean-up passes ran over the translated code.
    bytes.extend_from_slice(&victory_shader::emit::shader_opt().to_le_bytes());
    // And what the host GPU told the translator (its widest wave, a capped
    // view heap, rasterizer-ordered views): an offline warm-up made without them must not serve a run.
    bytes.extend_from_slice(&victory_shader::emit::host_max_wave().to_le_bytes());
    bytes.extend_from_slice(&victory_shader::emit::view_heap_limit().to_le_bytes());
    bytes.push(u8::from(victory_shader::emit::host_rovs()));
    bytes
}

/// The key of a container's last translation that worked, under any build
/// of the same settings.
fn last() -> &'static [u8; 32] {
    static LAST: OnceLock<[u8; 32]> = OnceLock::new();
    LAST.get_or_init(|| *blake3::hash(&[b"last".as_slice(), &settings()].concat()).as_bytes())
}

/// The translation of a console shader container, from the cache when
/// there is one.
pub(crate) fn host_shader(container: &[u8]) -> Result<HostShader, String> {
    let translate = |c: &[u8]| victory_shader::emit::translate(c).map_err(|e| e.to_string());
    note_used(container);
    match CACHE.get() {
        Some(cache) => {
            let result = cached(cache, build(), last(), container, translate);
            or_last_worked(cache, last(), container, result)
        }
        None => translate(container),
    }
}

/// A container the current translator fails on gets its last translation
/// that worked, if an earlier build made one: it draws as before the
/// change instead of not at all.
fn or_last_worked(
    cache: &Cache,
    last: &[u8; 32],
    container: &[u8],
    result: Result<HostShader, String>,
) -> Result<HostShader, String> {
    result.or_else(|error| {
        let shader = last_worked(cache, last, container).ok_or_else(|| error.clone())?;
        tracing::warn!(error, "shader does not translate; using its last translation that worked");
        HITS.fetch_add(1, Ordering::Relaxed);
        Ok(shader)
    })
}

/// Where a container's last translation that worked is: the entry's key.
struct LastSpec;

impl Converter for LastSpec {
    fn id(&self) -> &'static str {
        "shader.dxil.last"
    }
    fn version(&self) -> u32 {
        1
    }
    fn convert(&self, _input: &[u8]) -> anyhow::Result<Vec<u8>> {
        unreachable!("LastSpec only keys Cache::replace")
    }
}

/// Notes the entry made from `input` as `container`'s last translation that
/// worked.
fn remember(cache: &Cache, last: &[u8; 32], container: &[u8], input: &[u8]) {
    let entry = CacheKey::new(&KeySpec, input).to_hex();
    // shortcut: a failed write only loses the fallback for this shader.
    let _ = cache.replace(&LastSpec, &[&last[..], container].concat(), entry.as_bytes());
}

/// `container`'s last translation that worked, by an earlier build.
fn last_worked(cache: &Cache, last: &[u8; 32], container: &[u8]) -> Option<HostShader> {
    let pointer = CacheKey::new(&LastSpec, &[&last[..], container].concat());
    let entry = std::fs::read_to_string(cache.path_for(&pointer)).ok()?;
    let entry = CacheKey::from_hex(entry.trim())?;
    decode(&std::fs::read(cache.path_for(&entry)).ok()?)
}

/// The key of a translation entry: [`Translate`]'s id and version.
struct KeySpec;

impl Converter for KeySpec {
    fn id(&self) -> &'static str {
        "shader.dxil"
    }
    fn version(&self) -> u32 {
        // The build fingerprint in the input does the versioning.
        1
    }
    fn convert(&self, _input: &[u8]) -> anyhow::Result<Vec<u8>> {
        unreachable!("KeySpec only computes keys")
    }
}

/// The cache entry's converter: the build and the container in, the
/// encoded translation out. `ran` tells a miss from a hit.
struct Translate<F> {
    translate: F,
    ran: AtomicBool,
}

impl<F: Fn(&[u8]) -> Result<HostShader, String> + Send + Sync> Converter for Translate<F> {
    fn id(&self) -> &'static str {
        KeySpec.id()
    }
    fn version(&self) -> u32 {
        KeySpec.version()
    }
    fn convert(&self, input: &[u8]) -> anyhow::Result<Vec<u8>> {
        self.ran.store(true, Ordering::Relaxed);
        let shader = (self.translate)(&input[32..]).map_err(anyhow::Error::msg)?;
        Ok(encode(&shader))
    }
}

fn cached(
    cache: &Cache,
    build: &[u8; 32],
    last: &[u8; 32],
    container: &[u8],
    translate: impl Fn(&[u8]) -> Result<HostShader, String> + Send + Sync,
) -> Result<HostShader, String> {
    let input = [&build[..], container].concat();
    let converter = Translate {
        translate,
        ran: AtomicBool::new(false),
    };
    for _ in 0..2 {
        let path = cache
            .get_or_convert(&converter, &input)
            .map_err(|e| format!("{e:#}"))?;
        if let Some(shader) = std::fs::read(&path).ok().and_then(|b| decode(&b)) {
            let (counter, what) = if converter.ran.load(Ordering::Relaxed) {
                remember(cache, last, container, &input);
                (&MISSES, "translated and cached")
            } else {
                (&HITS, "from the cache")
            };
            counter.fetch_add(1, Ordering::Relaxed);
            tracing::debug!(path = %path.display(), "shader {what}");
            return Ok(shader);
        }
        tracing::debug!(path = %path.display(), "unreadable shader cache entry; translating again");
        let _ = std::fs::remove_file(&path);
    }
    MISSES.fetch_add(1, Ordering::Relaxed);
    (converter.translate)(container)
}

const MAGIC: &[u8; 4] = b"VSH1";
const STAGES: [HwStage; 8] = [
    HwStage::Ls,
    HwStage::Hs,
    HwStage::Es,
    HwStage::Gs,
    HwStage::Vs,
    HwStage::Ps,
    HwStage::Ms,
    HwStage::Cs,
];

fn root_code(r: RootType) -> u32 {
    match r {
        RootType::Constants => 0,
        RootType::Descriptor => 1,
        RootType::Table => 2,
        RootType::ImmediateSampler => 3,
        RootType::ResourceHeap => 4,
        RootType::SamplerHeap => 5,
        RootType::Other(x) => 6 + x,
    }
}

fn root_type(code: u32) -> RootType {
    match code {
        0 => RootType::Constants,
        1 => RootType::Descriptor,
        2 => RootType::Table,
        3 => RootType::ImmediateSampler,
        4 => RootType::ResourceHeap,
        5 => RootType::SamplerHeap,
        x => RootType::Other(x - 6),
    }
}

/// Little-endian words and length-prefixed lists.
fn encode(s: &HostShader) -> Vec<u8> {
    let mut out = MAGIC.to_vec();
    let word = |out: &mut Vec<u8>, v: u32| out.extend(v.to_le_bytes());
    let stage = STAGES.iter().position(|&x| x == s.stage).unwrap_or(0) as u32;
    word(&mut out, stage);
    word(&mut out, s.dxil.len() as u32);
    out.extend(&s.dxil);
    word(&mut out, s.user_data.len() as u32);
    for u in &s.user_data {
        for v in [u.index, u32::from(u.slot), root_code(u.root), u.dword] {
            word(&mut out, v);
        }
    }
    word(&mut out, s.samplers.len() as u32);
    for sampler in &s.samplers {
        for &v in sampler {
            word(&mut out, v);
        }
    }
    word(&mut out, s.static_samplers.len() as u32);
    for t in &s.static_samplers {
        let [u, v, w] = t.address;
        let words = [
            t.filter,
            u,
            v,
            w,
            t.mip_lod_bias.to_bits(),
            t.max_anisotropy,
            t.comparison,
            t.border,
            t.min_lod.to_bits(),
            t.max_lod.to_bits(),
        ];
        for v in words {
            word(&mut out, v);
        }
    }
    for slots in [&s.root_cbvs, &s.root_srvs, &s.root_uavs] {
        word(&mut out, slots.len() as u32);
        out.extend(slots);
    }
    word(&mut out, s.root_indexed_views.len() as u32);
    for view in &s.root_indexed_views {
        for value in [
            u32::from(view.table_slot),
            u32::from(view.constant_slot),
            view.constant_dword,
            view.byte_offset,
            u32::from(view.shift),
        ] {
            word(&mut out, value);
        }
    }
    out
}

fn decode(bytes: &[u8]) -> Option<HostShader> {
    struct Reader<'a>(&'a [u8]);
    impl<'a> Reader<'a> {
        fn take(&mut self, n: usize) -> Option<&'a [u8]> {
            let (head, rest) = self.0.split_at_checked(n)?;
            self.0 = rest;
            Some(head)
        }
        fn word(&mut self) -> Option<u32> {
            Some(u32::from_le_bytes(self.take(4)?.try_into().ok()?))
        }
        fn list(&mut self) -> Option<Vec<u8>> {
            let n = self.word()? as usize;
            Some(self.take(n)?.to_vec())
        }
    }
    let mut r = Reader(bytes);
    if r.take(4)? != MAGIC {
        return None;
    }
    let stage = *STAGES.get(r.word()? as usize)?;
    let dxil = r.list()?;
    let n = r.word()? as usize;
    let mut user_data = Vec::with_capacity(n.min(256));
    for _ in 0..n {
        user_data.push(UserData {
            index: r.word()?,
            slot: u8::try_from(r.word()?).ok()?,
            root: root_type(r.word()?),
            dword: r.word()?,
        });
    }
    let n = r.word()? as usize;
    let mut samplers = Vec::with_capacity(n.min(256));
    for _ in 0..n {
        samplers.push([r.word()?, r.word()?, r.word()?, r.word()?]);
    }
    let n = r.word()? as usize;
    let mut static_samplers = Vec::with_capacity(n.min(256));
    for _ in 0..n {
        static_samplers.push(StaticSampler {
            filter: r.word()?,
            address: [r.word()?, r.word()?, r.word()?],
            mip_lod_bias: f32::from_bits(r.word()?),
            max_anisotropy: r.word()?,
            comparison: r.word()?,
            border: r.word()?,
            min_lod: f32::from_bits(r.word()?),
            max_lod: f32::from_bits(r.word()?),
        });
    }
    let root_cbvs = r.list()?;
    let root_srvs = r.list()?;
    let root_uavs = r.list()?;
    let n = r.word()? as usize;
    let mut root_indexed_views = Vec::with_capacity(n.min(256));
    for _ in 0..n {
        root_indexed_views.push(RootIndexedView {
            table_slot: u8::try_from(r.word()?).ok()?,
            constant_slot: u8::try_from(r.word()?).ok()?,
            constant_dword: r.word()?,
            byte_offset: r.word()?,
            shift: u8::try_from(r.word()?).ok().filter(|&s| s < 32)?,
        });
    }
    r.0.is_empty().then_some(HostShader {
        stage,
        dxil,
        user_data,
        samplers,
        static_samplers,
        root_cbvs,
        root_srvs,
        root_uavs,
        root_indexed_views,
    })
}

#[cfg(test)]
mod tests {
    use super::*;

    const LAST: [u8; 32] = [9; 32];

    #[test]
    fn the_used_list_reads_digests_and_skips_damage() {
        let dir = tempfile::tempdir().unwrap();
        assert_eq!(used(dir.path()), None);
        let a = short_hash(b"a container");
        std::fs::write(dir.path().join(USED_FILE), format!("{a}\nhalf a li\n")).unwrap();
        assert_eq!(used(dir.path()), Some(HashSet::from([a])));
    }

    #[test]
    fn a_failing_translation_falls_back_to_the_last_that_worked() {
        let dir = tempfile::tempdir().unwrap();
        let cache = Cache::open(dir.path()).unwrap();
        let container = b"a console container".as_slice();
        let fail = |_: &[u8]| Err::<HostShader, _>("unsupported".to_owned());
        let ok = |_: &[u8]| Ok(shader());
        let run = |build: &[u8; 32], last: &[u8; 32], translate: fn(&[u8]) -> Result<HostShader, String>| {
            or_last_worked(&cache, last, container, cached(&cache, build, last, container, translate))
        };
        // Nothing worked yet: the failure stands.
        assert!(run(&[1; 32], &LAST, fail).is_err());
        assert_eq!(run(&[1; 32], &LAST, ok), Ok(shader()));
        // A later build that fails gets the earlier translation; without the
        // fallback (the offline warm-up) the failure is reported.
        assert_eq!(run(&[2; 32], &LAST, fail), Ok(shader()));
        assert!(cached(&cache, &[2; 32], &LAST, container, fail).is_err());
        // Other settings (another last key) do not.
        assert!(run(&[2; 32], &[3; 32], fail).is_err());
    }

    #[test]
    fn a_warmed_entry_serves_a_run_and_is_the_last_that_worked() {
        let dir = tempfile::tempdir().unwrap();
        let cache = Cache::open(dir.path()).unwrap();
        let container = b"a console container".as_slice();
        let fail = |_: &[u8]| Err::<HostShader, _>("unsupported".to_owned());
        let ok = |_: &[u8]| Ok(shader());
        assert_eq!(warm_with(&cache, &[1; 32], &LAST, container, ok), Ok(true));
        // Already there: not translated again.
        assert_eq!(warm_with(&cache, &[1; 32], &LAST, container, fail), Ok(false));
        // A run of that build reads it, and a later build that fails gets it.
        assert_eq!(cached(&cache, &[1; 32], &LAST, container, fail), Ok(shader()));
        let later = cached(&cache, &[2; 32], &LAST, container, fail);
        assert_eq!(or_last_worked(&cache, &LAST, container, later), Ok(shader()));
        // A failure reads as the converter's and stores nothing.
        let failed = warm_with(&cache, &[3; 32], &LAST, b"other", fail);
        assert_eq!(failed, Err("converter shader.dxil v1: unsupported".to_owned()));
        assert!(cached(&cache, &[3; 32], &LAST, b"other", fail).is_err());
    }

    fn shader() -> HostShader {
        HostShader {
            stage: HwStage::Ps,
            dxil: b"DXBC and the rest".to_vec(),
            user_data: vec![UserData {
                index: 8,
                slot: 1,
                root: RootType::Table,
                dword: 0,
            }],
            samplers: vec![[0x400, 0xc0_0000, 0x18f0_0000, 0]],
            static_samplers: vec![StaticSampler::from_words([
                0x400,
                0xc0_0000,
                0x18f0_0000,
                0,
            ])],
            root_cbvs: vec![2],
            root_srvs: vec![0, 3],
            root_uavs: vec![1],
            root_indexed_views: vec![RootIndexedView {
                table_slot: 1,
                constant_slot: 2,
                constant_dword: 3,
                byte_offset: 32,
                shift: 5,
            }],
        }
    }

    #[test]
    fn translations_come_back_from_the_cache() {
        let dir = tempfile::tempdir().unwrap();
        let cache = Cache::open(dir.path()).unwrap();
        let build = [7u8; 32];
        let container = b"a console container".as_slice();
        let runs = AtomicU64::new(0);
        let translate = |_: &[u8]| {
            runs.fetch_add(1, Ordering::Relaxed);
            Ok(shader())
        };
        assert_eq!(cached(&cache, &build, &LAST, container, translate), Ok(shader()));
        assert_eq!(cached(&cache, &build, &LAST, container, translate), Ok(shader()));
        assert_eq!(runs.load(Ordering::Relaxed), 1, "the second came from disk");

        // Another build translates again.
        assert_eq!(cached(&cache, &[8; 32], &LAST, container, translate), Ok(shader()));
        assert_eq!(runs.load(Ordering::Relaxed), 2);

        // A damaged entry is a miss, translated and stored again.
        for e in walk(dir.path()) {
            std::fs::write(e, b"VSH1 broken").unwrap();
        }
        assert_eq!(cached(&cache, &build, &LAST, container, translate), Ok(shader()));
        assert_eq!(runs.load(Ordering::Relaxed), 3);
        assert_eq!(cached(&cache, &build, &LAST, container, translate), Ok(shader()));
        assert_eq!(runs.load(Ordering::Relaxed), 3);

        // Failures are not stored.
        let fail = |_: &[u8]| Err::<HostShader, _>("unsupported".to_owned());
        assert!(cached(&cache, &build, &LAST, b"other", fail).is_err());
    }

    fn walk(dir: &std::path::Path) -> Vec<PathBuf> {
        let mut files = Vec::new();
        for e in std::fs::read_dir(dir).unwrap().flatten() {
            if e.path().is_dir() {
                files.extend(walk(&e.path()));
            } else {
                files.push(e.path());
            }
        }
        files
    }

    #[test]
    fn entries_round_trip_and_reject_damage() {
        let bytes = encode(&shader());
        assert_eq!(decode(&bytes), Some(shader()));
        assert_eq!(decode(&bytes[..bytes.len() - 1]), None);
        assert_eq!(decode(&[bytes.as_slice(), &[0]].concat()), None);
        assert_eq!(decode(b"VSH0"), None);
    }
}
