//! Precompiled pipelines: device methods 0x1c8 (graphics) and 0x218
//! (compute), the console's `DeserializeGraphicsPipelineStateX` and
//! `DeserializeComputePipelineStateX` (slots 57 and 67; 56 and 66 serialise).
//!
//! Both are `(this, description, interface id, out)`; the title asks for an
//! `ID3D12PipelineState`. The description is the root signature followed by
//! `(pointer, size)` pairs into the title's loaded copy of
//! `platform:/shaders/v2.d3d12PipelineCache` (format:
//! [`victory_game::pso_cache`]):
//!
//! ```text
//! +0x00 root signature (an object of ours)
//! pair 0, 1   the pipeline's state blocks (compute 0x1c and 0x14 bytes;
//!             graphics 380 to 520 and 80 to 340: GFX10 register state and
//!             the input layout)
//! then per shader stage slot a pair of its native code and its metadata
//! block; a stage the pipeline lacks is zero. Compute has one slot;
//! graphics three read here: pixel, vertex, mesh.
//! ```
//!
//! The code is native GFX10 compiled for the pipeline, not the archive's
//! programs, and comes with none of the signature parts the translator maps
//! stage inputs and outputs by. But each stage names its source shader: its
//! code's prologue and its metadata spell the digest of the container's
//! `HASH` part, and 99.94 % of the file's code blocks name a container of
//! the title's shader archive. So a stage is made from that container,
//! read from the archive ([`super::pack`]), through the same path as the
//! pipelines the title creates from containers.
//!
//! What the title does without one (title+0x2479760 for graphics,
//! +0x247fd30 and +0xf9f650 for compute): it creates the same pipeline from
//! its container, graphics with its full stream state. So a pipeline we
//! cannot make is refused, never handed out empty.
//!
//! `VICTORY_PRECOMPILED_PSO`:
//! - unset or `on`: compute pipelines get a host pipeline from the archive's
//!   container; so do graphics ones with `VICTORY_PRECOMPILED_GRAPHICS=1`,
//!   with the state their state blocks decode to
//!   ([`victory_game::pso_state`]). Without it graphics ones are refused,
//!   as before, and the title creates them from its stream (the decoding
//!   is checked against the title's own descriptions, not yet in a run of
//!   its pipelines). A pipeline whose shader is not found, or whose state
//!   does not decode, is refused.
//! - `stub`: as the first implementation: every pipeline is handed out
//!   without a host pipeline, its draws and dispatches dropped.
//! - `off`: both methods fail, with the error of a pipeline blob made by
//!   another driver.
//!
//! `VICTORY_PRECOMPILED_DUMP=<dir>` writes the blocks of the first calls of
//! each method to files.
use std::collections::HashSet;
use std::sync::atomic::{AtomicU64, Ordering};
use std::sync::{Mutex, OnceLock};

use victory_game::{pso_cache, pso_state};

use super::{hand_out, pack};
use crate::object::{E_INVALIDARG, HRESULT, Iid, This};

/// `D3D12_ERROR_DRIVER_VERSION_MISMATCH`: what D3D12 answers a pipeline
/// blob made by another driver.
pub(crate) const DRIVER_VERSION_MISMATCH: HRESULT = 0x887E_0002_u32 as i32;

/// Blocks bigger than this are not read (the largest code block of the
/// title's file is 0x1ed40 bytes).
const MAX_BLOCK: usize = 1 << 24;
/// Stage slots a graphics description is read for (pixel, vertex, mesh).
const GRAPHICS_STAGE_SLOTS: usize = 3;
const STATE_BLOCKS: usize = 2;
/// Blocks written to `VICTORY_PRECOMPILED_DUMP`, per method.
const DUMPED: u64 = 8;

#[derive(Clone, Copy, PartialEq, Eq, Debug)]
pub(crate) enum Kind {
    Graphics,
    Compute,
}

impl Kind {
    fn slot(self) -> usize {
        match self {
            Kind::Graphics => 0x1c8,
            Kind::Compute => 0x218,
        }
    }
    fn stage_slots(self) -> usize {
        match self {
            Kind::Graphics => GRAPHICS_STAGE_SLOTS,
            Kind::Compute => 1,
        }
    }
    /// The stage a slot holds.
    fn stage(self, slot: usize) -> Option<&'static str> {
        match self {
            Kind::Graphics => pso_cache::slot_stage(slot),
            Kind::Compute => (slot == 0).then_some("compute"),
        }
    }
}

/// What `VICTORY_PRECOMPILED_PSO` asks for.
#[derive(Clone, Copy, PartialEq, Eq, Debug)]
enum Mode {
    Real,
    Stub,
    Off,
}

fn mode_of(value: Option<&str>) -> Mode {
    match value.map(|v| v.trim().to_ascii_lowercase()).as_deref() {
        Some("off" | "0" | "false" | "no") => Mode::Off,
        Some("stub") => Mode::Stub,
        _ => Mode::Real,
    }
}

fn mode() -> Mode {
    static MODE: OnceLock<Mode> = OnceLock::new();
    *MODE.get_or_init(|| {
        let mode = mode_of(std::env::var("VICTORY_PRECOMPILED_PSO").ok().as_deref());
        match mode {
            Mode::Off => tracing::warn!("VICTORY_PRECOMPILED_PSO=off: precompiled pipelines fail as before"),
            Mode::Stub => tracing::warn!(
                "VICTORY_PRECOMPILED_PSO=stub: precompiled pipelines are made without host pipelines (their draws are dropped)"
            ),
            Mode::Real => {}
        }
        mode
    })
}

/// Whether graphics pipelines are handed out with the state their blocks
/// decode to (`VICTORY_PRECOMPILED_GRAPHICS=1`); else they are refused and
/// the title makes them from its stream, as before.
fn graphics_wanted() -> bool {
    static ON: OnceLock<bool> = OnceLock::new();
    *ON.get_or_init(|| {
        let on = std::env::var_os("VICTORY_PRECOMPILED_GRAPHICS").is_some_and(|v| v == "1");
        if on {
            tracing::info!("VICTORY_PRECOMPILED_GRAPHICS=1: precompiled graphics pipelines are made from their decoded state");
        }
        on
    })
}

/// Whether precompiled pipelines are made from the shader archive (else it
/// is not indexed).
pub(crate) fn wants_archive() -> bool {
    mode() == Mode::Real
}

/// One block of a description: how long it is and the first 8 bytes of its
/// BLAKE3.
#[derive(Clone, Debug, PartialEq, Eq)]
pub(crate) struct Block {
    pub(crate) size: usize,
    pub(crate) hash: String,
}

/// A shader stage: its native code and metadata blocks, and the digest of
/// the container it was compiled from.
#[derive(Clone, Debug, PartialEq, Eq)]
pub(crate) struct Stage {
    pub(crate) slot: usize,
    pub(crate) code: Block,
    pub(crate) meta: Block,
    pub(crate) digest: Option<[u8; 16]>,
}

/// What a precompiled pipeline was made from.
#[derive(Clone, Debug)]
pub(crate) struct Precompiled {
    pub(crate) kind: Kind,
    pub(crate) root: usize,
    pub(crate) state: Vec<Block>,
    pub(crate) stages: Vec<Stage>,
}

#[repr(C)]
#[derive(Clone, Copy)]
struct Pair {
    data: *const u8,
    size: usize,
}

/// The bytes of a block, or why they are not one.
///
/// # Safety
/// A non-null `data` of a non-zero `size` up to `MAX_BLOCK` is readable.
unsafe fn bytes<'a>(pair: Pair) -> Option<&'a [u8]> {
    if pair.data.is_null() || pair.size == 0 || pair.size > MAX_BLOCK {
        return None;
    }
    // SAFETY: the caller's.
    Some(unsafe { std::slice::from_raw_parts(pair.data, pair.size) })
}

fn block(data: &[u8]) -> Block {
    Block {
        size: data.len(),
        hash: blake3::hash(data).to_hex()[..16].to_owned(),
    }
}

fn hex(digest: &[u8; 16]) -> String {
    digest.iter().map(|b| format!("{b:02x}")).collect()
}

/// Reads a description of `kind`: the root signature word and
/// `2 + 2 * stage slots` pairs. `None` for what a pipeline cannot be made
/// from: a missing state block, or no stage at all.
///
/// # Safety
/// `desc` points at the title's description; the pairs' pointers are valid
/// for their sizes.
unsafe fn read(kind: Kind, desc: *const u8) -> Option<(Precompiled, Vec<Vec<u8>>)> {
    // SAFETY: root signature word, then pairs (a graphics description is
    // 1 + 2 * (2 + 2 * 3) words, a compute one 9).
    let root = unsafe { desc.cast::<usize>().read_unaligned() };
    // SAFETY: as above.
    let pair = |i: usize| unsafe { desc.add(8).cast::<Pair>().add(i).read_unaligned() };
    let mut blobs = Vec::new();
    let mut state = Vec::new();
    for i in 0..STATE_BLOCKS {
        // SAFETY: as above.
        let data = unsafe { bytes(pair(i)) }?;
        state.push(block(data));
        blobs.push(data.to_vec());
    }
    let mut stages = Vec::new();
    for s in 0..kind.stage_slots() {
        let at = STATE_BLOCKS + 2 * s;
        // SAFETY: as above.
        let (code, meta) = unsafe { (bytes(pair(at)), bytes(pair(at + 1))) };
        // A stage the pipeline lacks is zero in both.
        let Some(code) = code else { continue };
        let meta = meta.unwrap_or_default();
        stages.push(Stage {
            slot: s,
            code: block(code),
            meta: block(meta),
            digest: pso_cache::stage_shader_hash(code, meta),
        });
        blobs.push(code.to_vec());
        blobs.push(meta.to_vec());
    }
    if stages.is_empty() {
        return None;
    }
    Some((
        Precompiled {
            kind,
            root,
            state,
            stages,
        },
        blobs,
    ))
}

/// Counts per method: calls, created (with a host pipeline to come),
/// handed out without one (`stub`), refused.
static COUNTS: [[AtomicU64; 4]; 2] = [const { [const { AtomicU64::new(0) }; 4] }; 2];
const CALLS: usize = 0;
const MADE: usize = 1;
const EMPTY: usize = 2;
const REFUSED: usize = 3;

/// The counts of a method (for tests and reports).
#[cfg(test)]
fn counts(kind: Kind) -> [u64; 4] {
    COUNTS[kind as usize].each_ref().map(|c| c.load(Ordering::Relaxed))
}

fn dump(kind: Kind, n: u64, blobs: &[Vec<u8>]) {
    static DIR: OnceLock<Option<std::path::PathBuf>> = OnceLock::new();
    let Some(dir) = DIR.get_or_init(|| std::env::var_os("VICTORY_PRECOMPILED_DUMP").map(Into::into)) else {
        return;
    };
    if n > DUMPED {
        return;
    }
    let _ = std::fs::create_dir_all(dir);
    for (i, data) in blobs.iter().enumerate() {
        let name = format!("{:#x}-{n}-{i}.bin", kind.slot());
        let _ = std::fs::write(dir.join(name), data);
    }
}

/// Refuses a call, logging the first refusal of each kind and method.
fn refuse(kind: Kind, why: &'static str, detail: String) -> HRESULT {
    static SEEN: Mutex<Option<HashSet<(usize, &'static str)>>> = Mutex::new(None);
    COUNTS[kind as usize][REFUSED].fetch_add(1, Ordering::Relaxed);
    let first = SEEN
        .lock()
        .unwrap_or_else(|e| e.into_inner())
        .get_or_insert_with(HashSet::new)
        .insert((kind.slot(), why));
    if first {
        tracing::warn!(
            slot = format_args!("{:#x}", kind.slot()),
            why,
            detail = %detail,
            "precompiled pipeline refused (first of its kind; the title makes it from its container)"
        );
    }
    DRIVER_VERSION_MISMATCH
}

/// The shaders of a precompiled pipeline, each from the archive's container
/// its stage names; or why it has none.
fn shaders(p: &Precompiled) -> Result<Vec<std::sync::Arc<super::Shader>>, (&'static str, String)> {
    p.stages
        .iter()
        .map(|s| {
            let stage = p
                .kind
                .stage(s.slot)
                .ok_or_else(|| ("stage slot of unknown kind", format!("slot {}", s.slot)))?;
            let digest = s.digest.ok_or_else(|| ("stage names no shader", format!("{stage} code {}", s.code.hash)))?;
            let container = pack::container(&digest).map_err(|m| (m.why(), format!("{stage} {}", hex(&digest))))?;
            Ok(super::shader(stage, &container))
        })
        .collect()
}

/// The method behind both slots.
fn create_as(kind: Kind, desc: *const u8, iid: *const Iid, out: *mut This) -> HRESULT {
    let counts = &COUNTS[kind as usize];
    let n = counts[CALLS].fetch_add(1, Ordering::Relaxed) + 1;
    if !out.is_null() {
        // SAFETY: the title's out pointer (a local of its own).
        unsafe { *out = std::ptr::null() };
    }
    let mode = mode();
    if mode == Mode::Off {
        counts[REFUSED].fetch_add(1, Ordering::Relaxed);
        if n == 1 {
            tracing::warn!(slot = format_args!("{:#x}", kind.slot()), "precompiled pipelines are off");
        }
        return DRIVER_VERSION_MISMATCH;
    }
    let made = if desc.is_null() {
        None
    } else {
        // SAFETY: the title passes a description.
        unsafe { read(kind, desc) }
    };
    let Some((precompiled, blobs)) = made else {
        counts[REFUSED].fetch_add(1, Ordering::Relaxed);
        if n <= 4 {
            tracing::warn!(
                slot = format_args!("{:#x}", kind.slot()),
                calls = n,
                "precompiled pipeline description without state blocks or stages"
            );
        }
        return E_INVALIDARG;
    };
    dump(kind, n, &blobs);
    if n == 1 || n.is_power_of_two() {
        tracing::info!(
            slot = format_args!("{:#x}", kind.slot()),
            method = match kind {
                Kind::Graphics => "DeserializeGraphicsPipelineStateX",
                Kind::Compute => "DeserializeComputePipelineStateX",
            },
            calls = n,
            made = counts[MADE].load(Ordering::Relaxed),
            empty = counts[EMPTY].load(Ordering::Relaxed),
            refused = counts[REFUSED].load(Ordering::Relaxed),
            root = format_args!("{:#x}", precompiled.root),
            state = ?precompiled.state.iter().map(|b| (b.size, &b.hash)).collect::<Vec<_>>(),
            stages = ?precompiled
                .stages
                .iter()
                .map(|s| (kind.stage(s.slot).unwrap_or("?"), s.digest.as_ref().map(hex)))
                .collect::<Vec<_>>(),
            "precompiled pipeline"
        );
    }
    #[cfg(windows)]
    let mut graphics = None;
    let shaders = match mode {
        Mode::Stub => Vec::new(),
        _ => {
            let shaders = match shaders(&precompiled) {
                Ok(shaders) => shaders,
                Err((why, detail)) => return refuse(kind, why, detail),
            };
            if kind == Kind::Graphics {
                let names = || -> String {
                    let names: Vec<String> = shaders.iter().map(|s| format!("{} {}", s.stage, s.hash)).collect();
                    names.join(" + ")
                };
                if !graphics_wanted() {
                    // The title's own fallback makes this pipeline from its
                    // stream.
                    return refuse(kind, "graphics pipelines off (VICTORY_PRECOMPILED_GRAPHICS)", names());
                }
                let state = match pso_state::decode(&blobs[0], &blobs[1]) {
                    Ok(state) => state,
                    Err(e) => return refuse(kind, "graphics state blocks do not decode", format!("{e}: {}", names())),
                };
                #[cfg(windows)]
                {
                    graphics = Some(Box::new(super::host::GraphicsState::from_precompiled(&state)));
                }
                #[cfg(not(windows))]
                let _ = state;
            }
            shaders
        }
    };
    let empty = shaders.is_empty();
    let hr = hand_out(
        shaders,
        #[cfg(windows)]
        graphics,
        Some(precompiled),
        iid,
        out,
    );
    if hr < 0 {
        counts[REFUSED].fetch_add(1, Ordering::Relaxed);
    } else if empty {
        counts[EMPTY].fetch_add(1, Ordering::Relaxed);
    } else {
        counts[MADE].fetch_add(1, Ordering::Relaxed);
    }
    hr
}

/// Device method 0x1c8 (`SLOT` 0x1c8) or 0x218 (`SLOT` 0x218).
pub(crate) extern "system" fn create_precompiled<const SLOT: usize>(
    _: This,
    desc: *const u8,
    iid: *const Iid,
    out: *mut This,
) -> HRESULT {
    create_as(
        if SLOT == 0x218 { Kind::Compute } else { Kind::Graphics },
        desc,
        iid,
        out,
    )
}

/// What [`precompile_graphics`] did with the title's graphics records.
#[derive(Debug, Default, Clone, Copy)]
pub struct PrecompileReport {
    pub records: usize,
    /// Records whose state blocks did not decode.
    pub undecoded: usize,
    /// Records with a stage whose shader is not in the archive (or names
    /// none, or is of an unknown slot).
    pub unmapped: usize,
    /// Records that are another's pipeline (same shaders and state).
    pub duplicates: usize,
    /// Host pipelines made (or found in the library), refused by the
    /// host (translation or driver), or left out by policy.
    pub made: usize,
    pub failed: usize,
    pub left_out: usize,
}

/// Offline: makes the host pipeline of every graphics record of the
/// title's pipeline cache (found in the shader archive `archive`) into
/// the pipeline library, so that a run finds them there, at the key a
/// run names them by when the title hands out the same pipeline (with
/// `VICTORY_PRECOMPILED_GRAPHICS=1`). Runs `jobs` workers at the lowest
/// thread priority, in background mode (lowered I/O and memory priority
/// too), so it can go on beside other work; needs a host GPU. Stops after
/// `limit` records if given, after the first `skip`; with `mesh_only`,
/// only mesh-shader records count. The caller saves the library
/// ([`super::save_pipeline_cache`]).
#[cfg(windows)]
pub fn precompile_graphics(
    archive: &std::path::Path,
    index: Option<&std::path::Path>,
    jobs: usize,
    limit: Option<usize>,
    skip: usize,
    mesh_only: bool,
    progress: &(dyn Fn(usize, &PrecompileReport) + Sync),
) -> Result<PrecompileReport, String> {
    use std::sync::atomic::AtomicUsize;

    background();
    if crate::host::host().is_none() {
        return Err("no host GPU: graphics pipelines are not precompiled".into());
    }
    let shaders = pack::use_blocking(archive, index).map_err(|e| format!("shader archive {}: {e}", archive.display()))?;
    let bytes = std::fs::read(archive).map_err(|e| format!("{}: {e}", archive.display()))?;
    let file = pso_cache::find_in_archive(&bytes).ok_or("no pipeline cache in the shader archive")?;
    drop(bytes);
    let cache = pso_cache::Cache::parse(&file)?;
    // The record numbers to make, in order.
    let chosen: Vec<usize> = (0..cache.graphics_count())
        .filter(|&i| !mesh_only || cache.graphics(i).is_some_and(|g| g.stages[2].is_some()))
        .skip(skip)
        .take(limit.unwrap_or(usize::MAX))
        .collect();
    let records = chosen.len();
    tracing::info!(records, skip, mesh_only, shaders, jobs, "precompiling the title's graphics pipelines");

    let next = AtomicUsize::new(0);
    let keys = Mutex::new(HashSet::new());
    let report = Mutex::new(PrecompileReport {
        records,
        ..Default::default()
    });
    let count = |f: &dyn Fn(&mut PrecompileReport)| f(&mut report.lock().unwrap_or_else(|e| e.into_inner()));
    std::thread::scope(|s| {
        for _ in 0..jobs.max(1) {
            s.spawn(|| {
                background();
                loop {
                    let n = next.fetch_add(1, Ordering::Relaxed);
                    let Some(&i) = chosen.get(n) else { break };
                    let Some(g) = cache.graphics(i) else { continue };
                    let state = cache
                        .block(g.state[0])
                        .zip(cache.block(g.state[1]))
                        .ok_or(pso_state::Error::Registers)
                        .and_then(|(a, b)| pso_state::decode(a, b));
                    let Ok(state) = state else {
                        count(&|r| r.undecoded += 1);
                        continue;
                    };
                    let stages: Option<Vec<_>> = g
                        .stages
                        .iter()
                        .enumerate()
                        .filter_map(|(slot, st)| st.map(|st| (slot, st)))
                        .map(|(slot, (code, meta))| {
                            let stage = pso_cache::slot_stage(slot)?;
                            let digest =
                                pso_cache::stage_shader_hash(cache.code(code)?, cache.block(meta).unwrap_or_default())?;
                            Some(super::shader(stage, &pack::container(&digest).ok()?))
                        })
                        .collect();
                    let Some(stages) = stages.filter(|s| !s.is_empty()) else {
                        count(&|r| r.unmapped += 1);
                        continue;
                    };
                    let graphics = super::host::GraphicsState::from_precompiled(&state);
                    let (key, made) = super::make::warm_graphics(stages, graphics);
                    if !keys.lock().unwrap_or_else(|e| e.into_inner()).insert(key) {
                        count(&|r| r.duplicates += 1);
                        continue;
                    }
                    if made == Some(false) {
                        tracing::warn!(record = i, "precompile: the record's host pipeline failed");
                    }
                    count(&|r| match made {
                        Some(true) => r.made += 1,
                        Some(false) => r.failed += 1,
                        None => r.left_out += 1,
                    });
                    if (n + 1) % 500 == 0 {
                        progress(n + 1, &report.lock().unwrap_or_else(|e| e.into_inner()));
                    }
                }
            });
        }
    });
    Ok(report.into_inner().unwrap_or_else(|e| e.into_inner()))
}

/// The calling thread at the lowest priority, in background mode.
#[cfg(windows)]
fn background() {
    use windows_sys::Win32::System::Threading::{
        GetCurrentThread, SetThreadPriority, THREAD_MODE_BACKGROUND_BEGIN, THREAD_PRIORITY_LOWEST,
    };
    // SAFETY: the pseudo handle of the calling thread.
    unsafe {
        if SetThreadPriority(GetCurrentThread(), THREAD_MODE_BACKGROUND_BEGIN) == 0 {
            SetThreadPriority(GetCurrentThread(), THREAD_PRIORITY_LOWEST);
        }
    }
}

#[cfg(test)]
pub(crate) mod tests {
    use super::*;
    use crate::object::{S_OK, slot};

    /// A container with only a `HASH` part of `digest` and some code bytes.
    pub(crate) fn container_with_hash(digest: [u8; 16], code: &[u8]) -> Vec<u8> {
        let mut out = b"DXBC".to_vec();
        out.extend_from_slice(&[0; 16]);
        out.extend_from_slice(&1u32.to_le_bytes());
        out.extend_from_slice(&0u32.to_le_bytes());
        out.extend_from_slice(&2u32.to_le_bytes());
        out.extend_from_slice(&0x28u32.to_le_bytes());
        out.extend_from_slice(&(0x28 + 28u32).to_le_bytes());
        out.extend_from_slice(b"HASH");
        out.extend_from_slice(&20u32.to_le_bytes());
        out.extend_from_slice(&0u32.to_le_bytes());
        out.extend_from_slice(&digest);
        out.extend_from_slice(b"CODE");
        out.extend_from_slice(&(code.len() as u32).to_le_bytes());
        out.extend_from_slice(code);
        let size = out.len() as u32;
        out[0x18..0x1c].copy_from_slice(&size.to_le_bytes());
        out
    }

    /// A metadata block naming `digest` (graphics layout: 16-byte head,
    /// then the 0x43 entry).
    fn meta_naming(digest: [u8; 16]) -> Vec<u8> {
        let mut m = Vec::new();
        for w in [2u32, 0x24, 0x76, 0, 0x43] {
            m.extend_from_slice(&w.to_le_bytes());
        }
        m.extend_from_slice(&digest);
        m
    }

    /// The compute description of the title's logs: root, then 0x1c and 0x14
    /// bytes of state, the code, the metadata.
    fn compute_words(state: &[u8; 0x20], code: &[u8], meta: &[u8]) -> [usize; 9] {
        [
            0x1234,
            state.as_ptr() as usize,
            0x1c,
            state[0x1c..].as_ptr() as usize,
            0x4,
            code.as_ptr() as usize,
            code.len(),
            meta.as_ptr() as usize,
            meta.len(),
        ]
    }

    /// A record of the title's file (record 9285, created by the title at
    /// run time with this state) through the decoder into the host state.
    #[cfg(windows)]
    #[test]
    fn a_decoded_record_gives_the_host_state_the_title_s_stream_gave() {
        use windows::Win32::Graphics::Direct3D12::*;
        let hex = |s: &str| -> Vec<u8> {
            let s = s.trim();
            (0..s.len()).step_by(2).map(|i| u8::from_str_radix(&s[i..i + 2], 16).unwrap()).collect()
        };
        let a = hex(include_str!("../../../victory-game/src/pso_state_fixtures/r9285.a"));
        let b = hex(include_str!("../../../victory-game/src/pso_state_fixtures/r9285.b"));
        let d = pso_state::decode(&a, &b).unwrap();
        let g = super::super::host::GraphicsState::from_precompiled(&d);
        let text = g.describe();
        assert!(text.contains("POSITION"), "{text}");
        assert_eq!(g.topology, D3D12_PRIMITIVE_TOPOLOGY_TYPE_TRIANGLE);
        assert_eq!(g.targets.NumRenderTargets, 1);
    }

    #[test]
    fn modes_from_the_environment_value() {
        assert_eq!(mode_of(None), Mode::Real);
        assert_eq!(mode_of(Some("on")), Mode::Real);
        assert_eq!(mode_of(Some(" Stub ")), Mode::Stub);
        for off in ["off", "0", "FALSE", "no"] {
            assert_eq!(mode_of(Some(off)), Mode::Off);
        }
    }

    #[test]
    fn the_description_is_read_with_each_stage_s_digest() {
        let state = [1u8; 0x184];
        let (extra, ps, ps_meta, ms) = ([2u8; 0xbc], [3u8; 0x200], meta_naming([0x44; 16]), [5u8; 0x140]);
        let pair = |b: &[u8]| [b.as_ptr() as usize, b.len()];
        let mut words = vec![0x55usize];
        for b in [&state[..], &extra, &ps, &ps_meta] {
            words.extend(pair(b));
        }
        // The vertex slot is empty; the mesh stage has no metadata.
        words.extend([0, 0]);
        words.extend([0, 0]);
        words.extend(pair(&ms));
        words.extend([0, 0]);
        // SAFETY: the words describe live arrays.
        let (p, blobs) = unsafe { read(Kind::Graphics, words.as_ptr().cast()) }.unwrap();
        assert_eq!(p.root, 0x55);
        assert_eq!(p.state.iter().map(|b| b.size).collect::<Vec<_>>(), [0x184, 0xbc]);
        let stages: Vec<_> = p.stages.iter().map(|s| (Kind::Graphics.stage(s.slot), s.code.size, s.digest)).collect();
        assert_eq!(stages, [(Some("pixel"), 0x200, Some([0x44; 16])), (Some("mesh"), 0x140, None)]);
        assert_eq!(blobs.len(), 2 + 2 * 2);
    }

    #[test]
    fn a_compute_stage_is_made_from_the_archive_s_container() {
        let dir = tempfile::tempdir().unwrap();
        let container = container_with_hash([0x77; 16], b"the program");
        let archive = dir.path().join("shaders.rpf");
        std::fs::write(&archive, crate::pipeline::pack::tests::archive_of(std::slice::from_ref(&container))).unwrap();
        pack::use_now(&archive);

        let (state, code, meta) = ([7u8; 0x20], [0x90u8; 0x180], meta_naming([0x77; 16]));
        let words = compute_words(&state, &code, &meta);
        let iid = crate::pipeline::PIPELINE_IIDS[0];
        let mut p = std::ptr::null();
        let before = counts(Kind::Compute);
        assert_eq!(create_precompiled::<0x218>(std::ptr::null(), words.as_ptr().cast(), &iid, &mut p), S_OK);
        assert!(!p.is_null());
        let made = super::super::pipeline(p);
        assert_eq!(made.precompiled.as_ref().unwrap().kind, Kind::Compute);
        assert_eq!(made.shaders.len(), 1);
        assert_eq!((made.shaders[0].stage, &made.shaders[0].container), ("compute", &container));
        // The same container the title would pass shares its translation.
        assert!(std::sync::Arc::ptr_eq(&made.shaders[0], &super::super::shader("compute", &container)));
        assert!(counts(Kind::Compute)[MADE] > before[MADE]);
        assert_eq!(slot::<extern "system" fn(This) -> u32>(p, 0x10)(p), 0);

        // A stage whose shader is not in the archive is refused, so the
        // title makes the pipeline itself.
        let other = meta_naming([0x78; 16]);
        let words = compute_words(&state, &code, &other);
        assert_eq!(
            create_precompiled::<0x218>(std::ptr::null(), words.as_ptr().cast(), &iid, &mut p),
            DRIVER_VERSION_MISMATCH
        );
        assert!(p.is_null());

        // Graphics: the shaders are found, but graphics pipelines are off
        // (`VICTORY_PRECOMPILED_GRAPHICS` unset): refused.
        let (gstate, extra) = ([1u8; 0x184], [2u8; 0x50]);
        let pair = |b: &[u8]| [b.as_ptr() as usize, b.len()];
        let mut words = vec![0x55usize];
        for b in [&gstate[..], &extra, &code, &meta] {
            words.extend(pair(b));
        }
        words.extend([0; 8]);
        let before = counts(Kind::Graphics);
        assert_eq!(
            create_precompiled::<0x1c8>(std::ptr::null(), words.as_ptr().cast(), &iid, &mut p),
            DRIVER_VERSION_MISMATCH
        );
        assert!(p.is_null());
        assert!(counts(Kind::Graphics)[REFUSED] > before[REFUSED]);
    }

    #[test]
    fn a_description_without_state_or_stages_is_refused() {
        let iid = crate::pipeline::PIPELINE_IIDS[0];
        let mut p = std::ptr::null();
        assert_eq!(create_precompiled::<0x1c8>(std::ptr::null(), std::ptr::null(), &iid, &mut p), E_INVALIDARG);
        assert!(p.is_null());
        let zeros = [0usize; 17];
        assert_eq!(create_precompiled::<0x218>(std::ptr::null(), zeros.as_ptr().cast(), &iid, &mut p), E_INVALIDARG);
        assert!(p.is_null());
    }

    #[test]
    fn a_stage_that_names_no_shader_is_refused() {
        let (state, code, meta) = ([7u8; 0x20], [0x90u8; 0x180], [3u8; 0x118]);
        let words = compute_words(&state, &code, &meta);
        let iid = crate::pipeline::PIPELINE_IIDS[0];
        let mut p = std::ptr::null();
        assert_eq!(
            create_precompiled::<0x218>(std::ptr::null(), words.as_ptr().cast(), &iid, &mut p),
            DRIVER_VERSION_MISMATCH
        );
        assert!(p.is_null());
        // Asked only whether it could be made, with another interface: the
        // same refusal, before the interface is looked at.
        assert_eq!(
            create_precompiled::<0x218>(std::ptr::null(), words.as_ptr().cast(), &[0; 16], std::ptr::null_mut()),
            DRIVER_VERSION_MISMATCH
        );
    }
}
