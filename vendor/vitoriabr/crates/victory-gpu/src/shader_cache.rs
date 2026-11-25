//! Translated shaders kept on disk (decision 0003): under the configured
//! cache folder, keyed by the container's content and by the translator
//! that translated it (its source fingerprint and settings, see `build`), so
//! a changed translator never serves an old translation. Entries go through
//! `victory-pipeline`'s content-addressed cache, which writes them
//! atomically; an entry that does not read back is translated again.
//!
//! The entry is our own encoding of [`HostShader`]: a field added to it
//! must be added to `encode` and `decode` too (entries of older builds
//! are ignored anyway).

use std::path::PathBuf;
use std::sync::OnceLock;
use std::sync::atomic::{AtomicBool, AtomicU64, Ordering};

use victory_pipeline::{Cache, Converter};
use victory_shader::emit::{HostShader, StaticSampler, UserData};
use victory_shader::resource::RootIndexedView;
use victory_shader::{HwStage, RootType};

static CACHE: OnceLock<Cache> = OnceLock::new();
static HITS: AtomicU64 = AtomicU64::new(0);
static MISSES: AtomicU64 = AtomicU64::new(0);

/// Keeps translated shaders in `dir` from now on.
pub fn use_shader_cache(dir: PathBuf) {
    match Cache::open(dir) {
        Ok(cache) => {
            let _ = CACHE.set(cache);
        }
        Err(e) => tracing::warn!(error = %e, "no shader cache"),
    }
}

/// Shaders found in the cache and translated since the start.
pub(crate) fn counts() -> (u64, u64) {
    (HITS.load(Ordering::Relaxed), MISSES.load(Ordering::Relaxed))
}

/// What identifies the translation, besides the container: the shader
/// crate's fingerprint (its source, every switch it reads, the host facts the
/// device gave it, the loop limit, barrier waits and clean-up passes) and
/// this file's encoding of the result. Not the whole executable any more: a
/// rebuild that leaves the translator alone keeps its translations (each new
/// build used to translate the title's thousands of shaders again, which kept
/// the first minutes of every test run short of pipelines).
/// `VICTORY_SHADER_CACHE_KEY=exe` restores the old key (the executable's hash).
/// Worked out per lookup, so a translation made before the device said its
/// wave size is never filed under the later key.
fn build() -> [u8; 32] {
    static WARNED: std::sync::Once = std::sync::Once::new();
    let limit = victory_shader::emit::loop_limit();
    if limit != victory_shader::emit::DEFAULT_LOOP_LIMIT {
        WARNED.call_once(|| tracing::warn!(limit, "VICTORY_SHADER_LOOP_LIMIT: translated loops stop sooner"));
    }
    static EXE: OnceLock<Option<[u8; 32]>> = OnceLock::new();
    let exe = EXE.get_or_init(|| {
        if std::env::var_os("VICTORY_SHADER_CACHE_KEY").is_some_and(|v| v == "exe") {
            match std::env::current_exe().and_then(std::fs::read) {
                Ok(bytes) => Some(*blake3::hash(&bytes).as_bytes()),
                Err(e) => {
                    tracing::warn!(error = %e, "cannot read our executable; keying on the translator");
                    None
                }
            }
        } else {
            None
        }
    });
    let mut h = blake3::Hasher::new();
    h.update(b"victory shader cache key 2\0");
    if let Some(exe) = exe {
        h.update(exe);
    }
    h.update(&victory_shader::emit::translation_fingerprint());
    h.update(include_bytes!("shader_cache.rs"));
    *h.finalize().as_bytes()
}

/// The translation of a console shader container, from the cache when
/// there is one.
pub(crate) fn host_shader(container: &[u8]) -> Result<HostShader, String> {
    match CACHE.get() {
        Some(cache) => cached(cache, &build(), container, translate).map(|(shader, _)| shader),
        None => translate(container),
    }
}

fn translate(container: &[u8]) -> Result<HostShader, String> {
    victory_shader::emit::translate(container).map_err(|e| e.to_string())
}

/// Translates `container` into the cache before a run (`victory warm` /
/// `victory translate --warm`), so the run finds it there instead of
/// translating it while the title waits.
///
/// `Ok(true)`: translated and stored now; `Ok(false)`: it was cached
/// already. An `Err` is a shader that does not translate (a run drops its
/// draws as well), or one whose entry did not read back.
pub fn warm(container: &[u8]) -> Result<bool, String> {
    crate::descriptor::host::probe_view_limit();
    let cache = CACHE
        .get()
        .ok_or("no shader cache folder (use_shader_cache)")?;
    match cached(cache, &build(), container, translate)?.1 {
        Outcome::Hit => Ok(false),
        Outcome::Stored => Ok(true),
        Outcome::Unstored => Err("translated, but the cache entry did not read back".to_owned()),
    }
}

/// Where [`cached`]'s translation came from.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum Outcome {
    /// Read from the cache.
    Hit,
    /// Translated now and stored.
    Stored,
    /// Translated now; the entry would not read back, so nothing is stored.
    Unstored,
}

/// The cache entry's converter: the build and the container in, the
/// encoded translation out. `ran` tells a miss from a hit.
struct Translate<F> {
    translate: F,
    ran: AtomicBool,
}

impl<F: Fn(&[u8]) -> Result<HostShader, String> + Send + Sync> Converter for Translate<F> {
    fn id(&self) -> &'static str {
        "shader.dxil"
    }
    fn version(&self) -> u32 {
        // The build fingerprint in the input does the versioning.
        1
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
    container: &[u8],
    translate: impl Fn(&[u8]) -> Result<HostShader, String> + Send + Sync,
) -> Result<(HostShader, Outcome), String> {
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
            let (counter, what, outcome) = if converter.ran.load(Ordering::Relaxed) {
                (&MISSES, "translated and cached", Outcome::Stored)
            } else {
                (&HITS, "from the cache", Outcome::Hit)
            };
            counter.fetch_add(1, Ordering::Relaxed);
            tracing::debug!(path = %path.display(), "shader {what}");
            return Ok((shader, outcome));
        }
        tracing::debug!(path = %path.display(), "unreadable shader cache entry; translating again");
        let _ = std::fs::remove_file(&path);
    }
    MISSES.fetch_add(1, Ordering::Relaxed);
    (converter.translate)(container).map(|shader| (shader, Outcome::Unstored))
}

const MAGIC: &[u8; 4] = b"VSH3";
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
    for slots in [&s.root_cbvs, &s.root_srvs, &s.root_uavs, &s.written_root_uavs] {
        word(&mut out, slots.len() as u32);
        out.extend(slots);
    }
    word(&mut out, u32::from(s.writes_guest_memory));
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
    let written_root_uavs = r.list()?;
    let writes_guest_memory = match r.word()? {
        0 => false,
        1 => true,
        _ => return None,
    };
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
        written_root_uavs,
        writes_guest_memory,
        root_indexed_views,
    })
}

#[cfg(test)]
mod tests {
    use super::*;

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
            root_uavs: vec![1, 2],
            written_root_uavs: vec![2],
            writes_guest_memory: true,
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
        assert_eq!(
            cached(&cache, &build, container, translate),
            Ok((shader(), Outcome::Stored))
        );
        assert_eq!(
            cached(&cache, &build, container, translate),
            Ok((shader(), Outcome::Hit))
        );
        assert_eq!(runs.load(Ordering::Relaxed), 1, "the second came from disk");

        // Another build translates again.
        assert_eq!(
            cached(&cache, &[8; 32], container, translate),
            Ok((shader(), Outcome::Stored))
        );
        assert_eq!(runs.load(Ordering::Relaxed), 2);

        // A damaged entry is a miss, translated and stored again.
        for e in walk(dir.path()) {
            std::fs::write(e, b"VSH3 broken").unwrap();
        }
        assert_eq!(
            cached(&cache, &build, container, translate),
            Ok((shader(), Outcome::Stored))
        );
        assert_eq!(runs.load(Ordering::Relaxed), 3);
        assert_eq!(
            cached(&cache, &build, container, translate),
            Ok((shader(), Outcome::Hit))
        );
        assert_eq!(runs.load(Ordering::Relaxed), 3);

        // Failures are not stored.
        let fail = |_: &[u8]| Err::<HostShader, _>("unsupported".to_owned());
        assert!(cached(&cache, &build, b"other", fail).is_err());
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
        let mut read_only = shader();
        read_only.writes_guest_memory = false;
        assert_eq!(decode(&encode(&read_only)), Some(read_only));
        let mut stale = bytes.clone();
        stale[..4].copy_from_slice(b"VSH1");
        assert_eq!(decode(&stale), None);
        assert_eq!(decode(&bytes[..bytes.len() - 1]), None);
        assert_eq!(decode(&[bytes.as_slice(), &[0]].concat()), None);
        assert_eq!(decode(b"VSH0"), None);
    }
}
