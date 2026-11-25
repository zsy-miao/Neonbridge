//! Device slot 57 pipeline records, stage 1: decode and log only. No
//! pipeline is built from them yet. Ported from Xbox2Windows 009153b7
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

/// Block B: 0x50-byte header, count at +0x4C, then per element a u32 name
/// length (with NUL), the name, and six u32 in D3D12_INPUT_ELEMENT_DESC
/// order; zero padding to 4 bytes, filling the blob exactly. Returns
/// (semantic index, format word, offset, slot) per element; refuses words
/// no measurement pins.
pub(super) fn input_layout(b: &[u8]) -> Result<Vec<(u32, u32, u32, u32)>, String> {
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
        out.push((index, format, offset, slot));
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

/// TUXBOX_SLOT57_REAL: stage 2 switch, off and unused. Stage 2 (real
/// pipelines) needs xshader to accept raw console programs first.
#[allow(dead_code)]
pub(super) fn real() -> bool {
    crate::env_toggle!("TUXBOX_SLOT57_REAL") == Some("1")
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
