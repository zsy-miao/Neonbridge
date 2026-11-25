//! **Opt-in in fx-cand**: this two-level cache runs only with `VICTORY_SHADER_CACHE_KEY=output`;
//! by default the cache of fx-final (`legacy`, keyed by the translator) is used, so a cache warmed
//! by that build stays valid. (`victory warm --incremental` and the revalidation switches below
//! do nothing without it.)
//!
//! Translated shaders kept on disk (decision 0003, and the two-level cache below): under the
//! configured cache folder, keyed by the container's content and the settings that change
//! what the translator makes, **not** by the translator's build.
//!
//! An entry (`victory_pipeline::stamped`) holds the encoded [`HostShader`] and a *rule stamp*:
//! the fingerprint of each translator rule the shader's translation went through
//! (`victory_shader::rules`: the passes, features and marked instruction lowerings it used),
//! and the translator source it was last verified with. On a lookup:
//! - made or verified by this very translator source: used as it is;
//! - another translator source, but every rule it used has the fingerprint it had (an edit to
//!   a rule it never went through): used as it is, and restamped in the background;
//! - a rule it used changed: **used as it is now** and translated again in the background
//!   ([`Mode::Lazy`], the default), and the new translation compared with the old. The same
//!   output keeps the entry (restamped); a different one replaces it, for the next run.
//!   `VICTORY_SHADER_REVALIDATE=eager` translates and compares before use instead (what
//!   `victory warm --incremental` does), `=trust` never revalidates.
//!
//! So an edit to the translator costs one translation of each shader it can have changed,
//! off the path of the run, and nothing for the rest; and a shader whose output did not change
//! keeps its compiled pipelines, which the pipeline library names by the translation's bytes
//! (`pipeline::library::name`), never by the build.
//!
//! Fingerprints are hints (see `victory_shader::rules`): `victory warm --incremental
//! --verify-all` (or `VICTORY_SHADER_REVALIDATE_ALL=1`) translates every entry again whatever
//! its stamp says, which is the check on them.
//!
//! The entry's payload is our own encoding of [`HostShader`]: a field added to it must be
//! added to `encode` and `decode` too, and [`CACHE_SCHEMA_VERSION`] bumped (the key changes,
//! so entries of the old format are never read).

use std::path::PathBuf;
use std::sync::atomic::{AtomicU8, AtomicU64, AtomicUsize, Ordering};
use std::sync::mpsc::Sender;
use std::sync::{Mutex, OnceLock};

use victory_pipeline::stamped::{self, Context, How, Mode, Revalidated, Rules};
use victory_pipeline::{Cache, CacheKey};
use victory_shader::emit::{HostShader, StaticSampler, UserData};
use victory_shader::resource::RootIndexedView;
use victory_shader::{HwStage, RootType};

mod legacy;

static CACHE: OnceLock<Cache> = OnceLock::new();

/// `VICTORY_SHADER_CACHE_KEY=output`: the cache of this file (keyed by settings, entries stamped
/// with the translator rules they used). Otherwise (the default) the cache of fx-final 26f8afd,
/// keyed by the translator that made each entry (`legacy`), so a cache warmed by that build
/// is still served. The two keep their entries apart (other keys), in the same folder.
fn output_key() -> bool {
    static ON: OnceLock<bool> = OnceLock::new();
    *ON.get_or_init(|| std::env::var("VICTORY_SHADER_CACHE_KEY").is_ok_and(|v| v.trim() == "output"))
}

/// What the cache did since the start (see [`report`]).
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq)]
pub struct Report {
    /// Translations served from an entry (every kind below but `translated`).
    pub served: u64,
    /// Shaders with no entry, translated and stored.
    pub translated: u64,
    /// Served from an entry that another translator source made, whose rules may have
    /// changed: still to be checked in the background.
    pub pending: u64,
    /// Entries restamped without translating: no rule they used changed.
    pub restamped: u64,
    /// Translated again with the same result: the entry stays, its pipelines with it.
    pub unchanged: u64,
    /// Translated again with another result: the entry (and its compiled pipelines) is new.
    pub changed: u64,
    /// Translated again and failed (or not stored): the old entry stays.
    pub failed: u64,
}

const SERVED: usize = 0;
const TRANSLATED: usize = 1;
const PENDING: usize = 2;
const RESTAMPED: usize = 3;
const UNCHANGED: usize = 4;
const CHANGED: usize = 5;
const FAILED: usize = 6;
static COUNTS: [AtomicU64; 7] = [const { AtomicU64::new(0) }; 7];

fn count(which: usize) {
    COUNTS[which].fetch_add(1, Ordering::Relaxed);
}

/// What the cache has done so far.
pub fn report() -> Report {
    let n = |i: usize| COUNTS[i].load(Ordering::Relaxed);
    Report {
        served: n(SERVED),
        translated: n(TRANSLATED),
        pending: n(PENDING),
        restamped: n(RESTAMPED),
        unchanged: n(UNCHANGED),
        changed: n(CHANGED),
        failed: n(FAILED),
    }
}

/// Keeps translated shaders in `dir` from now on.
pub fn use_shader_cache(dir: PathBuf) {
    if !output_key() {
        return legacy::use_shader_cache(dir);
    }
    match Cache::open(dir) {
        Ok(cache) => {
            let _ = CACHE.set(cache);
        }
        Err(e) => tracing::warn!(error = %e, "no shader cache"),
    }
}

/// Shaders found in the cache and translated since the start.
pub(crate) fn counts() -> (u64, u64) {
    if !output_key() {
        return legacy::counts();
    }
    let r = report();
    (r.served, r.translated)
}

/// How entries of another translator are brought up to date, and whether the stamps are
/// believed. Set by `victory warm --incremental`; otherwise `VICTORY_SHADER_REVALIDATE`
/// (`lazy`, the default, `eager`, `trust`) and `VICTORY_SHADER_REVALIDATE_ALL=1` decide.
pub fn set_revalidation(eager: bool, verify_all: bool) {
    SETTINGS.store(1 | u8::from(eager) << 1 | u8::from(verify_all) << 2, Ordering::Relaxed);
}

/// bit 0: set by [`set_revalidation`]; bit 1: eager; bit 2: verify all.
static SETTINGS: AtomicU8 = AtomicU8::new(0);

fn mode() -> Mode {
    let set = SETTINGS.load(Ordering::Relaxed);
    if set & 1 != 0 {
        return if set & 2 != 0 { Mode::Eager } else { Mode::Lazy };
    }
    static FROM_ENV: OnceLock<Mode> = OnceLock::new();
    *FROM_ENV.get_or_init(|| match std::env::var("VICTORY_SHADER_REVALIDATE").as_deref().map(str::trim) {
        Ok("eager") => Mode::Eager,
        Ok("trust" | "off" | "0") => Mode::Trust,
        _ => Mode::Lazy,
    })
}

fn verify_all() -> bool {
    let set = SETTINGS.load(Ordering::Relaxed);
    if set & 1 != 0 {
        return set & 4 != 0;
    }
    static FROM_ENV: OnceLock<bool> = OnceLock::new();
    *FROM_ENV.get_or_init(|| std::env::var_os("VICTORY_SHADER_REVALIDATE_ALL").is_some_and(|v| v == "1"))
}

/// The translator this build holds, for [`stamped::Entry::verified`]: the BLAKE3 of the
/// sources of `victory-shader` (`victory_shader::SOURCE_HASH`), or with
/// `VICTORY_SHADER_CACHE_KEY=exe` that of the executable.
fn source() -> &'static str {
    static SOURCE: OnceLock<String> = OnceLock::new();
    SOURCE.get_or_init(|| match executable() {
        Some(exe) => blake3::Hash::from_bytes(exe).to_hex().to_string(),
        None => victory_shader::SOURCE_HASH.to_owned(),
    })
}

/// `VICTORY_SHADER_CACHE_KEY=exe`: the digest of the executable, which then is part of the
/// key (any new executable starts with an empty cache; a build patched outside the translator's
/// sources that changes what it makes should). `None` otherwise. When the executable cannot be
/// read no entry can be trusted: a fresh digest per run.
fn executable() -> Option<[u8; 32]> {
    static EXE: OnceLock<Option<[u8; 32]>> = OnceLock::new();
    *EXE.get_or_init(|| {
        if !std::env::var_os("VICTORY_SHADER_CACHE_KEY").is_some_and(|v| v == "exe") {
            return None;
        }
        match std::env::current_exe().and_then(std::fs::read) {
            Ok(bytes) => Some(*blake3::hash(&bytes).as_bytes()),
            Err(e) => {
                tracing::warn!(error = %e, "cannot read our executable; shader cache unused");
                let now = std::time::SystemTime::now()
                    .duration_since(std::time::UNIX_EPOCH)
                    .unwrap_or_default();
                Some(*blake3::hash(&now.as_nanos().to_le_bytes()).as_bytes())
            }
        }
    })
}

/// The key of the settings of this run that change what the translator makes (see
/// [`context`]): the loop limit, which group barriers wait, the switches that pick the form of
/// a wave64 program, the clean-up passes that ran over the translated code, and what the
/// host the code is made for is like (`probe_view_limit`, which runs before the first
/// translation). The translator itself is not in it: its parts are in each entry's stamp.
fn build() -> &'static [u8; 32] {
    static BUILD: OnceLock<[u8; 32]> = OnceLock::new();
    BUILD.get_or_init(|| {
        let limit = victory_shader::emit::loop_limit();
        if limit != victory_shader::emit::DEFAULT_LOOP_LIMIT {
            tracing::warn!(
                limit,
                "VICTORY_SHADER_LOOP_LIMIT: translated loops stop sooner"
            );
        }
        let switches = SWITCHES.map(|name| (name, std::env::var_os(name)));
        // (VICTORY_RT_MISS, VIcky #6: ray-traced passes the translator cannot follow run with
        // every ray missing; the view heap the host keeps, which clamps view indices and gives
        // the heap map bindless views are looked up in (`VICTORY_HEAP_MAP`,
        // `VICTORY_HEAP_WINDOW`); (victory-amd) the host's widest wave, which picks how wave64
        // programs are made)
        let mut host = vec![u8::from(victory_shader::resource::rt_miss())];
        host.extend_from_slice(&victory_shader::emit::view_heap_limit().to_le_bytes());
        host.extend_from_slice(&victory_shader::emit::host_max_wave().to_le_bytes());
        let map = victory_shader::emit::view_heap_map().map_or([u32::MAX; 2], |(w, s)| [w, s]);
        for word in map {
            host.extend_from_slice(&word.to_le_bytes());
        }
        // (`VICTORY_F3_VIEW_FIX=1` changes how typed buffer accesses are translated)
        if victory_shader::emit::words::f3_view_fix() {
            host.extend_from_slice(b"f3 view fix");
        }
        context(
            limit,
            victory_shader::emit::group_sync(),
            &switches,
            victory_shader::emit::shader_opt(),
            &host,
            executable().as_ref(),
        )
    })
}

/// The environment switches that change what the translator makes, part of the key.
const SWITCHES: [&str; 8] = [
    "VICTORY_WAVE_WIDE",
    "VICTORY_WAVE_DEEP_SKIPS",
    "VICTORY_WAVE_OWN_BITS",
    // (victory-amd: these three change the translation too)
    "VICTORY_WAVE_LOCAL",
    "VICTORY_STRICT_JOINS",
    "VICTORY_MESH_OUTPUT_STRICT",
    "VICTORY_F16_NEAREST",
    // (VIcky #6: the loop limit changes the translation too)
    "VICTORY_SHADER_LOOP_LIMIT",
];

/// Serialization schema version of [`encode`] / [`decode`], and of the entries around them:
/// bump it whenever [`HostShader`]'s on-disk format changes. It is part of the key, so what
/// older builds stored is not looked at.
const CACHE_SCHEMA_VERSION: u32 = 2;

/// The key of a translation's settings (`victory_pipeline::stamped::context_key`): the loop
/// limit, which group barriers wait, the switches, the clean-up passes, the host's
/// description, and the executable's digest if the executable asked for. Nothing of the
/// translator's build. A switch set to nothing reads as unset in the translator, so it keys
/// alike.
fn context(
    limit: u32,
    group_sync: u8,
    switches: &[(&str, Option<std::ffi::OsString>)],
    shader_opt: u32,
    host: &[u8],
    executable: Option<&[u8; 32]>,
) -> [u8; 32] {
    let limit = limit.to_le_bytes();
    let shader_opt = shader_opt.to_le_bytes();
    let mut settings: Vec<(&str, &[u8])> = vec![
        ("loop limit", &limit),
        ("group sync", std::slice::from_ref(&group_sync)),
        ("shader opt", &shader_opt),
        ("host", host),
    ];
    for (name, value) in switches {
        settings.push((name, value.as_ref().map_or(&[][..], |v| v.as_encoded_bytes())));
    }
    if let Some(exe) = executable {
        settings.push(("executable", exe));
    }
    stamped::context_key(CACHE_SCHEMA_VERSION, &settings)
}

/// The translation of a console shader container, from the cache when
/// there is one.
pub(crate) fn host_shader(container: &[u8]) -> Result<HostShader, String> {
    if !output_key() {
        return legacy::host_shader(container);
    }
    use victory_shader::emit;
    if let Some(s) = stand_in(container) {
        return Ok(s);
    }
    // The hash the log names the shader by (`pipeline.rs`), which the
    // per-program switches take.
    let hash = blake3::hash(container).to_hex()[..16].to_owned();
    let r = match CACHE.get() {
        Some(cache) => {
            // (`wave64_retried` second: it parses the container, and only a
            // program a list names, or with `VICTORY_WAVE_DEEP_SKIPS=0`
            // any, gets there)
            let off = !emit::deep_skips_allowed(Some(&hash)) && emit::wave64_retried(container);
            cached(cache, &program_key(build(), off), container, &hash)
        }
        None => translate(container, &hash).and_then(|(payload, _)| decode(&payload).ok_or_else(damaged)),
    };
    match r {
        Err(e) => exposure_stub(container).ok_or(e),
        ok => ok,
    }
}

fn damaged() -> String {
    "a translation that does not decode".to_owned()
}

/// The translation of `container` by this translator: encoded, with the rules it went through.
fn translate(container: &[u8], hash: &str) -> Result<(Vec<u8>, Rules), String> {
    use victory_shader::{emit, rules};
    let (shader, used) = rules::record(|| emit::for_program(hash, || emit::translate(container)));
    let shader = shader.map_err(|e| e.to_string())?;
    // (a rule this translator has no fingerprint for cannot be; it would read as changed)
    let stamp = used
        .into_iter()
        .map(|rule| (rule.to_owned(), rules::fingerprint(rule).unwrap_or([0; 32])))
        .collect();
    Ok((encode(&shader), stamp))
}

/// The stand-ins of `victory_shader::emit::stand_in`, each opt-in:
/// `VICTORY_AMBIENT_STUB=1` (the diffuse illumination's last pass writes
/// `VICTORY_AMBIENT_STUB_VALUE`, 0.1 by default) and
/// `VICTORY_SHADOWMASK_STUB=1` (the ray-traced distant shadows write
/// `VICTORY_SHADOWMASK_STUB_VALUE`, 1 = lit; each output in its own encoding,
/// by slot: `VICTORY_AMBIENT_STUB_SLOTS="152=rgb9e5,208=mask,*=f"`), for the containers in
/// `VICTORY_AMBIENT_STUB_HASHES` / `VICTORY_SHADOWMASK_STUB_HASHES` (the
/// title's by default). They replace those passes whenever switched on: the
/// passes translate, but what they read comes from ray tracing, which does
/// not run on the host yet. Not cached (the cache keys by container).
fn stand_in(container: &[u8]) -> Option<HostShader> {
    use victory_shader::emit::stand_in as s;
    let hash = blake3::hash(container).to_hex()[..16].to_owned();
    let kinds = [
        ("VICTORY_AMBIENT_STUB", s::AMBIENT_HASHES, s::AMBIENT_VALUE, s::ambient_slots(&hash)),
        ("VICTORY_SHADOWMASK_STUB", s::SHADOWMASK_HASHES, s::SHADOWMASK_VALUE, s::SHADOWMASK_SLOTS),
    ];
    for (name, hashes, value, defaults) in kinds {
        if std::env::var(name).ok().as_deref().map(str::trim) != Some("1") {
            continue;
        }
        let listed = match std::env::var(format!("{name}_HASHES")) {
            Ok(v) => v
                .split(|c: char| c == ',' || c.is_whitespace())
                .any(|h| h.eq_ignore_ascii_case(&hash)),
            Err(_) => hashes.contains(&hash.as_str()),
        };
        if !listed {
            continue;
        }
        let value = std::env::var(format!("{name}_VALUE"))
            .ok()
            .and_then(|v| v.trim().parse::<f32>().ok())
            .unwrap_or(value);
        let slots = std::env::var(format!("{name}_SLOTS")).unwrap_or_default();
        let plan = s::Plan::new(value)
            .with_overrides(defaults)
            .with_overrides(&slots);
        match s::storing_plan(container, plan) {
            Ok((shader, plan)) => {
                tracing::warn!(
                    %hash,
                    stand_in = name,
                    value,
                    outputs = ?plan.seen,
                    "pass replaced by a stand-in writing each output in its own encoding"
                );
                return Some(shader);
            }
            Err(e) => {
                tracing::warn!(%hash, stand_in = name, error = %e, "stand-in does not translate");
            }
        }
    }
    None
}

/// `VICTORY_EXPOSURE_STUB=1`: the auto-exposure average, when it does not
/// translate, replaced by a stand-in writing a constant luminance
/// (`victory_shader::emit::exposure`; `VICTORY_EXPOSURE_STUB_VALUE`, 0.18 by
/// default) for the containers in `VICTORY_EXPOSURE_STUB_HASHES` (16-digit
/// hashes as the pipeline logs them, comma-separated; the title's by
/// default).
fn exposure_stub(container: &[u8]) -> Option<HostShader> {
    use victory_shader::emit::exposure;
    if std::env::var("VICTORY_EXPOSURE_STUB").ok()?.trim() != "1" {
        return None;
    }
    let hash = blake3::hash(container).to_hex()[..16].to_owned();
    let listed = match std::env::var("VICTORY_EXPOSURE_STUB_HASHES") {
        Ok(v) => v
            .split(|c: char| c == ',' || c.is_whitespace())
            .any(|h| h.eq_ignore_ascii_case(&hash)),
        Err(_) => exposure::HASHES.contains(&hash.as_str()),
    };
    if !listed {
        return None;
    }
    let value = std::env::var("VICTORY_EXPOSURE_STUB_VALUE")
        .ok()
        .and_then(|v| v.trim().parse::<f32>().ok())
        .unwrap_or(exposure::VALUE);
    match exposure::stub(container, value) {
        Ok(s) => {
            tracing::warn!(%hash, value, "exposure shader replaced by a constant stand-in");
            Some(s)
        }
        Err(e) => {
            tracing::warn!(%hash, error = %e, "exposure stand-in does not translate either");
            None
        }
    }
}

/// The key of one program's entries: the settings', mixed with the
/// per-program decision when the program is kept out of the wave64 form
/// "without skips, scalars followed" (`VICTORY_WAVE_DEEP_SKIPS_NOT`,
/// `_ONLY`: `victory_shader::emit::deep_skips_allowed`). Only for a wave64
/// compute program on a host of 32-lane waves, the only ones it can change.
/// So the lists are in no key: changing one translates again only the
/// programs whose decision it changes, and a program allowed the form keeps
/// the key it had without any list.
fn program_key(build: &[u8; 32], deep_skips_off: bool) -> [u8; 32] {
    if !deep_skips_off {
        return *build;
    }
    let mut h = blake3::Hasher::new();
    h.update(build);
    h.update(b"VICTORY_WAVE_DEEP_SKIPS off for this program");
    *h.finalize().as_bytes()
}

/// Whether the translation of `container` is in the cache already (nothing is read or
/// translated: the entry's file is looked for). An entry of another translator counts: it
/// is used as it is (see the module notes).
pub(crate) fn is_cached(container: &[u8]) -> bool {
    if !output_key() {
        return legacy::is_cached(container);
    }
    CACHE
        .get()
        .is_some_and(|cache| has_entry(cache, build(), container))
}

/// Whether `cache` holds an entry for `container` under the settings `context`.
fn has_entry(cache: &Cache, context: &[u8; 32], container: &[u8]) -> bool {
    cache.path_for(&stamped::entry_key(context, container)).exists()
}

/// The translation of `container` under the settings `context`, as the module notes say:
/// from its entry, which is brought up to date now or in the background, else translated
/// and stored.
fn cached(cache: &Cache, context: &[u8; 32], container: &[u8], hash: &str) -> Result<HostShader, String> {
    let key = stamped::entry_key(context, container);
    let ctx = Context {
        source: source(),
        current: &|rule| victory_shader::rules::fingerprint(rule),
        mode: mode(),
        verify_all: verify_all(),
    };
    let served = stamped::serve(cache, &key, &ctx, || translate(container, hash))?;
    match served.how {
        How::New => count(TRANSLATED),
        how => {
            count(SERVED);
            match how {
                How::Pending => {
                    count(PENDING);
                    background::enqueue(&key, container, hash);
                }
                How::Revalidated(found) => {
                    count(outcome(found));
                    if found == Revalidated::Changed {
                        tracing::info!(%hash, "translation changed");
                    }
                }
                How::Hit | How::Trusted | How::New => {}
            }
        }
    }
    match decode(&served.payload) {
        Some(shader) => Ok(shader),
        None => {
            // An entry that is whole but does not read (a format this build does not know,
            // under a key that should have told): translated again, stored over it.
            tracing::debug!("shader cache entry does not decode; translating again");
            let (payload, rules) = translate(container, hash)?;
            let _ = stamped::store(cache, &key, source(), rules, payload.clone());
            decode(&payload).ok_or_else(damaged)
        }
    }
}

fn outcome(found: Revalidated) -> usize {
    match found {
        Revalidated::Restamped => RESTAMPED,
        Revalidated::Same => UNCHANGED,
        Revalidated::Changed => CHANGED,
        Revalidated::Failed => FAILED,
    }
}

/// The background pass of [`Mode::Lazy`]: one thread of low priority that translates again
/// the shaders the run was served from entries to check, and compares.
mod background {
    use super::*;

    struct Job {
        key: CacheKey,
        container: Vec<u8>,
        hash: String,
    }

    /// Bytes of containers waiting. Past it a shader is not queued: its entry stays as it is
    /// and the next run asks again.
    const QUEUED_MAX: usize = 96 << 20;
    static QUEUED: AtomicUsize = AtomicUsize::new(0);

    fn queue() -> &'static Mutex<Option<Sender<Job>>> {
        static QUEUE: OnceLock<Mutex<Option<Sender<Job>>>> = OnceLock::new();
        QUEUE.get_or_init(|| {
            let (tx, rx) = std::sync::mpsc::channel::<Job>();
            let started = std::thread::Builder::new().name("shader-revalidate".into()).spawn(move || {
                lower_priority();
                for job in rx {
                    QUEUED.fetch_sub(job.container.len(), Ordering::Relaxed);
                    run(&job);
                }
            });
            if let Err(e) = started {
                tracing::warn!(error = %e, "no background pass over shaders of another translator");
                return Mutex::new(None);
            }
            Mutex::new(Some(tx))
        })
    }

    fn lower_priority() {
        use windows_sys::Win32::System::Threading::{GetCurrentThread, SetThreadPriority, THREAD_PRIORITY_BELOW_NORMAL};
        // SAFETY: the calling thread's own pseudo-handle.
        unsafe { SetThreadPriority(GetCurrentThread(), THREAD_PRIORITY_BELOW_NORMAL) };
    }

    pub(super) fn enqueue(key: &CacheKey, container: &[u8], hash: &str) {
        if QUEUED.fetch_add(container.len(), Ordering::Relaxed) + container.len() > QUEUED_MAX {
            QUEUED.fetch_sub(container.len(), Ordering::Relaxed);
            return;
        }
        let job = Job { key: *key, container: container.to_vec(), hash: hash.to_owned() };
        let sent = queue().lock().ok().and_then(|q| q.as_ref().map(|tx| tx.send(job).is_ok()));
        if sent != Some(true) {
            QUEUED.fetch_sub(container.len(), Ordering::Relaxed);
        }
    }

    fn run(job: &Job) {
        let Some(cache) = CACHE.get() else { return };
        // (what the entry is now: another run may have brought it up to date)
        let Some(entry) = stamped::read(cache, &job.key) else { return };
        if entry.verified == source() && !verify_all() {
            return;
        }
        let convert = || translate(&job.container, &job.hash);
        let (found, _) = if verify_all() {
            stamped::reconvert(cache, &job.key, entry, source(), convert)
        } else {
            stamped::revalidate(cache, &job.key, entry, source(), &|rule| victory_shader::rules::fingerprint(rule), convert)
        };
        count(outcome(found));
        tracing::debug!(hash = %job.hash, ?found, "shader of another translator revalidated");
    }
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

    fn ctx(limit: u32, wide: Option<&str>) -> [u8; 32] {
        context(limit, 1, &[("VICTORY_WAVE_WIDE", wide.map(Into::into))], 3, b"host", None)
    }

    #[test]
    fn the_key_follows_the_settings_and_nothing_else() {
        let base = ctx(4096, None);
        assert_eq!(base, ctx(4096, None), "the same inputs, the same key");
        // its settings: the loop limit, a switch that picks the wave64 form
        assert_ne!(base, ctx(16, None));
        assert_ne!(base, ctx(4096, Some("1")));
        // (a switch set to nothing reads as unset in the translator, so it keys alike)
        assert_eq!(ctx(1, None), ctx(1, Some("")));
        // the host, the clean-up passes, the group sync
        assert_ne!(base, context(4096, 1, &[("VICTORY_WAVE_WIDE", None)], 3, b"another host", None));
        assert_ne!(base, context(4096, 1, &[("VICTORY_WAVE_WIDE", None)], 4, b"host", None));
        assert_ne!(base, context(4096, 2, &[("VICTORY_WAVE_WIDE", None)], 3, b"host", None));
        // the executable only when it is asked for
        assert_ne!(base, context(4096, 1, &[("VICTORY_WAVE_WIDE", None)], 3, b"host", Some(&[1; 32])));
    }

    #[test]
    fn the_key_of_this_build_has_no_translator_in_it() {
        // the translator's sources are named by the entries' stamps, not the key: the key is
        // the same function of the settings whatever `victory_shader::SOURCE_HASH` is
        if std::env::var_os("VICTORY_SHADER_CACHE_KEY").is_none() {
            assert!(executable().is_none());
            assert_eq!(source(), victory_shader::SOURCE_HASH);
        }
        assert_eq!(victory_shader::SOURCE_HASH.len(), 64);
        assert!(victory_shader::SOURCE_HASH.bytes().all(|b| b.is_ascii_hexdigit()));
    }

    #[test]
    fn an_entry_is_found_for_the_settings_that_stored_it_only() {
        let dir = tempfile::tempdir().unwrap();
        let cache = Cache::open(dir.path()).unwrap();
        let (container, mine, other) = (b"a container".as_slice(), ctx(1, None), ctx(2, None));
        assert!(!has_entry(&cache, &mine, container));
        let rules = vec![("core".to_owned(), [1; 32])];
        stamped::store(&cache, &stamped::entry_key(&mine, container), "a source", rules, encode(&shader())).unwrap();
        assert!(has_entry(&cache, &mine, container));
        // other settings, or another container, find nothing
        assert!(!has_entry(&cache, &other, container));
        assert!(!has_entry(&cache, &mine, b"another container"));
        // and an entry of another translator source is still found: it is used as it is
        let key = stamped::entry_key(&mine, container);
        let entry = stamped::read(&cache, &key).unwrap();
        assert_eq!(decode(&entry.payload), Some(shader()));
        assert!(has_entry(&cache, &mine, container));
    }

    #[test]
    fn a_program_kept_out_of_the_deep_skips_form_has_a_key_of_its_own() {
        let build = [7u8; 32];
        assert_eq!(program_key(&build, false), build, "allowed: the settings' key, as without lists");
        let off = program_key(&build, true);
        assert_ne!(off, build);
        assert_eq!(program_key(&build, true), off, "the same decision, the same key");
        assert_ne!(program_key(&[8; 32], true), off, "and still the settings'");

        // So only the program whose decision changed has another entry.
        let dir = tempfile::tempdir().unwrap();
        let cache = Cache::open(dir.path()).unwrap();
        let [a, b] = [b"program a".as_slice(), b"program b"];
        for c in [a, b] {
            let key = stamped::entry_key(&program_key(&build, false), c);
            stamped::store(&cache, &key, "src", Vec::new(), encode(&shader())).unwrap();
        }
        assert!(has_entry(&cache, &program_key(&build, false), a));
        assert!(!has_entry(&cache, &program_key(&build, true), a));
        assert!(has_entry(&cache, &program_key(&build, false), b));
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
