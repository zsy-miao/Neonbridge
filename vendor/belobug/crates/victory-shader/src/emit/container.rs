//! The PC container around emitted DXIL: feature flags (`SFI0`), the
//! signatures (`ISG1`, `OSG1`), pipeline state validation (`PSV0`) and the
//! program (`DXIL`), laid out as validator version 1.6 lays them out, plus
//! the container hash that D3D12 checks.

/// One signature element: one row (D3D12 links stages by semantic).
#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) struct SigElement {
    pub name: String,
    pub index: u32,
    /// DXIL semantic kind (0 arbitrary, 1 vertex id, 3 position, 16 target...).
    pub kind: u8,
    /// DXIL component type (4 i32, 5 u32, 9 f32, 1 i1).
    pub comp: u8,
    /// DXIL interpolation mode (0 for none).
    pub interp: u8,
    /// Start row, if the element has a register.
    pub row: Option<u32>,
    pub col: u8,
    pub cols: u8,
    /// Components read (inputs) or written (outputs), relative to `col`.
    pub usage: u8,
    /// 16-bit on the console (declared 32-bit here).
    pub half: bool,
}

impl SigElement {
    fn system_value(&self) -> u32 {
        match self.kind {
            1 => 6,  // vertex id
            2 => 8,  // instance id
            3 => 1,  // position
            4 => 4,  // render target array index
            5 => 5,  // viewport array index
            6 => 2,  // clip distance
            7 => 3,  // cull distance
            10 => 7, // primitive id
            12 => 10,
            13 => 9, // front face
            14 => 66,
            15 => 70,
            16 => 64, // target
            17 => 65, // depth
            18 => 68,
            19 => 67,
            20 => 69,
            28 => 23, // barycentrics
            30 => 25, // cull primitive
            _ => 0,
        }
    }

    fn sig_comp(&self) -> u32 {
        match self.comp {
            1 | 5 => 1, // bool and u32
            4 => 2,
            9 => 3,
            _ => 0,
        }
    }

    /// The start column as the signature parts count it: 0 for an
    /// unallocated element (DXC's `GetColsAsMask` and usage shift).
    fn start_col(&self) -> u8 {
        if self.row.is_some() { self.col } else { 0 }
    }

    fn mask(&self) -> u8 {
        ((1u8 << self.cols) - 1) << self.start_col()
    }

    /// The name `ISG1`/`OSG1`/`PSG1` carry: DXC's `GetName`, the element's
    /// own for arbitrary semantics, the semantic's canonical spelling for
    /// system values (whatever case the module's metadata uses).
    fn part_name(&self) -> &str {
        match self.kind {
            1 => "SV_VertexID",
            2 => "SV_InstanceID",
            3 => "SV_Position",
            4 => "SV_RenderTargetArrayIndex",
            5 => "SV_ViewportArrayIndex",
            6 => "SV_ClipDistance",
            7 => "SV_CullDistance",
            10 => "SV_PrimitiveID",
            12 => "SV_SampleIndex",
            13 => "SV_IsFrontFace",
            14 => "SV_Coverage",
            15 => "SV_InnerCoverage",
            16 => "SV_Target",
            17 => "SV_Depth",
            18 => "SV_DepthLessEqual",
            19 => "SV_DepthGreaterEqual",
            20 => "SV_StencilRef",
            28 => "SV_Barycentrics",
            30 => "SV_CullPrimitive",
            _ => &self.name,
        }
    }

    /// Whether a signature part lists the element: DXC leaves out the
    /// semantics its interpretation table marks `NotInSig` (pixel inputs
    /// `SV_Coverage`, `SV_InnerCoverage`, `SV_CullPrimitive`), though their
    /// names stay in the string table.
    fn in_part(&self, input: bool) -> bool {
        !(input && matches!(self.kind, 14 | 15 | 30))
    }
}

/// `ISG1`/`OSG1`/`PSG1`: 32-byte rows, then the names, byte for byte as
/// the validator rebuilds them from the module's signature metadata
/// (`DxilProgramSignatureWriter` with validator version 1.6), so every
/// field derives from the same [`SigElement`] the metadata does.
pub(crate) fn signature(elements: &[SigElement], input: bool) -> Vec<u8> {
    let listed: Vec<usize> = (0..elements.len())
        .filter(|&i| elements[i].in_part(input))
        .collect();
    let mut out = Vec::new();
    out.extend((listed.len() as u32).to_le_bytes());
    out.extend(8u32.to_le_bytes());
    // Validator 1.6 writes one name per element in element order, repeats
    // included, except that system values share theirs (it keys names by
    // the name's pointer, which is one static string per system value).
    // Elements left out of the rows still get a name.
    let mut offsets: Vec<u32> = Vec::new();
    let mut next = 8 + 32 * listed.len() as u32;
    let mut names = Vec::new();
    for (i, e) in elements.iter().enumerate() {
        let shared = (e.kind != 0)
            .then(|| (0..i).find(|&j| elements[j].kind == e.kind))
            .flatten();
        match shared {
            Some(j) => offsets.push(offsets[j]),
            None => {
                offsets.push(next);
                next += e.part_name().len() as u32 + 1;
                names.push(e.part_name());
            }
        }
    }
    // Rows by register (unallocated last, as register -1), then by name
    // offset (DXC's `sort_sig`); the sort is stable on ties, as DXC's
    // insertion sort is on signatures this small.
    let mut order = listed;
    order.sort_by_key(|&i| (elements[i].row.unwrap_or(u32::MAX), offsets[i]));
    for i in order {
        let (e, offset) = (&elements[i], offsets[i]);
        let used = e.usage << e.start_col();
        let rw = if input { used } else { !used & 0xf };
        for v in [
            0,
            offset,
            e.index,
            e.system_value(),
            e.sig_comp(),
            e.row.unwrap_or(u32::MAX),
        ] {
            out.extend(v.to_le_bytes());
        }
        out.extend([e.mask(), rw, 0, 0]);
        out.extend(0u32.to_le_bytes());
    }
    for name in names {
        out.extend(name.as_bytes());
        out.push(0);
    }
    out
}

/// A resource bound by register, as `PSV0` lists it.
#[derive(Debug, Clone, Copy)]
pub(crate) struct Binding {
    /// PSV resource type: 1 sampler, 2 CBV.
    pub kind: u32,
    pub space: u32,
    pub register: u32,
    /// DXIL resource kind (13 cbuffer, 14 sampler).
    pub dxil_kind: u32,
}

pub(crate) struct Psv<'a> {
    /// PSV shader kind: 0 pixel, 1 vertex, 5 compute.
    pub stage: u8,
    /// Vertex: writes `SV_Position`. Pixel: writes depth.
    pub flag: bool,
    pub inputs: &'a [SigElement],
    pub outputs: &'a [SigElement],
    /// Mesh: per-primitive outputs.
    pub prims: &'a [SigElement],
    /// Mesh: its state and the group-shared bytes it declares.
    pub mesh: Option<(super::shader::Mesh, u32)>,
    /// Geometry: triangles to strips (see `Shader::rectangles`).
    pub rectangles: bool,
    pub bindings: &'a [Binding],
    /// Compute: thread-group size and required wave size.
    pub threads: [u32; 3],
    pub wave_size: Option<u32>,
}

/// `PSV0` with runtime info version 2 (validator 1.6).
pub(crate) fn psv(p: &Psv) -> Vec<u8> {
    let vectors = |els: &[SigElement]| {
        els.iter()
            .filter_map(|e| e.row)
            .map(|r| r + 1)
            .max()
            .unwrap_or(0)
    };
    let (in_vectors, out_vectors) = (vectors(p.inputs), vectors(p.outputs));
    let mut out = Vec::new();
    let u32s = |out: &mut Vec<u8>, vs: &[u32]| {
        for v in vs {
            out.extend(v.to_le_bytes());
        }
    };
    u32s(&mut out, &[48]);
    let mut info = [0u8; 48];
    info[0] = u8::from(p.flag);
    if p.stage == 0 {
        // PSInfo.SampleFrequency: an input interpolated per sample, or the
        // sample index.
        info[1] = u8::from(
            p.inputs
                .iter()
                .any(|e| matches!(e.interp, 6 | 7) || e.kind == 12),
        );
    }
    let (min, max) = p.wave_size.map_or((0, u32::MAX), |n| (n, n));
    info[16..20].copy_from_slice(&min.to_le_bytes());
    info[20..24].copy_from_slice(&max.to_le_bytes());
    info[24] = p.stage;
    for (k, n) in p.threads.into_iter().enumerate() {
        info[36 + 4 * k..40 + 4 * k].copy_from_slice(&n.to_le_bytes());
    }
    info[28] = p.inputs.len() as u8;
    info[29] = p.outputs.len() as u8;
    info[31] = in_vectors as u8;
    info[32] = out_vectors as u8;
    if let Some((mesh, shared)) = p.mesh {
        info[0..4].copy_from_slice(&shared.to_le_bytes());
        info[12..14].copy_from_slice(&(mesh.max_vertices as u16).to_le_bytes());
        info[14..16].copy_from_slice(&(mesh.max_primitives as u16).to_le_bytes());
        info[26] = vectors(p.prims) as u8;
        info[27] = mesh.topology;
        info[30] = p.prims.len() as u8;
    }
    if p.rectangles {
        // Triangles in, triangle strips out, stream 0, a position.
        for (k, n) in [3u32, 5, 1].into_iter().enumerate() {
            info[4 * k..4 * k + 4].copy_from_slice(&n.to_le_bytes());
        }
        info[12] = 1;
        info[26..28].copy_from_slice(&(super::shader::RECTANGLE_VERTICES as u16).to_le_bytes());
    }
    out.extend(info);
    u32s(&mut out, &[p.bindings.len() as u32]);
    if !p.bindings.is_empty() {
        u32s(&mut out, &[24]);
        for b in p.bindings {
            u32s(
                &mut out,
                &[b.kind, b.space, b.register, b.register, b.dxil_kind, 0],
            );
        }
    }
    // Names of arbitrary semantics (system values use ""), then indices.
    let mut strings = vec![0u8];
    let mut indices: Vec<u32> = Vec::new();
    let mut elements = Vec::new();
    for e in p.inputs.iter().chain(p.outputs).chain(p.prims) {
        let name = if e.kind == 0 {
            let needle: Vec<u8> = e.name.bytes().chain([0]).collect();
            match find(&strings, &needle) {
                Some(at) => at,
                None => {
                    strings.extend(&needle);
                    strings.len() - needle.len()
                }
            }
        } else {
            0
        };
        let index = match indices.iter().position(|&i| i == e.index) {
            Some(at) => at,
            None => {
                indices.push(e.index);
                indices.len() - 1
            }
        };
        let (start, alloc) = match e.row {
            Some(r) => (r as u8, 0x40 | (e.col << 4)),
            None => (0, 0),
        };
        elements.push((name as u32, index as u32, start, e.cols | alloc, e));
    }
    strings.resize(strings.len().next_multiple_of(4), 0);
    u32s(&mut out, &[strings.len() as u32]);
    out.extend(&strings);
    u32s(&mut out, &[indices.len() as u32]);
    u32s(&mut out, &indices);
    if !elements.is_empty() {
        u32s(&mut out, &[16]);
        for (name, index, start, cols, e) in elements {
            u32s(&mut out, &[name, index]);
            out.extend([1, start, cols, e.kind, e.sig_comp() as u8, e.interp, 0, 0]);
        }
    }
    // Input-to-output dependencies, as the view-id state declares them:
    // every output scalar on every input scalar.
    if in_vectors > 0 && out_vectors > 0 {
        let scalars = |els: &[SigElement]| {
            els.iter()
                .filter_map(|e| e.row.map(|r| r * 4 + u32::from(e.col + e.cols)))
                .max()
                .unwrap_or(0)
        };
        let (ni, no) = (scalars(p.inputs), scalars(p.outputs));
        let per = (out_vectors * 4).div_ceil(32);
        for i in 0..in_vectors * 4 {
            for k in 0..per {
                let bits = if i < ni {
                    no.saturating_sub(32 * k).min(32)
                } else {
                    0
                };
                let mask = if bits == 32 {
                    u32::MAX
                } else {
                    (1u32 << bits) - 1
                };
                u32s(&mut out, &[mask]);
            }
        }
    }
    out
}

fn find(hay: &[u8], needle: &[u8]) -> Option<usize> {
    hay.windows(needle.len()).position(|w| w == needle)
}

/// The container: parts in order, hashed.
pub(crate) fn build(parts: &[(&[u8; 4], &[u8])]) -> Vec<u8> {
    let table = 0x20 + 4 * parts.len();
    let total = table + parts.iter().map(|(_, d)| 8 + d.len()).sum::<usize>();
    let mut out = Vec::with_capacity(total);
    out.extend(b"DXBC");
    out.extend([0; 16]);
    out.extend(1u16.to_le_bytes());
    out.extend(0u16.to_le_bytes());
    out.extend((total as u32).to_le_bytes());
    out.extend((parts.len() as u32).to_le_bytes());
    let mut offset = table;
    for (_, d) in parts {
        out.extend((offset as u32).to_le_bytes());
        offset += 8 + d.len();
    }
    for (fourcc, d) in parts {
        out.extend(*fourcc);
        out.extend((d.len() as u32).to_le_bytes());
        out.extend(*d);
    }
    let digest = hash(&out[20..]);
    out[4..20].copy_from_slice(&digest);
    out
}

/// The container hash D3D12 checks: MD5 whose last block carries the bit
/// count first and `2 × bytes + 1` last instead of standard padding.
pub(crate) fn hash(data: &[u8]) -> [u8; 16] {
    let len = data.len();
    let left = len % 64;
    let mut tail = Vec::with_capacity(128);
    if left < 56 {
        tail.extend(((len as u32) << 3).to_le_bytes());
        tail.extend(&data[len - left..]);
        tail.push(0x80);
        tail.resize(60, 0);
    } else {
        tail.extend(&data[len - left..]);
        tail.push(0x80);
        tail.resize(64, 0);
        tail.extend(((len as u32) << 3).to_le_bytes());
        tail.resize(124, 0);
    }
    tail.extend(((len as u32) << 1 | 1).to_le_bytes());
    let mut state = [0x6745_2301u32, 0xefcd_ab89, 0x98ba_dcfe, 0x1032_5476];
    for block in data[..len - left]
        .as_chunks::<64>()
        .0
        .iter()
        .chain(tail.as_chunks::<64>().0)
    {
        md5_block(&mut state, block);
    }
    let mut out = [0; 16];
    for (o, s) in out.as_chunks_mut::<4>().0.iter_mut().zip(state) {
        o.copy_from_slice(&s.to_le_bytes());
    }
    out
}

fn md5_block(state: &mut [u32; 4], block: &[u8; 64]) {
    const S: [u32; 16] = [7, 12, 17, 22, 5, 9, 14, 20, 4, 11, 16, 23, 6, 10, 15, 21];
    let x: Vec<u32> = block
        .as_chunks::<4>()
        .0
        .iter()
        .map(|c| u32::from_le_bytes(*c))
        .collect();
    let [mut a, mut b, mut c, mut d] = *state;
    for i in 0..64 {
        let (f, g) = match i / 16 {
            0 => ((b & c) | (!b & d), i),
            1 => ((d & b) | (!d & c), (5 * i + 1) % 16),
            2 => (b ^ c ^ d, (3 * i + 5) % 16),
            _ => (c ^ (b | !d), (7 * i) % 16),
        };
        let k = ((i as f64 + 1.0).sin().abs() * 4_294_967_296.0) as u32;
        let rotated = a
            .wrapping_add(f)
            .wrapping_add(k)
            .wrapping_add(x[g])
            .rotate_left(S[(i / 16) * 4 + i % 4]);
        (a, b, c, d) = (d, b.wrapping_add(rotated), b, c);
    }
    for (s, v) in state.iter_mut().zip([a, b, c, d]) {
        *s = s.wrapping_add(v);
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn el(name: &str, kind: u8, row: Option<u32>, col: u8, cols: u8, usage: u8) -> SigElement {
        SigElement {
            name: name.into(),
            index: 0,
            kind,
            comp: 9,
            interp: 2,
            row,
            col,
            cols,
            usage,
            half: false,
        }
    }

    /// A part's rows: (name, register, mask, rw mask).
    fn rows(part: &[u8]) -> Vec<(String, u32, u8, u8)> {
        let u = |at: usize| u32::from_le_bytes(part[at..at + 4].try_into().unwrap());
        (0..u(0) as usize)
            .map(|k| {
                let at = 8 + 32 * k;
                let name = &part[u(at + 4) as usize..];
                let name = name.split(|&b| b == 0).next().unwrap();
                (
                    String::from_utf8(name.to_vec()).unwrap(),
                    u(at + 20),
                    part[at + 24],
                    part[at + 25],
                )
            })
            .collect()
    }

    /// Elements sharing a row are listed by name offset (element order),
    /// not by column, as the validator's rebuilt part lists them.
    #[test]
    fn a_shared_row_keeps_element_order() {
        let els = [
            el("TEXCOORD", 0, Some(1), 2, 2, 0b11),
            el("COLOR", 0, Some(1), 0, 2, 0b01),
            el("NORMAL", 0, Some(0), 0, 3, 0b111),
        ];
        let r = rows(&signature(&els, true));
        let names: Vec<_> = r.iter().map(|r| r.0.as_str()).collect();
        assert_eq!(names, ["NORMAL", "TEXCOORD", "COLOR"]);
        assert_eq!((r[1].2, r[1].3), (0b1100, 0b1100));
        assert_eq!((r[2].2, r[2].3), (0b0011, 0b0001));
    }

    /// An unallocated element counts its mask and usage from column 0,
    /// whatever column it carries; outputs mark the unwritten channels.
    #[test]
    fn unallocated_elements_count_from_column_zero() {
        let els = [el("SV_Depth", 17, None, 2, 1, 1)];
        assert_eq!(
            rows(&signature(&els, false)),
            [("SV_Depth".into(), u32::MAX, 1, 0b1110)]
        );
        let els = [el("SV_Barycentrics", 28, None, 1, 3, 0b110)];
        assert_eq!(
            rows(&signature(&els, true)),
            [("SV_Barycentrics".into(), u32::MAX, 0b111, 0b110)]
        );
    }

    /// System values carry their canonical name; input coverage is named but
    /// not listed (DXC's `NotInSig`).
    #[test]
    fn system_value_names_and_left_out_elements() {
        let mut cov = el("SV_COVERAGE", 14, None, 0, 1, 1);
        cov.comp = 5;
        let els = [el("SV_POSITION", 3, Some(0), 0, 4, 0b11), cov];
        let part = signature(&els, true);
        assert_eq!(rows(&part), [("SV_Position".into(), 0, 0b1111, 0b11)]);
        // Both names in the string table, the coverage's after the row.
        assert!(part.ends_with(b"SV_Position\0SV_Coverage\0"));
        assert_eq!(part.len(), 8 + 32 + 12 + 12);
    }

    #[test]
    fn md5_core_matches_the_standard() {
        // Standard MD5 of "abc" with standard padding, through md5_block.
        let mut block = b"abc".to_vec();
        block.push(0x80);
        block.resize(56, 0);
        block.extend(24u64.to_le_bytes());
        let mut state = [0x6745_2301u32, 0xefcd_ab89, 0x98ba_dcfe, 0x1032_5476];
        md5_block(&mut state, block.as_slice().try_into().unwrap());
        let hex: String = state
            .iter()
            .flat_map(|s| s.to_le_bytes())
            .map(|b| format!("{b:02x}"))
            .collect();
        assert_eq!(hex, "900150983cd24fb0d6963f7d28e17f72");
    }
}
