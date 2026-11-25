//! Pipeline states. Both descriptions start as on PC: the root signature,
//! then the shader stages (five for graphics, one for compute); streams
//! carry the rest of the graphics state as subobjects of the standard
//! types.
//!
//! Host pipelines are made on a pipeline's first draw, not at creation:
//! Grand Theft Auto VI creates some 60,000 pipelines in its first 20
//! seconds and draws with a few dozen (decision 0007, step 3). Shader
//! containers are shared by content hash and translated once, on first
//! use (decision 0003).
//!
//! shortcut: only vertex plus pixel pipelines from streams and compute
//! pipelines get host pipelines; mesh, tessellation and geometry stages,
//! and the plain graphics description, keep their containers only. The 32-byte
//! packet after the header stays zero: titles only copy it inline, which
//! our null command-list limits keep from running.
#![allow(clippy::not_unsafe_ptr_arg_deref)]

use std::collections::HashMap;
use std::mem::offset_of;
use std::sync::{Arc, Mutex, OnceLock};

use crate::object::{self, E_INVALIDARG, E_NOTIMPL, HRESULT, Iid, Object, PIPELINE, Table, This, guid};
use crate::resource::PAGEABLE_IIDS;

const PIPELINE_IIDS: [Iid; 4] = [
    guid(
        0x765a30f3,
        0xf624,
        0x4c6f,
        [0xa8, 0x28, 0xac, 0xe9, 0x48, 0x62, 0x24, 0x45],
    ),
    PAGEABLE_IIDS[0],
    PAGEABLE_IIDS[1],
    PAGEABLE_IIDS[2],
];

#[repr(C)]
pub(crate) struct Bytecode {
    code: *const u8,
    size: usize,
}

#[repr(C)]
pub(crate) struct GraphicsHead {
    root: This,
    /// Vertex, pixel, domain, hull, geometry.
    stages: [Bytecode; 5],
}

#[repr(C)]
pub(crate) struct ComputeHead {
    root: This,
    stage: Bytecode,
}

/// A shader container a title passed, shared by every pipeline with the
/// same bytes.
pub(crate) struct Shader {
    pub(crate) stage: &'static str,
    /// The first 8 bytes of its BLAKE3, as logged.
    pub(crate) hash: String,
    container: Vec<u8>,
    /// It is left out: it animates or draws hair strands ([`hair_strands`]),
    /// or serves rays that do not run ([`idle_without_rays`]).
    #[cfg(windows)]
    hair: bool,
    /// Its result gets the least indirect light ([`crate::ambient`]).
    #[cfg(windows)]
    pub(crate) resolves_light: bool,
    /// It draws the sky as the title reflects it ([`crate::ambient::SKY`]).
    #[cfg(windows)]
    pub(crate) draws_sky: bool,
    /// It writes the title's reflections for the last time in a frame
    /// ([`crate::ambient::REFLECTED`]).
    #[cfg(windows)]
    pub(crate) ends_reflections: bool,
    #[cfg(windows)]
    translated: OnceLock<Option<Translated>>,
}

/// Whether a container's program only makes sense with the title's rays,
/// which do not run (`VICTORY_RAYS` has them run, and it with them): the
/// program that tells the screen probes which rays to trace.
/// shortcut: left out. The AMD driver never ended its pipeline until the
/// clean-up passes shortened it; once it ran, with no rays behind it, the
/// whole picture lost light, shadows and reflections (run opt31b against
/// sans39, 2026-11-25).
#[cfg(windows)]
fn idle_without_rays(container: &[u8]) -> bool {
    const NAME: &[u8] = b"CS_ScreenProbes_ActivateRays";
    !victory_shader::emit::rays() && container.windows(NAME.len()).any(|w| w == NAME)
}

/// Whether a container's program is one of those that animate and draw hair
/// strands, by its name. `VICTORY_HAIR` has them run.
/// shortcut: left out, as some of them do not translate (an address from a
/// phi) and the others draw lines all over the screen without them.
#[cfg(windows)]
fn hair_strands(container: &[u8]) -> bool {
    static RUN: OnceLock<bool> = OnceLock::new();
    const NAMES: [&[u8]; 18] = [
        b"InterpolateStrand",
        b"HairStrand",
        b"CS_VisHair",
        b"PS_Smh",
        b"CS_InitSimBuffer",
        b"CS_PreSkinSim",
        b"CS_SolvePer",
        b"CS_WavePerSpline",
        b"CS_EstimateStartPosition",
        b"CS_CachePrev",
        b"CS_RestoreSimVelocity",
        b"CS_UpdatePosePosition",
        b"CS_GenerateAccessories",
        b"CSGenerateHair",
        b"CS_CalculateFireVolumes",
        b"CS_ProjDecalCut",
        b"CS_CullMaskPass",
        b"CS_LodBlend",
    ];
    if *RUN.get_or_init(|| std::env::var_os("VICTORY_HAIR").is_some()) {
        return false;
    }
    const END: &[u8] = b"_Wrapped";
    container
        .windows(END.len())
        .enumerate()
        .filter(|(_, w)| *w == END)
        .any(|(at, _)| {
            let before = &container[at.saturating_sub(100)..at];
            let name = before
                .rsplit(|b| !b.is_ascii_alphanumeric() && *b != b'_')
                .next()
                .unwrap_or_default();
            NAMES.iter().any(|n| name.windows(n.len()).any(|w| w == *n))
        })
}

impl Shader {
    /// The program's name, if its container tells.
    #[cfg(all(windows, feature = "diagnostics"))]
    pub(crate) fn name(&self) -> String {
        const END: &[u8] = b"_Wrapped";
        let at = self.container.windows(END.len()).position(|w| w == END);
        let before = &self.container[at.unwrap_or(0).saturating_sub(100)..at.unwrap_or(0)];
        let name = before
            .rsplit(|b| !b.is_ascii_alphanumeric() && *b != b'_')
            .next()
            .unwrap_or_default();
        String::from_utf8_lossy(name).into_owned()
    }
}

impl std::fmt::Debug for Shader {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        write!(f, "{} {}", self.stage, self.hash)
    }
}

/// Every container seen, by full content hash.
static SHADERS: Mutex<Option<HashMap<[u8; 32], Arc<Shader>>>> = Mutex::new(None);

/// `VICTORY_SHADER_SWAP=<folder>`: a container whose hash (16 digits, as
/// logged) names a file `<hash>.dxbc` there is replaced by that file's
/// container, of the same stage and root signature.
/// shortcut: for the distant trees' wind variants (`VS_Octa*Wind*`), whose
/// wind volumes nothing fills yet, so that they threw the trees' quads
/// across the screen; their variants without wind stand in for them.
fn swapped(hash: &str) -> Option<&'static [u8]> {
    static SWAPS: OnceLock<HashMap<String, Vec<u8>>> = OnceLock::new();
    SWAPS
        .get_or_init(|| {
            let mut swaps = HashMap::new();
            let Some(folder) = std::env::var_os("VICTORY_SHADER_SWAP") else { return swaps };
            for entry in std::fs::read_dir(folder).into_iter().flatten().flatten() {
                let path = entry.path();
                if path.extension().is_some_and(|e| e == "dxbc")
                    && let (Some(name), Ok(bytes)) = (path.file_stem().and_then(|n| n.to_str()), std::fs::read(&path))
                {
                    swaps.insert(name.to_owned(), bytes);
                }
            }
            tracing::info!(count = swaps.len(), "shader containers replaced");
            swaps
        })
        .get(hash)
        .map(Vec::as_slice)
}

/// The shared container for `bytes`.
fn shader(stage: &'static str, bytes: &[u8]) -> Arc<Shader> {
    let full = blake3::hash(bytes);
    let (bytes, full) = match swapped(&full.to_hex()[..16]) {
        Some(other) => (other, blake3::hash(other)),
        None => (bytes, full),
    };
    let mut shaders = SHADERS.lock().unwrap_or_else(|e| e.into_inner());
    shaders
        .get_or_insert_with(HashMap::new)
        .entry(*full.as_bytes())
        .or_insert_with(|| {
            Arc::new(Shader {
                stage,
                hash: full.to_hex()[..16].to_owned(),
                container: bytes.to_vec(),
                #[cfg(windows)]
                hair: hair_strands(bytes) || idle_without_rays(bytes),
                #[cfg(windows)]
                resolves_light: bytes
                    .windows(crate::ambient::PROGRAM.len())
                    .any(|w| w == crate::ambient::PROGRAM),
                #[cfg(windows)]
                draws_sky: bytes
                    .windows(crate::ambient::SKY.len())
                    .any(|w| w == crate::ambient::SKY),
                #[cfg(windows)]
                ends_reflections: bytes
                    .windows(crate::ambient::REFLECTED.len())
                    .any(|w| w == crate::ambient::REFLECTED),
                #[cfg(windows)]
                translated: OnceLock::new(),
            })
        })
        .clone()
}

/// A translated shader and the console resources its code reads.
#[cfg(windows)]
pub(crate) struct Translated {
    pub(crate) host: victory_shader::emit::HostShader,
    pub(crate) used: Vec<victory_shader::UsedResource>,
    /// Whether the shader indexes `ResourceDescriptorHeap[]` directly.
    pub(crate) resource_heap: bool,
    /// Whether the shader may write memory (buffer, image, or global stores
    /// and atomics); `true` when its code is not known.
    pub(crate) writes_memory: bool,
}

#[cfg(windows)]
impl Shader {
    /// The translation, made on first use; a shader that does not
    /// translate is logged once.
    pub(crate) fn translated(&self) -> Option<&Translated> {
        self.translated
            .get_or_init(|| {
                let done = translate(&self.container);
                #[cfg(feature = "diagnostics")]
                if done.is_err()
                    || std::env::var("VICTORY_SHADER_HASHES")
                        .is_ok_and(|l| l.split(',').any(|h| h == self.hash))
                {
                    dump(self);
                }
                match &done {
                    Ok(t) => tracing::info!(
                        stage = self.stage,
                        hash = %self.hash,
                        user_data = t.host.user_data.len(),
                        samplers = t.host.samplers.len(),
                        // As Radeon GPU Detective names a shader in flight.
                        dxil = %t.host.dxil.get(4..20).map_or(String::new(), |d| {
                            let half = |at: usize| u64::from_le_bytes(d[at..at + 8].try_into().unwrap_or([0; 8]));
                            format!("{:016x}{:016x}", half(0), half(8))
                        }),
                        "shader translated"
                    ),
                    Err(e) => tracing::warn!(
                        stage = self.stage,
                        hash = %self.hash,
                        error = %e,
                        "shader does not translate; its pipelines' draws are dropped"
                    ),
                }
                done.ok()
            })
            .as_ref()
    }
}

/// Diagnostics (`VICTORY_SHADER_DUMP=<dir>`, outside the repository): saves
/// a shader that does not translate, for `victory translate`.
#[cfg(all(windows, feature = "diagnostics"))]
fn dump(shader: &Shader) {
    let Some(dir) = std::env::var_os("VICTORY_SHADER_DUMP").map(std::path::PathBuf::from) else {
        return;
    };
    let path = dir.join(format!("{}-{}.dxbc", shader.stage, shader.hash));
    if let Err(error) = victory_core::ensure_outside_repo(&path) {
        tracing::warn!(%error, "shader not saved");
    } else if let Err(error) = std::fs::write(&path, &shader.container) {
        tracing::warn!(%error, "shader not saved");
    }
}

/// As [`dump`], for the shaders of a pipeline the host refused: they
/// translate, so only this tells which containers to look at.
#[cfg(windows)]
fn dump_all(shaders: &[Arc<Shader>]) {
    #[cfg(feature = "diagnostics")]
    for shader in shaders {
        dump(shader);
    }
    #[cfg(not(feature = "diagnostics"))]
    let _ = shaders;
}

#[cfg(windows)]
fn translate(container: &[u8]) -> Result<Translated, String> {
    use victory_shader::{Container, FourCC, HwStage, RootType};
    let target = crate::host::host().map_or(victory_shader::emit::Target::ANY, |h| h.target());
    let host = crate::shader_cache::host_shader(container, target)?;
    let dxil_heap = Container::parse(&host.dxil).is_ok_and(|c| {
        c.part(FourCC::SFI0)
            .and_then(|p| p.data.get(..8))
            .is_some_and(|b| {
                let flags = u64::from_le_bytes(b.try_into().unwrap_or_default());
                flags & (1 << 32) != 0
            })
    });
    let c = Container::parse(container).map_err(|e| e.to_string())?;
    let programs = victory_shader::container_programs(&c).unwrap_or_default();
    let writes_memory = programs.is_empty() || programs.iter().any(writes_memory);
    let (used, native_heap) = programs
        .into_iter()
        .find(|p| {
            matches!(
                p.stage,
                HwStage::Vs | HwStage::Ps | HwStage::Ms | HwStage::Cs
            )
        })
        .map(|p| {
            let rh = p.layout.iter().any(|rp| rp.root == RootType::ResourceHeap)
                || p.used.iter().any(|u| u.root == RootType::ResourceHeap);
            (p.used, rh)
        })
        .unwrap_or_default();
    Ok(Translated {
        host,
        used,
        resource_heap: dxil_heap || native_heap,
        writes_memory,
    })
}

/// Whether a native program may write memory: it has a buffer, image,
/// flat, or global store or atomic, or code that is not all known (an
/// instruction outside our table, an indirect jump, or code that does not
/// decode).
#[cfg(windows)]
fn writes_memory(program: &victory_shader::NativeProgram) -> bool {
    use victory_shader::gfx10::{Encoding, decode};
    let Ok(code) = decode(&program.code) else {
        return true;
    };
    code.iter().any(|i| {
        let name = i.name();
        match i.encoding {
            Encoding::Mubuf
            | Encoding::Mtbuf
            | Encoding::Mimg
            | Encoding::Flat
            | Encoding::Global => name.is_none_or(stores),
            Encoding::Sop1 => name.is_none_or(|n| matches!(n, "s_setpc_b64" | "s_swappc_b64")),
            _ => false,
        }
    })
}

/// Whether a memory instruction (by name) writes memory.
#[cfg(windows)]
fn stores(name: &str) -> bool {
    name.contains("store") || name.contains("atomic")
}

#[repr(C)]
struct Pipeline {
    object: Object,
    packet: [u32; 8],
    /// Each used stage's container.
    shaders: Vec<Arc<Shader>>,
    /// Whether a list has bound it yet (its shaders are logged then).
    bound: std::sync::atomic::AtomicBool,
    /// What its host pipeline is made from, and the host pipeline once made
    /// (by a worker: the title may release the pipeline meanwhile).
    #[cfg(windows)]
    host: Arc<make::Pending>,
}
const _: () = assert!(offset_of!(Pipeline, packet) == 0x10);

/// Stage names, in description order.
const GRAPHICS_STAGES: [&str; 5] = ["vertex", "pixel", "domain", "hull", "geometry"];

fn pipeline<'a>(this: This) -> &'a Pipeline {
    // SAFETY: the title passes one of our pipelines.
    unsafe { &*this.cast::<Pipeline>() }
}

/// List method: logs each pipeline's shaders the first time a list binds
/// it, so a run lists the shaders its frames need first.
pub(crate) fn bound(this: This) {
    if this.is_null() {
        return;
    }
    let p = pipeline(this);
    if p.bound.swap(true, std::sync::atomic::Ordering::Relaxed) {
        return;
    }
    let frame = crate::frame::current();
    for shader in &p.shaders {
        tracing::info!(pipeline = ?this, frame, stage = shader.stage, size = shader.container.len(), hash = %shader.hash, "first bind");
    }
}

/// The host pipeline of a title pipeline; `None` if one of its shaders does
/// not translate, the host refuses it, or it is still being made (its first
/// use starts that, see [`make`]).
#[cfg(windows)]
pub(crate) fn host_pipeline<'a>(this: This) -> Option<&'a host::HostPipeline> {
    if this.is_null() {
        return None;
    }
    make::ready(&pipeline(this).host)
}

/// Host pipelines queued and not made yet.
#[cfg(windows)]
pub(crate) fn making() -> u64 {
    make::waiting()
}

/// Draws and dispatches dropped since the last [`take_drops`], by reason
/// (diagnostics).
#[cfg(all(windows, feature = "diagnostics"))]
static DROPS: Mutex<Option<HashMap<String, u64>>> = Mutex::new(None);

/// Shader hashes as printed in the pipeline diagnostics.
#[cfg(all(windows, feature = "diagnostics"))]
fn parse_skip_hashes(value: &str) -> Vec<String> {
    value
        .split(',')
        .map(str::trim)
        .filter(|hash| !hash.is_empty())
        .filter_map(|hash| {
            if hash.len() == 16 && hash.bytes().all(|byte| byte.is_ascii_hexdigit()) {
                Some(hash.to_ascii_lowercase())
            } else {
                tracing::warn!(
                    hash,
                    "VICTORY_SKIP_PIPELINES expects 16-digit shader hashes"
                );
                None
            }
        })
        .collect()
}

#[cfg(all(windows, feature = "diagnostics"))]
fn skipped_shader(p: &Pipeline) -> Option<&str> {
    static HASHES: OnceLock<Vec<String>> = OnceLock::new();
    let hashes = HASHES.get_or_init(|| {
        let value = std::env::var("VICTORY_SKIP_PIPELINES").unwrap_or_default();
        parse_skip_hashes(&value)
    });
    p.shaders
        .iter()
        .find(|shader| shader.hair || !hashes.is_empty() && hashes.contains(&shader.hash))
        .map(|shader| shader.hash.as_str())
}

#[cfg(all(windows, feature = "diagnostics"))]
pub(crate) fn is_skipped(this: This) -> bool {
    !this.is_null() && skipped_shader(pipeline(this)).is_some()
}

/// Counts a draw or dispatch of `this` intentionally skipped or dropped
/// for want of a host pipeline (diagnostics).
#[cfg(all(windows, feature = "diagnostics"))]
pub(crate) fn note_drop(this: This) {
    let why = if this.is_null() {
        Some("no pipeline set".to_owned())
    } else {
        let p = pipeline(this);
        skipped_shader(p)
            .map(|hash| format!("skipped shader {hash}"))
            .or_else(|| make::missing(&p.host))
    };
    if let Some(why) = why {
        note_why(why);
    }
}

/// A title pipeline's shaders (diagnostics).
#[cfg(all(windows, feature = "diagnostics"))]
pub(crate) fn names(this: This) -> String {
    if this.is_null() {
        return "none".into();
    }
    pipeline(this).shaders.iter().map(|s| format!("{} {}", s.name(), s.hash)).collect()
}

/// Counts a draw or dispatch dropped for `why` (diagnostics).
#[cfg(all(windows, feature = "diagnostics"))]
pub(crate) fn note_why(why: String) {
    let mut drops = DROPS.lock().unwrap_or_else(|e| e.into_inner());
    *drops.get_or_insert_with(HashMap::new).entry(why).or_default() += 1;
}

/// The drops counted since the last call, most first (diagnostics).
#[cfg(all(windows, feature = "diagnostics"))]
pub(crate) fn take_drops() -> Vec<(String, u64)> {
    let drops = DROPS.lock().unwrap_or_else(|e| e.into_inner()).take();
    let mut drops: Vec<(String, u64)> = drops.into_iter().flatten().collect();
    // Frame reports only show the first 40 reasons. Keep ties ordered so
    // a HashMap iteration order change cannot change which reasons appear.
    drops.sort_unstable_by(|a, b| b.1.cmp(&a.1).then_with(|| a.0.cmp(&b.0)));
    drops
}

/// The stage's container, if it has one.
fn copy(stage: &'static str, code: &Bytecode) -> Option<Arc<Shader>> {
    if code.code.is_null() || code.size == 0 {
        return None;
    }
    // SAFETY: the title passes `size` bytes of shader code.
    Some(shader(stage, unsafe {
        std::slice::from_raw_parts(code.code, code.size)
    }))
}

fn hand_out(
    shaders: Vec<Arc<Shader>>,
    #[cfg(windows)] graphics: Option<Box<host::GraphicsState>>,
    iid: *const Iid,
    out: *mut This,
) -> HRESULT {
    tracing::trace!(?shaders, "pipeline state");
    #[cfg(windows)]
    let host = Arc::new(make::Pending::new(shaders.clone(), graphics));
    #[cfg(windows)]
    make::early(&host);
    let pipeline = Box::new(Pipeline {
        object: Object::new(&FUNCTIONS),
        packet: [0; 8],
        #[cfg(windows)]
        host,
        shaders,
        bound: std::sync::atomic::AtomicBool::new(false),
    });
    object::hand_out(PIPELINE, pipeline, iid, &PIPELINE_IIDS, out)
}

/// Device method.
pub(crate) extern "system" fn create_graphics(
    _: This,
    desc: *const GraphicsHead,
    iid: *const Iid,
    out: *mut This,
) -> HRESULT {
    // SAFETY: the title passes a description (null is tolerated).
    let Some(desc) = (unsafe { desc.as_ref() }) else {
        return E_INVALIDARG;
    };
    let stages = GRAPHICS_STAGES.iter().zip(&desc.stages);
    let shaders = stages.filter_map(|(s, c)| copy(s, c)).collect();
    hand_out(
        shaders,
        #[cfg(windows)]
        None,
        iid,
        out,
    )
}

/// Device method.
pub(crate) extern "system" fn create_compute(
    _: This,
    desc: *const ComputeHead,
    iid: *const Iid,
    out: *mut This,
) -> HRESULT {
    // SAFETY: the title passes a description (null is tolerated).
    let Some(desc) = (unsafe { desc.as_ref() }) else {
        return E_INVALIDARG;
    };
    let shader = copy("compute", &desc.stage);
    // A container of another compiler's code only (`ACCC`: Grand Theft Auto
    // VI's upscaler passes) does not run here: the title is told, and does
    // without (`VICTORY_ACCC=1` takes them, their dispatches are dropped).
    #[cfg(windows)]
    if let Some(s) = &shader
        && victory_shader::Container::parse(&s.container)
            .is_ok_and(|c| c.part(victory_shader::FourCC::DXIL).is_none())
        && std::env::var("VICTORY_ACCC").as_deref() == Ok("0")
    {
        tracing::warn!(hash = %s.hash, "a compute pipeline without DXIL is not made");
        if !out.is_null() {
            // SAFETY: the title's out pointer.
            unsafe { *out = std::ptr::null() };
        }
        return E_NOTIMPL;
    }
    hand_out(
        shader.into_iter().collect(),
        #[cfg(windows)]
        None,
        iid,
        out,
    )
}

/// Device method 0x1f0: the plain compute description plus console
/// extensions `(desc, extension count, extensions, iid, out)`.
/// shortcut: the extensions (console shader settings) are ignored, as is
/// the rest of the description.
pub(crate) extern "system" fn create_compute_x(
    this: This,
    desc: *const ComputeHead,
    _extensions: u32,
    _extended: *const u8,
    iid: *const Iid,
    out: *mut This,
) -> HRESULT {
    create_compute(this, desc, iid, out)
}

/// A pipeline described as a stream: its size and the subobjects, each a
/// type then its data, padded to 8 bytes.
#[repr(C)]
pub(crate) struct StreamDesc {
    size: usize,
    stream: *const u8,
}

/// A subobject type's size (type included), and the stage it is if it is a
/// shader. The console numbers amplification and mesh shaders 23 and 24
/// (PC: 24 and 25); its own types from 25 on are not sized yet.
fn subobject(kind: u32) -> Option<(usize, Option<&'static str>)> {
    let stage = |name| Some((24, Some(name)));
    match kind {
        1 => stage("vertex"),
        2 => stage("pixel"),
        3 => stage("domain"),
        4 => stage("hull"),
        5 => stage("geometry"),
        6 => stage("compute"),
        23 => stage("amplification"),
        24 => stage("mesh"),
        0 => Some((16, None)),
        12 | 19 => Some((24, None)),
        7 | 15 => Some((40, None)),
        8 => Some((336, None)),
        10 => Some((48, None)),
        11 => Some((56, None)),
        17 => Some((16, None)),
        21 => Some((64, None)),
        22 => Some((32, None)),
        9 | 13 | 14 | 16 | 18 | 20 => Some((8, None)),
        _ => None,
    }
}

/// Device method: a pipeline from a stream; the shaders are kept, and the
/// graphics state for the host pipeline.
pub(crate) extern "system" fn create_stream(
    _: This,
    desc: *const StreamDesc,
    iid: *const Iid,
    out: *mut This,
) -> HRESULT {
    // SAFETY: the title passes a description (null is tolerated).
    let Some(desc) = (unsafe { desc.as_ref() }) else {
        return E_INVALIDARG;
    };
    if desc.stream.is_null() {
        return E_INVALIDARG;
    }
    let mut shaders = Vec::new();
    #[cfg(windows)]
    let mut graphics = host::GraphicsState::default();
    let mut at = 0;
    while at + 8 <= desc.size {
        // SAFETY: inside the stream the title passes.
        let kind = unsafe { desc.stream.add(at).cast::<u32>().read_unaligned() };
        let Some((size, stage)) = subobject(kind) else {
            tracing::warn!(
                kind,
                at,
                "pipeline subobject of unknown size; the rest is ignored"
            );
            break;
        };
        if let Some(stage) = stage {
            // SAFETY: a shader subobject has its bytecode at +8.
            let code = unsafe { &*desc.stream.add(at + 8).cast::<Bytecode>() };
            shaders.extend(copy(stage, code));
        }
        #[cfg(windows)]
        // SAFETY: a whole subobject of this kind lies at `at`.
        unsafe {
            graphics.read(kind, desc.stream.add(at))
        };
        at += size;
    }
    hand_out(
        shaders,
        #[cfg(windows)]
        Some(Box::new(graphics)),
        iid,
        out,
    )
}

extern "system" fn query(this: This, iid: *const Iid, out: *mut This) -> HRESULT {
    let found = object::is(iid, &PIPELINE_IIDS).then_some(this);
    object::answer(PIPELINE, iid, found, out)
}
extern "system" fn release(this: This) -> u32 {
    object::release_boxed::<Pipeline>(this)
}

/// IUnknown 3, object 4, device child 1, pipeline 3.
static FUNCTIONS: Table<24> = {
    let mut f = object::d3d12_object(object::missing_table::<PIPELINE, 24>());
    f[0] = query as *const ();
    f[2] = release as *const ();
    f[7] = object::parent_device as *const (); // 0x38
    Table(f)
};

#[cfg(windows)]
pub(crate) mod host;
#[cfg(windows)]
mod library;
#[cfg(windows)]
mod make;

/// Keeps compiled host pipelines, and the list of title pipelines to start
/// early, in `dir` (a folder of the cache) across runs.
#[cfg(windows)]
pub fn use_pipeline_cache(dir: std::path::PathBuf) {
    library::use_pipeline_cache(dir);
}

/// Saves the pipeline cache now if anything was added (at the end of a run).
#[cfg(windows)]
pub fn save_pipeline_cache() {
    library::save(true);
}

#[cfg(test)]
mod tests {
    #[cfg(all(windows, feature = "diagnostics"))]
    #[test]
    fn skip_hashes_accepts_full_hashes_only() {
        assert!(super::parse_skip_hashes(" , ").is_empty());
        assert_eq!(
            super::parse_skip_hashes(" 0123456789ABCDEF, 0123456789abcde, xyz, 0011223344556677,"),
            ["0123456789abcdef", "0011223344556677"]
        );
    }

    use super::*;
    use crate::object::{S_OK, slot};

    fn shaders(p: This) -> Vec<(&'static str, Vec<u8>)> {
        pipeline(p)
            .shaders
            .iter()
            .map(|s| (s.stage, s.container.clone()))
            .collect()
    }

    #[test]
    fn pipelines_keep_their_shaders() {
        let (vs, ps) = ([1u8, 2, 3], [4u8; 5]);
        let none = || Bytecode {
            code: std::ptr::null(),
            size: 0,
        };
        let desc = GraphicsHead {
            root: std::ptr::null(),
            stages: [
                Bytecode {
                    code: vs.as_ptr(),
                    size: vs.len(),
                },
                Bytecode {
                    code: ps.as_ptr(),
                    size: ps.len(),
                },
                none(),
                none(),
                none(),
            ],
        };
        let mut p = std::ptr::null();
        assert_eq!(
            create_graphics(std::ptr::null(), &desc, &PIPELINE_IIDS[0], &mut p),
            S_OK
        );
        assert_eq!(
            shaders(p),
            [("vertex", vs.to_vec()), ("pixel", ps.to_vec())]
        );
        // A second pipeline with the same pixel shader shares its container.
        let mut q = std::ptr::null();
        create_graphics(std::ptr::null(), &desc, &PIPELINE_IIDS[0], &mut q);
        assert!(Arc::ptr_eq(
            &pipeline(p).shaders[1],
            &pipeline(q).shaders[1]
        ));
        assert_eq!(slot::<extern "system" fn(This) -> u32>(p, 0x10)(p), 0);
    }

    /// The stream layout Grand Theft Auto VI builds (title+0x86f090): root
    /// signature, five shader slots (the first a mesh shader), blend, sample
    /// mask, rasterizer, depth-stencil 1, sample description, input layout,
    /// topology, render target and depth formats: 0x2b0 bytes.
    #[test]
    fn streams_yield_their_shaders() {
        let (ms, ps) = ([9u8; 7], [4u8; 5]);
        let mut stream = vec![0u8; 0x2b0];
        let mut put = |at: usize, v: u64| stream[at..at + 8].copy_from_slice(&v.to_le_bytes());
        for (at, kind) in [
            (0x10, 24),
            (0x28, 2),
            (0x40, 3),
            (0x58, 4),
            (0x70, 5),
            (0x88, 8),
            (0x1d8, 9),
            (0x1e0, 10),
            (0x210, 21),
            (0x250, 17),
            (0x260, 12),
            (0x278, 14),
            (0x280, 15),
            (0x2a8, 16),
        ] {
            put(at, kind);
        }
        put(0x18, ms.as_ptr() as u64);
        put(0x20, ms.len() as u64);
        put(0x30, ps.as_ptr() as u64);
        put(0x38, ps.len() as u64);
        // Topology type 3 (triangles), one render target of format 28.
        put(0x278, 14 | 3 << 32);
        put(0x280, 15 | 28 << 32);
        put(0x2a0, 1 << 32);
        let desc = StreamDesc {
            size: stream.len(),
            stream: stream.as_ptr(),
        };
        let mut p = std::ptr::null();
        let hr = create_stream(std::ptr::null(), &desc, &PIPELINE_IIDS[0], &mut p);
        assert_eq!(hr, S_OK);
        assert_eq!(shaders(p), [("mesh", ms.to_vec()), ("pixel", ps.to_vec())]);
        #[cfg(windows)]
        {
            let g = pipeline(p).host.graphics.as_deref().unwrap();
            assert_eq!(g.topology.0, 3);
            assert_eq!(
                (g.targets.NumRenderTargets, g.targets.RTFormats[0].0),
                (1, 28)
            );
        }
    }
}

#[cfg(all(test, windows))]
mod depth_bounds_test;
