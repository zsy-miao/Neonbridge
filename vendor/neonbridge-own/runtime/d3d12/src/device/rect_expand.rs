//! TUXBOX_RECT_EXPAND (default on, =0 off): rectangles drawn with a pipeline
//! that does not complete them (no rectangle geometry stage: one not made for
//! the rectangle topology) are expanded on the CPU into two triangles each in
//! a buffer of the list's, not drawn as their first triangle. Copied from
//! VIctory-fxceless victory-gpu primitive.rs (verbatim below) and draw.rs
//! rectangles (GPL-3.0, credit to its authors). shortcut (ours): the vertices
//! are read when the draw is recorded, not at submission as fx does.
//!
//! Console primitive types the host lacks. A rectangle list gives three
//! corners per rectangle, the second the one between the other two, and
//! the hardware adds the fourth at `v0 + v2 - v1` (the title's
//! full-screen rectangles come as top left, bottom left, bottom right); on
//! the host each rectangle becomes two triangles, with the fourth vertex
//! made from the other three on the CPU.
//!
//! shortcut: the fourth vertex is computed from vertex inputs, not from
//! vertex shader outputs as the hardware does; that is the same for vertex
//! shaders linear in their inputs (blits, 2D quads). Inputs in formats
//! [`components`] does not know keep the first vertex's value, and so do
//! components past the end of the vertex (an element that reaches into the
//! next one, which the layout allows).
//!
//! A pipeline the title makes for rectangles completes them on the GPU
//! instead, in a geometry stage (`victory_shader::emit::rect_list_gs`):
//! from the vertex shader's outputs, whichever vertex the corner is, for
//! direct, indexed and indirect draws alike. This expansion is what is
//! left for a pipeline that could not get that stage.

/// Expands `rects` rectangles of `stride`-byte vertices from `src` into six
/// vertices each in `dst`: `(v0, v1, v2)` and `(v0, v2, v3)`, so both
/// triangles turn the same way. `elements` are (byte offset, format).
pub(crate) fn expand_rects(src: &[u8], dst: &mut [u8], stride: usize, elements: &[(u32, u32)]) {
    for (rect, out) in src
        .chunks_exact(3 * stride)
        .zip(dst.chunks_exact_mut(6 * stride))
    {
        let v = |i: usize| &rect[i * stride..(i + 1) * stride];
        for (slot, i) in [0, 1, 2, 0, 2].into_iter().enumerate() {
            out[slot * stride..(slot + 1) * stride].copy_from_slice(v(i));
        }
        let fourth = &mut out[5 * stride..];
        fourth.copy_from_slice(v(0));
        for &(offset, format) in elements {
            let at = offset as usize;
            let Some((n, kind)) = components(format) else {
                continue;
            };
            // Only the components inside the vertex: the rest belong to the
            // next one.
            let inside = stride.saturating_sub(at) / kind.bytes();
            for c in 0..n.min(inside) {
                let get = |i: usize| kind.read(&v(i)[at..], c);
                let value = get(0) + get(2) - get(1);
                kind.write(&mut fourth[at..], c, value);
            }
        }
    }
}

#[derive(Clone, Copy)]
enum Kind {
    F32,
    F16,
    Unorm8,
    Unorm16,
}

/// Component count and kind of the vertex formats rectangles use.
fn components(format: u32) -> Option<(usize, Kind)> {
    Some(match format {
        2 => (4, Kind::F32),
        6 => (3, Kind::F32),
        16 => (2, Kind::F32),
        41 => (1, Kind::F32),
        10 => (4, Kind::F16),
        34 => (2, Kind::F16),
        54 => (1, Kind::F16),
        11 => (4, Kind::Unorm16),
        35 => (2, Kind::Unorm16),
        28 | 29 | 87 | 91 => (4, Kind::Unorm8),
        _ => return None,
    })
}

impl Kind {
    fn bytes(self) -> usize {
        match self {
            Kind::F32 => 4,
            Kind::F16 | Kind::Unorm16 => 2,
            Kind::Unorm8 => 1,
        }
    }

    fn read(self, at: &[u8], c: usize) -> f32 {
        let b = &at[c * self.bytes()..];
        match self {
            Kind::F32 => f32::from_le_bytes([b[0], b[1], b[2], b[3]]),
            Kind::F16 => half_to_f32(u16::from_le_bytes([b[0], b[1]])),
            Kind::Unorm16 => f32::from(u16::from_le_bytes([b[0], b[1]])) / 65535.0,
            Kind::Unorm8 => f32::from(b[0]) / 255.0,
        }
    }

    fn write(self, at: &mut [u8], c: usize, v: f32) {
        let n = self.bytes();
        let b = &mut at[c * n..(c + 1) * n];
        match self {
            Kind::F32 => b.copy_from_slice(&v.to_le_bytes()),
            Kind::F16 => b.copy_from_slice(&f32_to_half(v).to_le_bytes()),
            Kind::Unorm16 => {
                b.copy_from_slice(&((v.clamp(0.0, 1.0) * 65535.0).round() as u16).to_le_bytes())
            }
            Kind::Unorm8 => b[0] = (v.clamp(0.0, 1.0) * 255.0).round() as u8,
        }
    }
}

fn half_to_f32(h: u16) -> f32 {
    let sign = u32::from(h >> 15) << 31;
    let exp = u32::from((h >> 10) & 0x1f);
    let man = u32::from(h & 0x3ff);
    let bits = match (exp, man) {
        (0, 0) => sign,
        // Subnormal: scale the mantissa.
        (0, m) => {
            let v = m as f32 * 2f32.powi(-24);
            return if sign != 0 { -v } else { v };
        }
        (31, m) => sign | 0x7f80_0000 | m << 13,
        (e, m) => sign | (e + 112) << 23 | m << 13,
    };
    f32::from_bits(bits)
}

fn f32_to_half(v: f32) -> u16 {
    let bits = v.to_bits();
    let sign = ((bits >> 16) & 0x8000) as u16;
    let exp = ((bits >> 23) & 0xff) as i32 - 127 + 15;
    let man = bits & 0x7f_ffff;
    if exp >= 31 {
        return sign | 0x7c00; // too large: infinity
    }
    if exp <= 0 {
        if exp < -10 {
            return sign; // below half the smallest subnormal: zero
        }
        // Half subnormals are `m * 2^-24`. An f32 in this range is still
        // normal (exponent field >= 1), so its significand has the implicit
        // leading 1; shift it into the half mantissa, rounding to even.
        let sig = (1u32 << 23) | man;
        let shift = (14 - exp) as u32;
        let halfway = 1u32 << (shift - 1);
        let q = sig >> shift;
        let r = sig & ((1u32 << shift) - 1);
        let m = q + u32::from(r > halfway || (r == halfway && (q & 1) != 0));
        return sign | m as u16;
    }
    // Rounded to nearest: a carry out of the mantissa is added into the
    // exponent (`2 - 2^-12` is 2.0); at most it reaches infinity, 0x7c00.
    let half = (exp as u32) << 10;
    sign | (half + ((man + 0x1000) >> 13)) as u16
}


pub(crate) fn enabled() -> bool {
    static ON: std::sync::OnceLock<bool> = std::sync::OnceLock::new();
    *ON.get_or_init(|| crate::env_toggle!("TUXBOX_RECT_EXPAND") != Some("0"))
}

/// A pipeline's per-vertex input elements: (slot, byte offset, format).
pub(crate) type Inputs = std::sync::Arc<[(u32, u32, u32)]>;

/// Per-vertex elements of an input layout, append-aligned offsets resolved
/// (None: an element whose size is unknown follows append-aligned ones).
pub(crate) fn inputs(layout: &windows::Win32::Graphics::Direct3D12::D3D12_INPUT_LAYOUT_DESC) -> Option<Inputs> {
    use windows::Win32::Graphics::Direct3D12::*;
    if layout.NumElements > 64 || (layout.NumElements != 0 && layout.pInputElementDescs.is_null()) {
        return None;
    }
    let mut ends = [0u32; 32];
    let mut out = Vec::new();
    for i in 0..layout.NumElements as usize {
        let e = unsafe { &*layout.pInputElementDescs.add(i) };
        let slot = e.InputSlot as usize;
        if slot >= 32 {
            return None;
        }
        let offset = if e.AlignedByteOffset == D3D12_APPEND_ALIGNED_ELEMENT { ends[slot] } else { e.AlignedByteOffset };
        let size = crate::device::views::element_stride(e.Format.0, 0);
        ends[slot] = offset + size.unwrap_or(0);
        if size.is_none() && i + 1 < layout.NumElements as usize {
            let next = unsafe { &*layout.pInputElementDescs.add(i + 1) };
            if next.InputSlot == e.InputSlot && next.AlignedByteOffset == D3D12_APPEND_ALIGNED_ELEMENT {
                return None;
            }
        }
        if e.InputSlotClass == D3D12_INPUT_CLASSIFICATION_PER_VERTEX_DATA {
            out.push((e.InputSlot, offset, e.Format.0 as u32));
        }
    }
    Some(out.into())
}

/// The vertex buffers holding the rectangles of vertices `first..first +
/// count` of the title's `views` (guest addresses) as triangles: (slot, a
/// buffer of ours, its view). None: a slot's vertices are outside its view.
pub(crate) fn rectangles(
    device: &windows::Win32::Graphics::Direct3D12::ID3D12Device,
    views: &[windows::Win32::Graphics::Direct3D12::D3D12_VERTEX_BUFFER_VIEW; 32],
    inputs: &[(u32, u32, u32)], count: u32, first: u32,
) -> Option<Vec<(u32, windows::Win32::Graphics::Direct3D12::ID3D12Resource, windows::Win32::Graphics::Direct3D12::D3D12_VERTEX_BUFFER_VIEW)>> {
    use windows::Win32::Graphics::Direct3D12::*;
    let rects = u64::from(count / 3);
    let mut out = Vec::new();
    for slot in 0..32u32 {
        let elements: Vec<(u32, u32)> = inputs.iter().filter(|e| e.0 == slot).map(|e| (e.1, e.2)).collect();
        if elements.is_empty() {
            continue;
        }
        let v = views[slot as usize];
        let stride = u64::from(v.StrideInBytes);
        if v.BufferLocation == 0 || stride == 0 || u64::from(first + count) * stride > u64::from(v.SizeInBytes) {
            static OUTSIDE: std::sync::Once = std::sync::Once::new();
            crate::diagnostics::log_once(&OUTSIDE, || format!("rectangle vertices outside their buffer (slot {slot})"));
            return None;
        }
        let from = v.BufferLocation + u64::from(first) * stride;
        let bytes = rects * 6 * stride;
        let heap = D3D12_HEAP_PROPERTIES { Type: D3D12_HEAP_TYPE_UPLOAD, ..Default::default() };
        let desc = D3D12_RESOURCE_DESC {
            Dimension: D3D12_RESOURCE_DIMENSION_BUFFER, Width: bytes.max(1), Height: 1, DepthOrArraySize: 1, MipLevels: 1,
            SampleDesc: windows::Win32::Graphics::Dxgi::Common::DXGI_SAMPLE_DESC { Count: 1, Quality: 0 },
            Layout: D3D12_TEXTURE_LAYOUT_ROW_MAJOR, ..Default::default()
        };
        let mut buffer: Option<ID3D12Resource> = None;
        unsafe { device.CreateCommittedResource(&heap, D3D12_HEAP_FLAG_NONE, &desc, D3D12_RESOURCE_STATE_GENERIC_READ, None, &mut buffer) }.ok()?;
        let buffer = buffer?;
        let mut to = core::ptr::null_mut();
        unsafe { buffer.Map(0, None, Some(&mut to)) }.ok()?;
        let (stride, n) = (stride as usize, rects as usize);
        // SAFETY: the title's vertices lie in its view (checked above); the
        // mapping is ours and `bytes` long.
        let (src, dst) = unsafe {
            (core::slice::from_raw_parts(from as *const u8, n * 3 * stride),
             core::slice::from_raw_parts_mut(to.cast::<u8>(), n * 6 * stride))
        };
        expand_rects(src, dst, stride, &elements);
        unsafe { buffer.Unmap(0, None) };
        let view = D3D12_VERTEX_BUFFER_VIEW {
            BufferLocation: unsafe { buffer.GetGPUVirtualAddress() }, SizeInBytes: bytes as u32, StrideInBytes: v.StrideInBytes,
        };
        out.push((slot, buffer, view));
    }
    Some(out)
}
