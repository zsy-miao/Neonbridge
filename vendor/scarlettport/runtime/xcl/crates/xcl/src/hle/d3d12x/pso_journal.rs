//! The pipelines a title asked for, on disk (`<data>\cache\pipelines.bin`), so another process
//! can compile them: `xcl pipelines` reads the journal – during the run (`--watch`) or before
//! the next one – and fills the DXIL cache and the driver's with what the title will ask for.
//! Written for every pipeline with console shaders that goes to `pso_async` (its DXIL was not
//! in the cache, or it is made lazily); a pipeline's first build is marked as used, which the
//! tool compiles first. `XCL_PSO_JOURNAL=0`: off.
//!
//! Append-only frames, little-endian: tag u8, memory windows u8, 0 u16, payload length u32,
//! hash u64, payload. Tags: `S` a shader (its bytes; hash = fnv64 of them), `G` a graphics
//! pipeline, `C` a compute one, `U` (no payload) pipeline `hash` was built. A pipeline's
//! payload names its shaders by hash; each shader is written once, before the first pipeline
//! that uses it. Duplicates of earlier runs are not written again.

use std::collections::HashSet;
use std::io::{Seek, SeekFrom, Write};
use std::path::{Path, PathBuf};
use std::sync::Mutex;
use windows::Win32::Graphics::Direct3D12::*;

pub const MAGIC: &[u8; 8] = b"XCLPJ001";
pub const HEAD: usize = 16;

pub fn path_in(data_dir: &Path) -> PathBuf {
    data_dir.join("cache").join("pipelines.bin")
}

struct Journal {
    file: std::fs::File,
    seen: HashSet<(u8, u64)>,
}

static J: Mutex<Option<Option<Journal>>> = Mutex::new(None);

fn enabled() -> bool {
    std::env::var("XCL_PSO_JOURNAL").map_or(true, |v| v != "0")
}

fn open() -> Option<Journal> {
    use std::io::{Read, Seek, SeekFrom};
    use std::os::windows::fs::OpenOptionsExt;
    if !enabled() {
        return None;
    }
    let path = path_in(&crate::state::data_dir());
    let _ = std::fs::create_dir_all(path.parent()?);
    // the only writer: another title process gets no journal (`xcl pipelines` only reads)
    const FILE_SHARE_READ: u32 = 1;
    let mut file = std::fs::OpenOptions::new()
        .read(true)
        .write(true)
        .create(true)
        .truncate(false)
        .share_mode(FILE_SHARE_READ)
        .open(&path)
        .map_err(|e| crate::xlog_warn!("pipeline journal {}: {e} – none in this run", path.display()))
        .ok()?;
    let mut seen = HashSet::new();
    let len = file.metadata().map_or(0, |m| m.len());
    let mut magic = [0u8; 8];
    if file.read_exact(&mut magic).is_ok() && &magic == MAGIC {
        // the frames' headers only (the shaders' bytes stay on disk); a torn last frame (a
        // run killed mid-write) is cut off
        let mut at = MAGIC.len() as u64;
        let mut head = [0u8; HEAD];
        while file.read_exact(&mut head).is_ok() {
            let end = at + HEAD as u64 + u32::from_le_bytes(head[4..8].try_into().unwrap()) as u64;
            if end > len {
                break;
            }
            seen.insert((head[0], u64::from_le_bytes(head[8..16].try_into().unwrap())));
            at = end;
            if file.seek(SeekFrom::Start(at)).is_err() {
                break;
            }
        }
        if at < len {
            let _ = file.set_len(at);
        }
    } else {
        let _ = file.set_len(0);
        let _ = file.seek(SeekFrom::Start(0));
        let _ = file.write_all(MAGIC);
    }
    file.seek(SeekFrom::End(0)).ok()?;
    crate::xlog_info!("pipeline journal: {} ({} entries from earlier runs) – `xcl pipelines` compiles them outside the title", path.display(), seen.len());
    Some(Journal { file, seen })
}

/// Runs `f` on the journal; a write error turns it off for the rest of the run (0).
fn with(f: impl FnOnce(&mut Journal) -> std::io::Result<u64>) -> u64 {
    let mut g = J.lock().unwrap();
    let Some(j) = g.get_or_insert_with(open).as_mut() else { return 0 };
    match f(j) {
        Ok(h) => h,
        Err(e) => {
            crate::xlog_warn!("pipeline journal: {e}: off for this run");
            *g = Some(None);
            0
        }
    }
}

fn frame(tag: u8, windows: u8, hash: u64, payload: &[u8]) -> Vec<u8> {
    let mut v = Vec::with_capacity(HEAD + payload.len());
    v.extend_from_slice(&[tag, windows, 0, 0]);
    v.extend_from_slice(&(payload.len() as u32).to_le_bytes());
    v.extend_from_slice(&hash.to_le_bytes());
    v.extend_from_slice(payload);
    v
}

/// One frame of the journal.
pub struct Frame<'a> {
    pub tag: u8,
    pub windows: u8,
    pub hash: u64,
    pub payload: &'a [u8],
}

/// Call `f` on each whole frame of `bytes` (a journal from its start); the end of the last whole one.
pub fn for_each_frame<'a>(bytes: &'a [u8], f: impl FnMut(Frame<'a>)) -> usize {
    for_each_frame_from(bytes, MAGIC.len(), f)
}

/// `for_each_frame` from offset `at` (the end of an earlier call: a journal being written).
pub fn for_each_frame_from<'a>(bytes: &'a [u8], mut at: usize, mut f: impl FnMut(Frame<'a>)) -> usize {
    while at + HEAD <= bytes.len() {
        let n = u32::from_le_bytes(bytes[at + 4..at + 8].try_into().unwrap()) as usize;
        if at + HEAD + n > bytes.len() {
            break;
        }
        f(Frame { tag: bytes[at], windows: bytes[at + 1], hash: u64::from_le_bytes(bytes[at + 8..at + 16].try_into().unwrap()), payload: &bytes[at + HEAD..at + HEAD + n] });
        at += HEAD + n;
    }
    at.min(bytes.len())
}

/// Copy each field named into `out` at its offset in `T`: a desc's bytes without its pointers
/// and padding (both stay 0), so the same pipeline always gives the same bytes and hash.
macro_rules! fields {
    ($out:expr, $d:expr, $T:ty, $($($f:ident).+),+ $(,)?) => {{
        $(put_at(&mut $out, std::mem::offset_of!($T, $($f).+), &$d.$($f).+);)+
    }};
}

fn put_at<V>(out: &mut [u8], off: usize, v: &V) {
    let b = unsafe { std::slice::from_raw_parts(v as *const V as *const u8, std::mem::size_of::<V>()) };
    out[off..off + b.len()].copy_from_slice(b);
}

fn graphics_bytes(d: &D3D12_GRAPHICS_PIPELINE_STATE_DESC) -> Vec<u8> {
    type G = D3D12_GRAPHICS_PIPELINE_STATE_DESC;
    type R = D3D12_RENDER_TARGET_BLEND_DESC;
    let mut out = vec![0u8; std::mem::size_of::<G>()];
    fields!(out, d, G, BlendState.AlphaToCoverageEnable, BlendState.IndependentBlendEnable, SampleMask);
    let rt0 = std::mem::offset_of!(G, BlendState.RenderTarget);
    for (i, r) in d.BlendState.RenderTarget.iter().enumerate() {
        let mut one = vec![0u8; std::mem::size_of::<R>()];
        fields!(one, r, R, BlendEnable, LogicOpEnable, SrcBlend, DestBlend, BlendOp, SrcBlendAlpha, DestBlendAlpha, BlendOpAlpha, LogicOp, RenderTargetWriteMask);
        let at = rt0 + i * one.len();
        out[at..at + one.len()].copy_from_slice(&one);
    }
    fields!(
        out, d, G,
        RasterizerState.FillMode, RasterizerState.CullMode, RasterizerState.FrontCounterClockwise, RasterizerState.DepthBias,
        RasterizerState.DepthBiasClamp, RasterizerState.SlopeScaledDepthBias, RasterizerState.DepthClipEnable,
        RasterizerState.MultisampleEnable, RasterizerState.AntialiasedLineEnable, RasterizerState.ForcedSampleCount,
        RasterizerState.ConservativeRaster,
        DepthStencilState.DepthEnable, DepthStencilState.DepthWriteMask, DepthStencilState.DepthFunc, DepthStencilState.StencilEnable,
        DepthStencilState.StencilReadMask, DepthStencilState.StencilWriteMask,
        DepthStencilState.FrontFace.StencilFailOp, DepthStencilState.FrontFace.StencilDepthFailOp,
        DepthStencilState.FrontFace.StencilPassOp, DepthStencilState.FrontFace.StencilFunc,
        DepthStencilState.BackFace.StencilFailOp, DepthStencilState.BackFace.StencilDepthFailOp,
        DepthStencilState.BackFace.StencilPassOp, DepthStencilState.BackFace.StencilFunc,
        InputLayout.NumElements, IBStripCutValue, PrimitiveTopologyType, NumRenderTargets, RTVFormats, DSVFormat,
        SampleDesc.Count, SampleDesc.Quality, NodeMask, Flags,
    );
    out
}

fn compute_bytes(d: &D3D12_COMPUTE_PIPELINE_STATE_DESC) -> Vec<u8> {
    type C = D3D12_COMPUTE_PIPELINE_STATE_DESC;
    let mut out = vec![0u8; std::mem::size_of::<C>()];
    fields!(out, d, C, NodeMask, Flags);
    out
}

unsafe fn bytes_of<'a>(b: &D3D12_SHADER_BYTECODE) -> Option<&'a [u8]> {
    (!b.pShaderBytecode.is_null() && b.BytecodeLength != 0).then(|| std::slice::from_raw_parts(b.pShaderBytecode as *const u8, b.BytecodeLength))
}

/// Writes a frame once: all of it, or (an error) none of it, so later frames stay readable.
fn put(j: &mut Journal, tag: u8, windows: u8, hash: u64, payload: &[u8]) -> std::io::Result<()> {
    if j.seen.contains(&(tag, hash)) {
        return Ok(());
    }
    let at = j.file.stream_position()?;
    if let Err(e) = j.file.write_all(&frame(tag, windows, hash, payload)) {
        let _ = j.file.set_len(at);
        let _ = j.file.seek(SeekFrom::Start(at));
        return Err(e);
    }
    j.seen.insert((tag, hash));
    Ok(())
}

fn put_shader(j: &mut Journal, out: &mut Vec<u8>, sh: Option<&[u8]>) -> std::io::Result<()> {
    let h = sh.map_or(0, crate::gpu::shader::fnv64);
    if let Some(sh) = sh {
        put(j, b'S', 0, h, sh)?;
    }
    out.extend_from_slice(&h.to_le_bytes());
    Ok(())
}

fn finish(j: &mut Journal, tag: u8, mut payload: Vec<u8>) -> std::io::Result<u64> {
    // the memory windows are part of what the pipeline is (its HLSL differs with them)
    let windows = crate::gpu::recomp::pso::mem_windows() as u8;
    payload.push(windows);
    let hash = crate::gpu::shader::fnv64(&payload);
    payload.pop();
    put(j, tag, windows, hash, &payload)?;
    Ok(hash)
}

/// Note a graphics pipeline; its hash for `used`, 0 = not written.
pub unsafe fn graphics(d: &D3D12_GRAPHICS_PIPELINE_STATE_DESC) -> u64 {
    with(|j| graphics_in(j, d))
}

unsafe fn graphics_in(j: &mut Journal, d: &D3D12_GRAPHICS_PIPELINE_STATE_DESC) -> std::io::Result<u64> {
    let mut p = Vec::new();
    for b in [&d.VS, &d.PS, &d.DS, &d.HS, &d.GS] {
        put_shader(j, &mut p, bytes_of(b))?;
    }
    let desc = graphics_bytes(d);
    p.extend_from_slice(&(desc.len() as u32).to_le_bytes());
    p.extend_from_slice(&desc);
    let elems = if d.InputLayout.NumElements > 0 && !d.InputLayout.pInputElementDescs.is_null() {
        std::slice::from_raw_parts(d.InputLayout.pInputElementDescs, d.InputLayout.NumElements as usize)
    } else {
        &[]
    };
    p.extend_from_slice(&(elems.len() as u32).to_le_bytes());
    for e in elems {
        let name = std::ffi::CStr::from_ptr(e.SemanticName.0 as *const std::ffi::c_char).to_bytes();
        p.extend_from_slice(&(name.len() as u32).to_le_bytes());
        p.extend_from_slice(name);
        for w in [e.SemanticIndex, e.Format.0 as u32, e.InputSlot, e.AlignedByteOffset, e.InputSlotClass.0 as u32, e.InstanceDataStepRate] {
            p.extend_from_slice(&w.to_le_bytes());
        }
    }
    finish(j, b'G', p)
}

/// Note a compute pipeline; its hash for `used`, 0 = not written.
pub unsafe fn compute(d: &D3D12_COMPUTE_PIPELINE_STATE_DESC) -> u64 {
    with(|j| {
        let mut p = Vec::new();
        put_shader(j, &mut p, bytes_of(&d.CS))?;
        let desc = compute_bytes(d);
        p.extend_from_slice(&(desc.len() as u32).to_le_bytes());
        p.extend_from_slice(&desc);
        finish(j, b'C', p)
    })
}

/// Pipeline `hash` was built (the title used it): `xcl pipelines` makes these first.
pub fn used(hash: u64) {
    if hash == 0 {
        return;
    }
    with(|j| put(j, b'U', 0, hash, &[]).map(|_| 0));
}

/// A journal's pipeline, read back with everything it points to owned.
pub enum Replay {
    Graphics(Box<D3D12_GRAPHICS_PIPELINE_STATE_DESC>, Vec<Vec<u8>>, Vec<D3D12_INPUT_ELEMENT_DESC>, Vec<std::ffi::CString>),
    Compute(Box<D3D12_COMPUTE_PIPELINE_STATE_DESC>, Vec<Vec<u8>>),
}

unsafe impl Send for Replay {}

fn read_desc<T>(p: &[u8], at: &mut usize) -> Option<Box<T>> {
    let n = u32::from_le_bytes(p.get(*at..*at + 4)?.try_into().ok()?) as usize;
    *at += 4;
    let bytes = p.get(*at..*at + n)?;
    *at += n;
    if n != std::mem::size_of::<T>() {
        return None;
    }
    unsafe {
        let mut b: Box<std::mem::MaybeUninit<T>> = Box::new(std::mem::MaybeUninit::zeroed());
        std::ptr::copy_nonoverlapping(bytes.as_ptr(), b.as_mut_ptr() as *mut u8, n);
        Some(Box::from_raw(Box::into_raw(b) as *mut T))
    }
}

fn u32_at(p: &[u8], at: &mut usize) -> Option<u32> {
    let v = u32::from_le_bytes(p.get(*at..*at + 4)?.try_into().ok()?);
    *at += 4;
    Some(v)
}

/// The pipeline of frame `f` (`G` / `C`), its shaders from `shader` by hash. None: malformed,
/// or a shader not in the journal.
pub fn replay(f: &Frame, shader: impl Fn(u64) -> Option<Vec<u8>>) -> Option<Replay> {
    let p = f.payload;
    let mut at = 0usize;
    let sh = |at: &mut usize| -> Option<Option<Vec<u8>>> {
        let h = u64::from_le_bytes(p.get(*at..*at + 8)?.try_into().ok()?);
        *at += 8;
        if h == 0 {
            return Some(None);
        }
        Some(Some(shader(h)?))
    };
    let code = |b: &Vec<u8>| D3D12_SHADER_BYTECODE { pShaderBytecode: b.as_ptr() as *const _, BytecodeLength: b.len() };
    match f.tag {
        b'C' => {
            let cs = sh(&mut at)??;
            let mut d: Box<D3D12_COMPUTE_PIPELINE_STATE_DESC> = read_desc(p, &mut at)?;
            // (no pointer from the file reaches the driver)
            d.pRootSignature = std::mem::ManuallyDrop::new(None);
            d.CachedPSO = D3D12_CACHED_PIPELINE_STATE::default();
            d.CS = code(&cs);
            Some(Replay::Compute(d, vec![cs]))
        }
        b'G' => {
            let mut blobs = Vec::new();
            let mut slots = [None; 5];
            for s in slots.iter_mut() {
                if let Some(b) = sh(&mut at)? {
                    blobs.push(b);
                    *s = Some(blobs.len() - 1);
                }
            }
            let mut d: Box<D3D12_GRAPHICS_PIPELINE_STATE_DESC> = read_desc(p, &mut at)?;
            // (no pointer from the file reaches the driver)
            d.pRootSignature = std::mem::ManuallyDrop::new(None);
            d.StreamOutput = unsafe { std::mem::zeroed() };
            d.CachedPSO = D3D12_CACHED_PIPELINE_STATE::default();
            for b in [&mut d.VS, &mut d.PS, &mut d.DS, &mut d.HS, &mut d.GS] {
                *b = D3D12_SHADER_BYTECODE::default();
            }
            for (s, b) in slots.iter().zip([&mut d.VS, &mut d.PS, &mut d.DS, &mut d.HS, &mut d.GS]) {
                if let Some(i) = s {
                    *b = code(&blobs[*i]);
                }
            }
            let n = u32_at(p, &mut at)? as usize;
            if n > (p.len() - at) / 28 {
                return None; // each element: a name length and six words at least
            }
            let mut names = Vec::with_capacity(n);
            let mut raw = Vec::with_capacity(n);
            for _ in 0..n {
                let len = u32_at(p, &mut at)? as usize;
                names.push(std::ffi::CString::new(p.get(at..at + len)?).ok()?);
                at += len;
                let mut w = [0u32; 6];
                for x in w.iter_mut() {
                    *x = u32_at(p, &mut at)?;
                }
                raw.push(w);
            }
            let elems: Vec<D3D12_INPUT_ELEMENT_DESC> = names
                .iter()
                .zip(&raw)
                .map(|(name, w)| D3D12_INPUT_ELEMENT_DESC {
                    SemanticName: windows::core::PCSTR(name.as_ptr() as *const u8),
                    SemanticIndex: w[0],
                    Format: windows::Win32::Graphics::Dxgi::Common::DXGI_FORMAT(w[1] as i32),
                    InputSlot: w[2],
                    AlignedByteOffset: w[3],
                    InputSlotClass: D3D12_INPUT_CLASSIFICATION(w[4] as i32),
                    InstanceDataStepRate: w[5],
                })
                .collect();
            d.InputLayout = D3D12_INPUT_LAYOUT_DESC { pInputElementDescs: if elems.is_empty() { std::ptr::null() } else { elems.as_ptr() }, NumElements: elems.len() as u32 };
            Some(Replay::Graphics(d, blobs, elems, names))
        }
        _ => None,
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn graphics_round_trip() {
        let vs = vec![1u8, 2, 3, 4];
        let ps = vec![9u8; 40];
        let name = std::ffi::CString::new("TEXCOORD").unwrap();
        let elems = [D3D12_INPUT_ELEMENT_DESC { SemanticName: windows::core::PCSTR(name.as_ptr() as *const u8), SemanticIndex: 3, InputSlot: 1, AlignedByteOffset: 12, ..Default::default() }];
        let mut d = D3D12_GRAPHICS_PIPELINE_STATE_DESC::default();
        d.VS = D3D12_SHADER_BYTECODE { pShaderBytecode: vs.as_ptr() as *const _, BytecodeLength: vs.len() };
        d.PS = D3D12_SHADER_BYTECODE { pShaderBytecode: ps.as_ptr() as *const _, BytecodeLength: ps.len() };
        d.InputLayout = D3D12_INPUT_LAYOUT_DESC { pInputElementDescs: elems.as_ptr(), NumElements: 1 };
        d.NumRenderTargets = 2;
        d.SampleMask = 0xffff_ffff;
        let mut j = Journal { file: tempfile(), seen: HashSet::new() };
        let h1 = unsafe { graphics_in(&mut j, &d).unwrap() };
        let h2 = unsafe { graphics_in(&mut j, &d).unwrap() };
        assert_eq!(h1, h2, "the same desc made again: the same hash");
        // garbage in the padding after a render target's write mask (a desc built on the stack)
        let mut d3: D3D12_GRAPHICS_PIPELINE_STATE_DESC = unsafe { std::ptr::read(&d) };
        let pad = std::mem::offset_of!(D3D12_GRAPHICS_PIPELINE_STATE_DESC, BlendState.RenderTarget)
            + std::mem::offset_of!(D3D12_RENDER_TARGET_BLEND_DESC, RenderTargetWriteMask)
            + 1;
        unsafe { *(&mut d3 as *mut D3D12_GRAPHICS_PIPELINE_STATE_DESC as *mut u8).add(pad) = 0xAA };
        assert_eq!(unsafe { graphics_in(&mut j, &d3).unwrap() }, h1, "padding is not part of the pipeline");
        std::mem::forget(d3);
        let mut bytes = MAGIC.to_vec();
        use std::io::{Read, Seek};
        j.file.seek(std::io::SeekFrom::Start(0)).unwrap();
        j.file.read_to_end(&mut bytes).unwrap();
        let mut frames = Vec::new();
        for_each_frame(&bytes, |f| frames.push((f.tag, f.hash, f.payload.to_vec())));
        assert_eq!(frames.iter().filter(|f| f.0 == b'S').count(), 2);
        assert_eq!(frames.iter().filter(|f| f.0 == b'G').count(), 1, "written once");
        let g = frames.iter().find(|f| f.0 == b'G').unwrap();
        let shader = |h| frames.iter().find(|f| f.0 == b'S' && f.1 == h).map(|f| f.2.clone());
        let Some(Replay::Graphics(r, blobs, _elems, _names)) = replay(&Frame { tag: b'G', windows: 0, hash: g.1, payload: &g.2 }, shader) else { panic!("no replay") };
        assert_eq!(blobs, vec![vs.clone(), ps.clone()]);
        unsafe {
            assert_eq!(bytes_of(&r.VS).unwrap(), &vs[..]);
            assert_eq!(bytes_of(&r.PS).unwrap(), &ps[..]);
            assert!(bytes_of(&r.GS).is_none());
            assert_eq!((r.NumRenderTargets, r.SampleMask, r.InputLayout.NumElements), (2, 0xffff_ffff, 1));
            let e = &*r.InputLayout.pInputElementDescs;
            assert_eq!((e.SemanticName.to_string().unwrap().as_str(), e.SemanticIndex, e.InputSlot, e.AlignedByteOffset), ("TEXCOORD", 3, 1, 12));
        }
        // a torn tail is not a frame
        let whole = for_each_frame(&bytes, |_| {});
        assert_eq!(for_each_frame(&bytes[..bytes.len() - 3], |_| {}) < whole, true);
    }

    fn tempfile() -> std::fs::File {
        let p = std::env::temp_dir().join(format!("xcl_pso_journal_{}.bin", std::process::id()));
        let f = std::fs::OpenOptions::new().create(true).truncate(true).read(true).write(true).open(&p).unwrap();
        let _ = std::fs::remove_file(&p);
        f
    }
}
