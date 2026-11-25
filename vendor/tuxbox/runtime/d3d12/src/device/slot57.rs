//! Device slot 57 pipeline records. Stage 1 decodes and logs them; stage 2
//! (TUXBOX_SLOT57_REAL (default on, =0 off)) wraps their programs as synthetic DXBC and builds
//! a real graphics pipeline lazily (Xbox2Windows d3d12_draw.inl
//! translation_for / pipeline_for, MIT). Ported from Xbox2Windows 009153b7
//! pipeline_state_decode.h / depth_state_apply.h (MIT); palmetto
//! docs/graphics_record.md, convert_slot57_record.
//!
//! The record is 8-byte words: +0x00 root signature, +0x08/+0x10 state
//! block A, +0x18/+0x20 block B (input layout), +0x28/+0x30 PS program,
//! +0x48/+0x50 VS program (palmetto naming; X2W calls them a/b).
use std::ffi::c_void;
use windows::Win32::Foundation::{FALSE, TRUE};
use windows::Win32::Graphics::Direct3D12::*;

/// Largest state block accepted (block A and B).
const BLOCK_MAX: usize = 0x1000;
/// Largest shader program copied.
const PROGRAM_MAX: usize = 1 << 20;

/// A slot 57 record with its blobs copied at create time.
pub(super) struct Record {
    pub root: usize,
    pub a: Vec<u8>,
    pub b: Vec<u8>,
    pub ps: Vec<u8>,
    pub vs: Vec<u8>,
    /// The seven code slots as read (address:size, ! when unreadable).
    pub slots: String,
}

impl Record {
    pub(super) fn hash(&self) -> u64 {
        use std::hash::{Hash, Hasher};
        let mut h = std::collections::hash_map::DefaultHasher::new();
        (&self.a, &self.b, &self.ps, &self.vs).hash(&mut h);
        h.finish()
    }
}

fn blob(address: usize, size: usize, max: usize) -> Vec<u8> {
    if address == 0 || size == 0 || size > max || !super::compute::readable(address, size) {
        return Vec::new();
    }
    // SAFETY: readable confirmed the range is committed and readable.
    unsafe { std::slice::from_raw_parts(address as *const u8, size) }.to_vec()
}

/// Copies the record at desc; None when its words are unreadable.
pub(super) fn decode(desc: *const c_void) -> Option<Record> {
    let address = desc as usize;
    if !super::compute::readable(address, 0x58) {
        return None;
    }
    // SAFETY: 0x58 bytes are readable.
    let word = |n: usize| unsafe { (address as *const u64).add(n).read_unaligned() } as usize;
    Some(Record {
        root: word(0),
        a: blob(word(1), word(2), BLOCK_MAX),
        b: blob(word(3), word(4), BLOCK_MAX),
        ps: blob(word(5), word(6), PROGRAM_MAX),
        vs: blob(word(9), word(10), PROGRAM_MAX),
        slots: if super::compute::readable(address, 0x28 + 7 * 32) {
            (0..7).map(|n| {
                let (code, size) = (word(5 + 4 * n), word(6 + 4 * n));
                let ok = code != 0 && size != 0 && size <= PROGRAM_MAX && super::compute::readable(code, size);
                format!("{n}={code:#x}:{size:#x}{}", if ok || code == 0 { "" } else { "!" })
            }).collect::<Vec<_>>().join(" ")
        } else { "unreadable".into() },
    })
}

fn bits(v: u32, lo: u32, hi: u32) -> u32 {
    (v >> lo) & ((1u32 << (hi - lo + 1)) - 1)
}

fn u32_at(b: &[u8], off: usize) -> Option<u32> {
    b.get(off..off + 4).map(|s| u32::from_le_bytes(s.try_into().unwrap()))
}

/// CompareFrag NEVER..ALWAYS has D3D12_COMPARISON_FUNC's order (1..8).
fn compare(frag: u32) -> D3D12_COMPARISON_FUNC {
    D3D12_COMPARISON_FUNC((frag & 7) as i32 + 1)
}

/// StencilOp to D3D12; None without an equivalent. opval: STENCILOPVAL.
fn stencil_op(op: u32, opval: u32) -> Option<D3D12_STENCIL_OP> {
    Some(match op {
        0 => D3D12_STENCIL_OP_KEEP,
        1 => D3D12_STENCIL_OP_ZERO,
        3 => D3D12_STENCIL_OP_REPLACE,
        5 if opval == 1 => D3D12_STENCIL_OP_INCR_SAT,
        6 if opval == 1 => D3D12_STENCIL_OP_DECR_SAT,
        7 => D3D12_STENCIL_OP_INVERT,
        8 if opval == 1 => D3D12_STENCIL_OP_INCR,
        9 if opval == 1 => D3D12_STENCIL_OP_DECR,
        _ => return None,
    })
}

/// State block A's DB_DEPTH_CONTROL (+0xB8), DB_STENCIL_CONTROL (+0xBC),
/// DB_STENCILREFMASK (+0xC0) and its back-face copy (+0xC4), Mesa gfx10
/// layouts as X2W decodes them. The reference is Some when stencil is on.
pub(super) fn depth_stencil(a: &[u8]) -> Result<(D3D12_DEPTH_STENCIL_DESC, Option<u8>), &'static str> {
    let (Some(depth), Some(stencil), Some(refmask), Some(refmask_bf)) =
        (u32_at(a, 0xb8), u32_at(a, 0xbc), u32_at(a, 0xc0), u32_at(a, 0xc4))
    else {
        return Err("state block A too short for the depth-stencil words");
    };
    if bits(depth, 30, 31) != 0 {
        return Err("colour writes tied to the depth result (DB_DEPTH_CONTROL bits 30-31)");
    }
    let mut d = D3D12_DEPTH_STENCIL_DESC::default();
    let z = bits(depth, 1, 1) != 0;
    d.DepthEnable = if z { TRUE } else { FALSE };
    d.DepthWriteMask = if z && bits(depth, 2, 2) != 0 { D3D12_DEPTH_WRITE_MASK_ALL } else { D3D12_DEPTH_WRITE_MASK_ZERO };
    d.DepthFunc = compare(bits(depth, 4, 6));
    let stencil_on = bits(depth, 0, 0) != 0;
    d.StencilEnable = if stencil_on { TRUE } else { FALSE };
    let back_own = bits(depth, 7, 7) != 0;
    d.StencilReadMask = bits(refmask, 8, 15) as u8;
    d.StencilWriteMask = bits(refmask, 16, 23) as u8;
    let opval = bits(refmask, 24, 31);
    let opval_bf = if back_own { bits(refmask_bf, 24, 31) } else { opval };
    let face = |lo: u32, func: u32, val: u32| -> Option<D3D12_DEPTH_STENCILOP_DESC> {
        Some(D3D12_DEPTH_STENCILOP_DESC {
            StencilFailOp: stencil_op(bits(stencil, lo, lo + 3), val)?,
            StencilDepthFailOp: stencil_op(bits(stencil, lo + 8, lo + 11), val)?,
            StencilPassOp: stencil_op(bits(stencil, lo + 4, lo + 7), val)?,
            StencilFunc: compare(func),
        })
    };
    let front = face(0, bits(depth, 8, 10), opval);
    let back = if back_own { face(12, bits(depth, 20, 22), opval_bf) } else { front };
    let keep = D3D12_DEPTH_STENCILOP_DESC {
        StencilFailOp: D3D12_STENCIL_OP_KEEP,
        StencilDepthFailOp: D3D12_STENCIL_OP_KEEP,
        StencilPassOp: D3D12_STENCIL_OP_KEEP,
        StencilFunc: D3D12_COMPARISON_FUNC_ALWAYS,
    };
    let mut stencil_ref = None;
    if stencil_on {
        let (Some(front), Some(back)) = (front, back) else {
            return Err("stencil op without a D3D12 form (ONES, REPLACE_OP, logic op, or add / subtract by other than one)");
        };
        d.FrontFace = front;
        d.BackFace = back;
        stencil_ref = Some(bits(refmask, 0, 7) as u8);
    } else {
        // Canonical when off: one PSO per real state.
        d.StencilReadMask = 0xff;
        d.StencilWriteMask = 0xff;
        d.FrontFace = keep;
        d.BackFace = keep;
    }
    if !z {
        d.DepthFunc = D3D12_COMPARISON_FUNC_ALWAYS;
    }
    Ok((d, stencil_ref))
}

/// DXGI format words measured in block B, with their byte sizes.
fn format_size(format: u32) -> Option<u32> {
    Some(match format {
        2 => 16,
        6 => 12,
        16 => 8,
        28 | 30 => 4,
        _ => return None,
    })
}

/// One block B element: semantic name, index, format word, offset, slot.
pub(super) struct Element {
    pub name: std::ffi::CString,
    pub index: u32,
    pub format: u32,
    pub offset: u32,
    pub slot: u32,
}

/// Block B: 0x50-byte header, count at +0x4C, then per element a u32 name
/// length (with NUL), the name, and six u32 in D3D12_INPUT_ELEMENT_DESC
/// order; zero padding to 4 bytes, filling the blob exactly. Returns
/// (semantic index, format word, offset, slot) per element; refuses words
/// no measurement pins.
pub(super) fn input_layout(b: &[u8]) -> Result<Vec<(u32, u32, u32, u32)>, String> {
    Ok(elements(b)?.into_iter().map(|e| (e.index, e.format, e.offset, e.slot)).collect())
}

/// input_layout with the semantic names.
pub(super) fn elements(b: &[u8]) -> Result<Vec<Element>, String> {
    let count = u32_at(b, 0x4c).ok_or("input layout shorter than its 0x50-byte header")? as usize;
    let mut at = 0x50usize;
    let mut out = Vec::new();
    let mut spans: Vec<(u32, u32)> = Vec::new();
    for i in 0..count {
        let len = u32_at(b, at).ok_or_else(|| format!("elements[{i}].name_len past the blob"))? as usize;
        at += 4;
        let name = b.get(at..at.saturating_add(len)).filter(|_| len >= 2)
            .ok_or_else(|| format!("elements[{i}].name[{len}] past the blob"))?;
        if name[len - 1] != 0 || !name[..len - 1].iter().all(|c| c.is_ascii_graphic()) {
            return Err(format!("elements[{i}].name not printable"));
        }
        let cname = std::ffi::CString::new(&name[..len - 1]).map_err(|_| format!("elements[{i}].name not printable"))?;
        at += len;
        let w: Vec<u32> = (0..6).map(|n| u32_at(b, at + n * 4)).collect::<Option<_>>()
            .ok_or_else(|| format!("elements[{i}].attributes past the blob"))?;
        at += 24;
        let (index, format, slot, offset, class, step) = (w[0], w[1], w[2], w[3], w[4], w[5]);
        let size = format_size(format).ok_or_else(|| format!("elements[{i}].format[{format}]"))?;
        if slot != 0 {
            return Err(format!("elements[{i}].slot[{slot}]"));
        }
        if class != 0 {
            return Err(format!("elements[{i}].class[{class}]"));
        }
        if step != 0 {
            return Err(format!("elements[{i}].step_rate[{step}]"));
        }
        let end = offset.checked_add(size).ok_or_else(|| format!("elements[{i}].offset[{offset}]"))?;
        if spans.iter().any(|&(s, e)| offset < e && s < end) {
            return Err(format!("elements[{i}].offset[{offset}] overlaps"));
        }
        spans.push((offset, end));
        out.push(Element { name: cname, index, format, offset, slot });
    }
    let padded = at.next_multiple_of(4);
    if padded != b.len() || b[at..].iter().any(|&c| c != 0) {
        return Err(format!("input layout fills {padded:#x} of {:#x} bytes", b.len()));
    }
    Ok(out)
}

/// TUXBOX_SLOT57_DSS (on by default, =0 off): log each distinct record.
fn dss_logging() -> bool {
    crate::env_toggle!("TUXBOX_SLOT57_DSS") != Some("0")
}

/// TUXBOX_SLOT57_REAL (default on, =0 off): stage 2, real pipelines (off by default). It
/// implies the placeholder path (TUXBOX_SLOT57=placeholder): the title gets
/// the placeholder with a lazy upgrade built at first bind.
pub(super) fn real() -> bool {
    crate::env_toggle!("TUXBOX_SLOT57_REAL") != Some("0")
}

fn head(p: &[u8]) -> String {
    if p.is_empty() {
        return "unreadable".into();
    }
    let hex: String = p.iter().take(16).map(|b| format!("{b:02x}")).collect();
    format!("{:#x} bytes [{hex}]", p.len())
}

/// Logs a record's depth-stencil, input layout and program heads once per
/// distinct record hash, 32 lines at most.
pub(super) fn log_record(slot: u32, desc: *const c_void) {
    use std::sync::Mutex;
    use std::sync::atomic::{AtomicU32, Ordering};
    static SEEN: Mutex<Vec<u64>> = Mutex::new(Vec::new());
    static LINES: AtomicU32 = AtomicU32::new(0);
    if !dss_logging() || LINES.load(Ordering::Relaxed) >= 32 {
        return;
    }
    let Some(record) = decode(desc) else { return };
    let hash = record.hash();
    {
        let mut seen = SEEN.lock().unwrap_or_else(|e| e.into_inner());
        if seen.contains(&hash) {
            return;
        }
        seen.push(hash);
    }
    if LINES.fetch_add(1, Ordering::Relaxed) >= 32 {
        return;
    }
    let dss = match depth_stencil(&record.a) {
        Ok((d, r)) => format!(
            "depth {} write {} func {} stencil {} read {:#x} write {:#x} front {}/{}/{}/{} back {}/{}/{}/{} ref {r:?}",
            d.DepthEnable.as_bool(), d.DepthWriteMask.0, d.DepthFunc.0, d.StencilEnable.as_bool(),
            d.StencilReadMask, d.StencilWriteMask,
            d.FrontFace.StencilFailOp.0, d.FrontFace.StencilDepthFailOp.0, d.FrontFace.StencilPassOp.0, d.FrontFace.StencilFunc.0,
            d.BackFace.StencilFailOp.0, d.BackFace.StencilDepthFailOp.0, d.BackFace.StencilPassOp.0, d.BackFace.StencilFunc.0),
        Err(why) => format!("refused: {why}"),
    };
    let layout = if record.b.is_empty() {
        "unreadable".into()
    } else {
        match input_layout(&record.b) {
            Ok(e) => format!("{e:?}"),
            Err(why) => format!("refused: {why}"),
        }
    };
    crate::diagnostics::log_line(format_args!(
        "slot {slot} record {hash:016x} root {:#x}: dss {dss}; layout (index, format, offset, slot) {layout}; ps {}; vs {}",
        record.root, head(&record.ps), head(&record.vs)));
}

/// s_inst_prefetch 3, the program's first instruction after its header.
const PREFETCH: [u8; 4] = [0x03, 0x00, 0xa0, 0xbf];
const ABSENT: u64 = 0xdead_beef_dead_beef;
const CODE_END: u32 = 0xbf9f_0000;

/// Xbox2Windows gfx2hlsl.py strip_header (MIT): the program starts at the
/// first prefetch within 0x80 bytes, or at an s_swappc_b64 just before it;
/// a misaligned or absent prefetch keeps the whole blob.
fn strip_header(code: &[u8]) -> &[u8] {
    let scan = &code[..code.len().min(0x80)];
    let Some(mut at) = scan.windows(4).position(|w| w == PREFETCH) else { return code };
    if at >= 4 && at % 4 == 0 && u32_at(code, at - 4).is_some_and(|w| w & 0xff80_ff00 == 0xbe80_2100) {
        at -= 4;
    }
    if at % 4 == 0 { &code[at..] } else { code }
}

fn put(v: &mut [u8], at: usize, bytes: &[u8]) {
    v[at..at + bytes.len()].copy_from_slice(bytes);
}

/// A raw console program as the synthetic DXBC xshader parses
/// (tuxbox_dxbc::Dxbc::parse, shader-compiler native_dxil, fssm::decode):
/// one DXIL part whose XDAT holds a kind 13 XBCH v3 (zlib) FSSM, layout 5,
/// one stage slot, and a "NOBC" bitcode range. stage: 0 PS, 1 VS. No
/// signature, PSV0 or RTS0 parts (side-session note slot57-synthetic-dxbc).
pub(super) fn wrap(program: &[u8], stage: u32) -> Vec<u8> {
    let code = strip_header(program);
    let isa = &code[..code.len() & !3];
    // Image: layout u64, 12 slots, descriptor (entry count at +0x58),
    // 16-byte preamble, then the code padded with s_code_end.
    const DESCRIPTOR: usize = 0x68;
    const CODE: usize = DESCRIPTOR + 0x68 + 16;
    let capacity = (isa.len() + 4).next_multiple_of(256);
    let mut image = vec![0u8; CODE + capacity];
    put(&mut image, 0, &5u64.to_le_bytes());
    for slot in 0..12 {
        let value = if slot == stage as usize { DESCRIPTOR as u64 } else { ABSENT };
        put(&mut image, 8 + slot * 8, &value.to_le_bytes());
    }
    put(&mut image, DESCRIPTOR, &(CODE as u64).to_le_bytes());
    put(&mut image, DESCRIPTOR + 8, &(capacity as u64).to_le_bytes());
    for (n, word) in [0u32, 64, 1, 1].iter().enumerate() {
        put(&mut image, CODE - 16 + n * 4, &word.to_le_bytes());
    }
    put(&mut image, CODE, isa);
    for at in (CODE + isa.len()..image.len()).step_by(4) {
        put(&mut image, at, &CODE_END.to_le_bytes());
    }
    container(&image, stage)
}

/// TUXBOX_SLOT67_REAL (default on, =0 off): a slot 67 compute program (kind 6, bare RDNA2 code)
/// as the synthetic DXBC xshader parses: FSSM image layout 6 with directory
/// slot 11 occupied (dxbc fssm.rs compute_lds_bytes), the LDS bytes at
/// descriptor +0x4c, no entry (+0x60 count 0), and the preamble's group
/// size [0, X, Y, Z] that xshader's compute_group falls back to.
pub(super) fn wrap_compute(program: &[u8], threads: [u32; 3], lds: u32) -> Vec<u8> {
    let isa = &program[..program.len() & !3];
    const DESCRIPTOR: usize = 0x68;
    const CODE: usize = DESCRIPTOR + 0x70 + 16;
    let capacity = (isa.len() + 4).next_multiple_of(256);
    let mut image = vec![0u8; CODE + capacity];
    put(&mut image, 0, &6u64.to_le_bytes());
    for slot in 0..12 {
        let value = if slot == 11 { DESCRIPTOR as u64 } else { ABSENT };
        put(&mut image, 8 + slot * 8, &value.to_le_bytes());
    }
    put(&mut image, DESCRIPTOR, &(CODE as u64).to_le_bytes());
    put(&mut image, DESCRIPTOR + 8, &(capacity as u64).to_le_bytes());
    put(&mut image, DESCRIPTOR + 0x4c, &lds.to_le_bytes());
    for (n, word) in [0, threads[0], threads[1], threads[2]].iter().enumerate() {
        put(&mut image, CODE - 16 + n * 4, &word.to_le_bytes());
    }
    put(&mut image, CODE, isa);
    for at in (CODE + isa.len()..image.len()).step_by(4) {
        put(&mut image, at, &CODE_END.to_le_bytes());
    }
    container(&image, 5)
}

/// FSSM image -> XBCH v3 -> XDAT kind 13 -> DXIL part -> DXBC (wrap).
fn container(image: &[u8], stage: u32) -> Vec<u8> {
    let mut fssm = Vec::with_capacity(16 + image.len());
    fssm.extend_from_slice(b"FSSM");
    fssm.extend_from_slice(&0x101u32.to_le_bytes());
    fssm.extend_from_slice(&(image.len() as u64).to_le_bytes());
    fssm.extend_from_slice(image);
    let packed = miniz_oxide::deflate::compress_to_vec_zlib(&fssm, 6);
    let mut xbch = Vec::with_capacity(24 + packed.len());
    xbch.extend_from_slice(b"XBCH");
    xbch.extend_from_slice(&3u32.to_le_bytes());
    xbch.extend_from_slice(&(packed.len() as u64).to_le_bytes());
    xbch.extend_from_slice(&(fssm.len() as u64).to_le_bytes());
    xbch.extend_from_slice(&packed);
    // DXIL body: 24-byte program header, XDAT (offsets include the header),
    // its section, then "NOBC".
    let mut body = vec![0u8; 24];
    body.extend_from_slice(b"XDAT");
    body.extend_from_slice(&1u32.to_le_bytes());
    body.extend_from_slice(&(24u32 + 12).to_le_bytes());
    body.extend_from_slice(&13u32.to_le_bytes());
    body.extend_from_slice(&(24u32 + 24).to_le_bytes());
    body.extend_from_slice(&(xbch.len() as u32).to_le_bytes());
    body.extend_from_slice(&xbch);
    body.resize(body.len().next_multiple_of(4), 0);
    let nobc = body.len();
    body.extend_from_slice(b"NOBC");
    let dwords = (body.len() / 4) as u32;
    put(&mut body, 0, &(stage << 16 | 0x60).to_le_bytes());
    put(&mut body, 4, &dwords.to_le_bytes());
    put(&mut body, 8, b"DXIL");
    put(&mut body, 12, &0x106u32.to_le_bytes());
    put(&mut body, 16, &(nobc as u32 - 8).to_le_bytes());
    put(&mut body, 20, &4u32.to_le_bytes());
    let total = 44 + body.len();
    let mut dxbc = Vec::with_capacity(total);
    dxbc.extend_from_slice(b"DXBC");
    dxbc.extend_from_slice(&[0; 16]);
    dxbc.extend_from_slice(&1u32.to_le_bytes());
    dxbc.extend_from_slice(&(total as u32).to_le_bytes());
    dxbc.extend_from_slice(&1u32.to_le_bytes());
    dxbc.extend_from_slice(&36u32.to_le_bytes());
    dxbc.extend_from_slice(b"DXIL");
    dxbc.extend_from_slice(&(body.len() as u32).to_le_bytes());
    dxbc.extend_from_slice(&body);
    dxbc
}

/// Appends one stream subobject of `tag` (stream.rs layout).
fn push<T>(stream: &mut Vec<u8>, tag: i32, value: &T) {
    let (offset, bytes) = super::stream::layout(tag).expect("known subobject");
    let start = stream.len();
    stream.resize(start + bytes, 0);
    put(stream, start, &tag.to_le_bytes());
    // SAFETY: T is a plain D3D12 struct of size_of::<T>() bytes.
    let raw = unsafe { std::slice::from_raw_parts((value as *const T).cast::<u8>(), size_of::<T>()) };
    put(stream, start + offset, raw);
}

fn opaque() -> D3D12_BLEND_DESC {
    let mut blend = D3D12_BLEND_DESC::default();
    for target in &mut blend.RenderTarget {
        *target = D3D12_RENDER_TARGET_BLEND_DESC {
            SrcBlend: D3D12_BLEND_ONE,
            DestBlend: D3D12_BLEND_ZERO,
            BlendOp: D3D12_BLEND_OP_ADD,
            SrcBlendAlpha: D3D12_BLEND_ONE,
            DestBlendAlpha: D3D12_BLEND_ZERO,
            BlendOpAlpha: D3D12_BLEND_OP_ADD,
            LogicOp: D3D12_LOGIC_OP_NOOP,
            RenderTargetWriteMask: D3D12_COLOR_WRITE_ENABLE_ALL.0 as u8,
            ..Default::default()
        };
    }
    blend
}

/// Render target format of the create-time variant (palmetto's default).
const RT_FORMAT: windows::Win32::Graphics::Dxgi::Common::DXGI_FORMAT =
    windows::Win32::Graphics::Dxgi::Common::DXGI_FORMAT_R8G8B8A8_UNORM;

/// A command list's bound render-target and depth formats (host formats,
/// OMSetRenderTargets), which pick a record's pipeline variant (Xbox2Windows
/// d3d12_draw.inl pipeline_for / pass_dsv_format, MIT).
#[derive(Clone, Copy, PartialEq, Eq, Hash, Default, Debug)]
pub(crate) struct Targets {
    pub count: u32,
    pub rtv: [i32; 8],
    /// 0 (DXGI_FORMAT_UNKNOWN) when no DSV is bound.
    pub dsv: i32,
    /// A bound handle whose view format was never recorded.
    pub unknown: bool,
}

impl Targets {
    /// The create-time variant: one RGBA8 target, no depth.
    fn legacy() -> Self {
        let mut t = Targets { count: 1, ..Default::default() };
        t.rtv[0] = RT_FORMAT.0;
        t
    }
}

/// Host view format of each RTV/DSV handle, written at view creation
/// (TUXBOX_SLOT57_REAL (default on, =0 off) only).
static VIEW_FORMATS: std::sync::Mutex<Option<std::collections::HashMap<usize, i32>>> = std::sync::Mutex::new(None);

pub(super) fn note_view(handle: usize, format: Option<windows::Win32::Graphics::Dxgi::Common::DXGI_FORMAT>) {
    if !real() {
        return;
    }
    let mut map = VIEW_FORMATS.lock().unwrap_or_else(|e| e.into_inner());
    let map = map.get_or_insert_with(Default::default);
    match format {
        Some(f) => { map.insert(handle, f.0); }
        None => { map.remove(&handle); }
    }
}

/// The formats an OMSetRenderTargets binds; None when the flag is off.
pub(crate) unsafe fn targets(
    device: *mut c_void, count: u32, rtvs: *const D3D12_CPU_DESCRIPTOR_HANDLE, contiguous: bool,
    dsv: *const D3D12_CPU_DESCRIPTOR_HANDLE,
) -> Option<Targets> {
    if !real() {
        return None;
    }
    let device = unsafe { &*device.cast::<super::Device>() };
    let stride = if contiguous && count > 1 {
        (unsafe { device.native.GetDescriptorHandleIncrementSize(D3D12_DESCRIPTOR_HEAP_TYPE_RTV) }) as usize
    } else { 0 };
    let map = VIEW_FORMATS.lock().unwrap_or_else(|e| e.into_inner());
    let mut unknown = false;
    let mut look = |handle: usize| match map.as_ref().and_then(|m| m.get(&handle)) {
        Some(&f) => f,
        None => { unknown = true; 0 }
    };
    let count = count.min(8);
    let mut rtv = [0i32; 8];
    for (i, slot) in rtv.iter_mut().enumerate().take(count as usize) {
        // SAFETY: the caller checked rtvs holds count handles (or one when contiguous).
        let handle = if contiguous { unsafe { (*rtvs).ptr }.wrapping_add(i * stride) } else { unsafe { (*rtvs.add(i)).ptr } };
        *slot = look(handle);
    }
    let dsv = unsafe { dsv.as_ref() }.map_or(0, |h| look(h.ptr));
    Some(Targets { count, rtv, dsv, unknown })
}

/// A slot 57 record a pipeline was made from, with its key computed once.
pub(crate) struct Bound {
    record: Record,
    key: u64,
}

/// The variant of `bound` for `targets`: its pipeline once made and the
/// stencil reference to set around its draws (Xbox2Windows d3d12_draw.inl
/// pipeline_for and depth_state_apply.h, MIT). None while it is still being
/// made or was refused: the draw is dropped, logged once per variant.
pub(crate) unsafe fn variant(device: *mut c_void, bound: &Bound, targets: Targets) -> Option<(ID3D12PipelineState, Option<u8>)> {
    use std::sync::{Arc, Mutex};
    type Entry = (Arc<super::stream::Upgrade>, Option<u8>);
    static VARIANTS: Mutex<Option<std::collections::HashMap<(u64, Targets), Entry>>> = Mutex::new(None);
    let entry = {
        let mut map = VARIANTS.lock().unwrap_or_else(|e| e.into_inner());
        let map = map.get_or_insert_with(Default::default);
        match map.get(&(bound.key, targets)) {
            Some(e) => e.clone(),
            None => {
                let device = unsafe { &*device.cast::<super::Device>() };
                let made = if targets.unknown {
                    Err("a bound RTV or DSV has no recorded view format".to_string())
                } else {
                    unsafe { build(device, &bound.record, targets) }
                };
                let e = match made {
                    Ok(e) => e,
                    Err(why) => {
                        crate::diagnostics::log_line(format_args!(
                            "slot 57 record {:016x} variant {targets:?}: refused: {why}; its draws are dropped (TUXBOX_SLOT57_REAL (default on, =0 off))", bound.key));
                        (super::stream::background::never(), None)
                    }
                };
                map.insert((bound.key, targets), e.clone());
                e
            }
        }
    };
    match super::stream::background::ready(&entry.0) {
        Some(built) => Some((built.pipeline().native.clone(), entry.1)),
        None => {
            static SEEN: Mutex<Vec<(u64, Targets)>> = Mutex::new(Vec::new());
            let mut seen = SEEN.lock().unwrap_or_else(|e| e.into_inner());
            if seen.len() < 64 && !seen.contains(&(bound.key, targets)) {
                seen.push((bound.key, targets));
                crate::diagnostics::log_line(format_args!(
                    "slot 57 record {:016x} variant {targets:?}: not made yet, draw dropped (logged once; TUXBOX_SLOT57_REAL (default on, =0 off))", bound.key));
            }
            None
        }
    }
}

/// The upgrade for one record: a lazy stream pipeline (Xbox2Windows
/// d3d12_draw.inl translation_for / pipeline_for, MIT; palmetto
/// docs/graphics_record.md) that the stream workers build at first bind
/// through create_pipeline, so the wrapped VS/PS go through raster_with_root
/// with the record's root (+0x00) via the root proxy. Records with the same
/// VS, PS, block A, B, root and formats share one upgrade.
unsafe fn build(device: &super::Device, record: &Record, targets: Targets) -> Result<(std::sync::Arc<super::stream::Upgrade>, Option<u8>), String> {
    use std::sync::{Arc, Mutex};
    use super::stream::{Upgrade, background};
    use windows::core::Interface;
    static CACHE: Mutex<Vec<(u64, Arc<Upgrade>, Option<u8>)>> = Mutex::new(Vec::new());
    if record.vs.is_empty() || record.ps.is_empty() {
        // test157: 32 records were refused here. Name each of the seven code
        // slots (five-star PipelineLayout: {code, size, metadata, size} at
        // +0x28 + 32n) so the empty or unreadable one shows.
        return Err(format!("VS or PS program unreadable (slots {})", record.slots));
    }
    let identity = super::identity(device);
    let root = unsafe { super::compute::root_proxy(record.root as *mut c_void, identity.as_raw()) }
        .map_err(|_| format!("+0x00 {:#x} is not a root signature of this device", record.root))?;
    if root.is_none() {
        return Err("no root signature at +0x00".into());
    }
    let key = {
        use std::hash::{Hash, Hasher};
        let mut h = std::collections::hash_map::DefaultHasher::new();
        (&record.vs, &record.ps, &record.a, &record.b, record.root, targets).hash(&mut h);
        h.finish()
    };
    if let Some(slot) = CACHE.lock().unwrap_or_else(|e| e.into_inner()).iter().find(|(k, ..)| *k == key) {
        return Ok((slot.1.clone(), slot.2));
    }
    let layout = if record.b.is_empty() { Vec::new() } else { elements(&record.b)? };
    let descs: Vec<D3D12_INPUT_ELEMENT_DESC> = layout.iter().map(|e| D3D12_INPUT_ELEMENT_DESC {
        SemanticName: windows::core::PCSTR(e.name.as_ptr().cast()),
        SemanticIndex: e.index,
        Format: windows::Win32::Graphics::Dxgi::Common::DXGI_FORMAT(e.format as i32),
        InputSlot: e.slot,
        AlignedByteOffset: e.offset,
        InputSlotClass: D3D12_INPUT_CLASSIFICATION_PER_VERTEX_DATA,
        InstanceDataStepRate: 0,
    }).collect();
    let vs = wrap(&record.vs, 1);
    let ps = wrap(&record.ps, 0);
    let blend = if super::cb_blend::enabled() {
        super::cb_blend::state_block_blend(&record.a).unwrap_or_else(|_| opaque())
    } else {
        opaque()
    };
    // Depth and stencil only with a DSV bound (Xbox2Windows d3d12_draw.inl
    // pass_dsv_format, MIT); off otherwise, as the create-time variant has.
    let keep = D3D12_DEPTH_STENCILOP_DESC {
        StencilFailOp: D3D12_STENCIL_OP_KEEP,
        StencilDepthFailOp: D3D12_STENCIL_OP_KEEP,
        StencilPassOp: D3D12_STENCIL_OP_KEEP,
        StencilFunc: D3D12_COMPARISON_FUNC_ALWAYS,
    };
    let depth = D3D12_DEPTH_STENCIL_DESC {
        DepthEnable: FALSE,
        DepthWriteMask: D3D12_DEPTH_WRITE_MASK_ZERO,
        DepthFunc: D3D12_COMPARISON_FUNC_ALWAYS,
        StencilEnable: FALSE,
        StencilReadMask: 0xff,
        StencilWriteMask: 0xff,
        FrontFace: keep,
        BackFace: keep,
    };
    let (depth, reference) = if targets.dsv != 0 { depth_stencil(&record.a)? } else { (depth, None) };
    let raster = D3D12_RASTERIZER_DESC {
        FillMode: D3D12_FILL_MODE_SOLID,
        CullMode: D3D12_CULL_MODE_NONE,
        DepthClipEnable: TRUE,
        ..Default::default()
    };
    let mut formats = D3D12_RT_FORMAT_ARRAY { NumRenderTargets: targets.count, ..Default::default() };
    for (to, &from) in formats.RTFormats.iter_mut().zip(&targets.rtv).take(targets.count as usize) {
        *to = windows::Win32::Graphics::Dxgi::Common::DXGI_FORMAT(from);
    }
    let mut stream = Vec::new();
    push(&mut stream, 0, &(record.root as *mut c_void));
    push(&mut stream, 1, &D3D12_SHADER_BYTECODE { pShaderBytecode: vs.as_ptr().cast(), BytecodeLength: vs.len() });
    push(&mut stream, 2, &D3D12_SHADER_BYTECODE { pShaderBytecode: ps.as_ptr().cast(), BytecodeLength: ps.len() });
    push(&mut stream, 8, &blend);
    push(&mut stream, 9, &u32::MAX);
    push(&mut stream, 10, &raster);
    push(&mut stream, 11, &depth);
    push(&mut stream, 12, &D3D12_INPUT_LAYOUT_DESC { pInputElementDescs: descs.as_ptr(), NumElements: descs.len() as u32 });
    push(&mut stream, 14, &D3D12_PRIMITIVE_TOPOLOGY_TYPE_TRIANGLE.0);
    push(&mut stream, 15, &formats);
    if targets.dsv != 0 {
        push(&mut stream, 16, &windows::Win32::Graphics::Dxgi::Common::DXGI_FORMAT(targets.dsv));
    }
    push(&mut stream, 17, &windows::Win32::Graphics::Dxgi::Common::DXGI_SAMPLE_DESC { Count: 1, Quality: 0 });
    let mut words = vec![0usize; stream.len().div_ceil(size_of::<usize>())];
    // SAFETY: words holds at least stream.len() bytes.
    unsafe { std::ptr::copy_nonoverlapping(stream.as_ptr(), words.as_mut_ptr().cast::<u8>(), stream.len()) };
    let desc = D3D12_PIPELINE_STATE_STREAM_DESC {
        SizeInBytes: words.len() * size_of::<usize>(),
        pPipelineStateSubobjectStream: words.as_mut_ptr().cast(),
    };
    // SAFETY: every pointer in the stream (programs, elements, names, the
    // checked root) is live for this call; the copy owns its own.
    let owned = unsafe { background::OwnedStream::copy(&desc) }.ok_or("stream copy refused")?;
    let slot = background::lazy((*identity).clone(), background::Work::Stream(owned));
    CACHE.lock().unwrap_or_else(|e| e.into_inner()).push((key, slot.clone(), reference));
    Ok((slot, reference))
}

/// Stage 2: the upgrade the placeholder carries. On any failure the
/// placeholder keeps a never-built upgrade (draws dropped, as before) and
/// the reason is logged once per record, 32 lines at most.
/// Also returns the record, for the per-list variants of its draws.
pub(super) unsafe fn upgrade(device: &super::Device, desc: *const c_void) -> (std::sync::Arc<super::stream::Upgrade>, Option<std::sync::Arc<Bound>>) {
    use std::sync::Mutex;
    let result = match decode(desc) {
        Some(record) => match unsafe { build(device, &record, Targets::legacy()) } {
            Ok((slot, _)) => Ok((slot, record)),
            Err(why) => Err((record.hash(), why)),
        },
        None => Err((0, "record unreadable".to_string())),
    };
    match result {
        Ok((slot, record)) => {
            let key = record.hash();
            (slot, Some(std::sync::Arc::new(Bound { record, key })))
        }
        Err((hash, why)) => {
            static SEEN: Mutex<Vec<u64>> = Mutex::new(Vec::new());
            let mut seen = SEEN.lock().unwrap_or_else(|e| e.into_inner());
            if seen.len() < 32 && !seen.contains(&hash) {
                seen.push(hash);
                crate::diagnostics::log_line(format_args!(
                    "slot 57 record {hash:016x}: real pipeline refused: {why}; placeholder (TUXBOX_SLOT57_REAL (default on, =0 off))"));
            }
            (super::stream::background::never(), None)
        }
    }
}
