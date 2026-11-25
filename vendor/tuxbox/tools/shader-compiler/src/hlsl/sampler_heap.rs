//! FSSM descriptor-table and root-constant entry registers.
use crate::{meta::RootParamKind, ShaderExtract, Stage};

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(super) struct DirectSampler {
    pub register: u32,
    pub space: u32,
    pub count: u32,
    pub component: u32,
}

pub(super) fn constants(ex: &ShaderExtract) -> Option<(u8, DirectSampler)> {
    use tuxbox_dxbc::compute_table::records;
    let root = ex.root_sig.as_ref()?;
    let (index, start) = match (ex.stage, root.params.as_slice()) {
        (Stage::Pixel, [table, _, rest @ ..])
            if matches!(table.kind, RootParamKind::Table(_))
                && (rest.is_empty()
                    || matches!(rest, [map] if matches!(map.kind, RootParamKind::Descriptor {kind: 3, register: 0, space: 32766}))) =>
        {
            (1, 9u8)
        }
        (Stage::Compute, [table, _]) if matches!(table.kind, RootParamKind::Table(_)) => (1, 5),
        (Stage::Compute, [first, second, _])
            if matches!(first.kind, RootParamKind::Table(_))
                && matches!(second.kind, RootParamKind::Table(_)) =>
        {
            (2, 6)
        }
        _ => return None,
    };
    let constants = &root.params[index];
    let RootParamKind::Constants {
        register,
        space,
        num32,
    } = constants.kind
    else {
        return None;
    };
    if ex.stage == Stage::Compute {
        if let Some(count) = renamed_constants(ex, index as u32, start, num32, register) {
            return Some((start, DirectSampler { register, space, count, component: 0 }));
        }
    }
    if !(1..=4).contains(&num32)
        || !records(&ex.fssm)?.any(|[class, offset, reg, _, location]| {
            class == 2
                && offset == 0
                && reg == register
                && location == ((index as u32) << 16 | (u32::from(start) << 8) | 4)
        })
    {
        return None;
    }
    if !ex
        .resources
        .iter()
        .any(|r| (r.res_type, r.space, r.lower, r.upper) == (2, space, register, register))
    {
        return None;
    }
    Some((
        start,
        DirectSampler {
            register,
            space,
            count: num32,
            component: 0,
        },
    ))
}

/// Root constants whose register differs between the program and the pipeline's root
/// signature. The console fills the launch registers by root parameter, so one program runs
/// under root signatures that number the constants differently; the host binds them by
/// register and needs the root signature's. Returns how many registers are filled.
fn renamed_constants(ex: &ShaderExtract, index: u32, start: u8, num32: u32, register: u32) -> Option<u32> {
    use tuxbox_dxbc::compute_table::{records, user_data_records};
    let mut rows = user_data_records(&ex.fssm)?.filter(|&[class, ..]| class == 0);
    let [_, 0, count @ 1..=4, name, location] = rows.next()? else { return None };
    if rows.next().is_some() || count > num32 || name == register || location != index << 8 | u32::from(start) {
        return None;
    }
    let mut used = records(&ex.fssm)?.filter(|&[class, ..]| class == 2);
    if used.next()? != [2, 0, name, 0, location << 8 | 4] || used.next().is_some() {
        return None;
    }
    let mut bound = ex.resources.iter().filter(|r| r.res_type == 2 && r.lower <= name && name <= r.upper);
    let binding = bound.next()?;
    (bound.next().is_none() && binding.lower == binding.upper).then_some(count)
}

/// Root constants of a root-less program: the first user SGPR and the constant buffer.
///
/// The FSSM user-data row of class 0 names the launch registers the root constants fill
/// (count, register, root parameter and first SGPR). The used-resource row of class 2
/// repeats register and location, and PSV gives the register's space. Accept the program
/// only when exactly one of each exists and all agree; the pipeline's root signature, which
/// the program was compiled against, holds the constants at that register.
pub(super) fn rootless_constants(ex: &ShaderExtract) -> Option<(u8, DirectSampler)> {
    use tuxbox_dxbc::compute_table::{records, user_data_records};
    if ex.root_sig.is_some() || !matches!(ex.stage, Stage::Pixel | Stage::Vertex | Stage::Compute) {
        return None;
    }
    let mut rows = user_data_records(&ex.fssm)?.filter(|&[class, ..]| class == 0);
    let [_, 0, count @ 1..=32, register, location @ 0..=0xffff] = rows.next()? else { return None };
    if rows.next().is_some() {
        return None;
    }
    let mut used = records(&ex.fssm)?.filter(|&[class, ..]| class == 2);
    if used.next()? != [2, 0, register, 0, location << 8 | 4] || used.next().is_some() {
        return None;
    }
    let mut bound = ex.resources.iter()
        .filter(|r| r.res_type == 2 && r.lower <= register && register <= r.upper);
    let binding = bound.next()?;
    if bound.next().is_some() || binding.lower != binding.upper {
        return None;
    }
    let start = location as u8;
    (u32::from(start) + count <= 104).then_some((start, DirectSampler {
        register,
        space: binding.space,
        count,
        component: 0,
    }))
}

/// First SGPR after the user data of a compute program, where the hardware puts the group
/// ID. FSSM user-data rows name every launch argument: root constants (class 0, one SGPR
/// each), root descriptors (1 and 2, a pair), descriptor tables (3) and the heap bases
/// (5 and 6, one each). Static samplers (4) take no register.
pub(super) fn user_data_end(ex: &ShaderExtract) -> Option<usize> {
    let mut end = 0;
    for [class, _, width, _, location] in tuxbox_dxbc::compute_table::user_data_records(&ex.fssm)? {
        let width = match class {
            0 => width,
            1 | 2 => 2,
            3 | 5 | 6 => 1,
            4 => continue,
            _ => return None,
        };
        end = end.max((location & 0xff) as usize + width as usize);
    }
    (4..=100).contains(&end).then_some(end)
}

/// User SGPRs that root constants fill in a root-less program, read or not.
pub(super) fn rootless_constant_sgprs(ex: &ShaderExtract) -> Vec<String> {
    let rows = ex.root_sig.is_none()
        .then(|| tuxbox_dxbc::compute_table::user_data_records(&ex.fssm)).flatten();
    rows.into_iter().flatten().filter(|&[class, ..]| class == 0)
        .flat_map(|[_, _, count, _, location]| (0..count.min(104)).map(move |k| format!("s{}", (location & 0xff) + k)))
        .collect()
}

/// HLSL name of root constant `component`.
pub(super) fn constant_name(key: &DirectSampler, component: u32) -> String {
    match key.count {
        1..=4 => format!("tb_index{}_{}.{}", key.register, key.space, super::XYZW[component as usize]),
        _ => format!("tb_index{}_{}_{component}", key.register, key.space),
    }
}

pub(super) fn init_constants(
    ex: &ShaderExtract,
    packed: bool,
    regs: &std::collections::BTreeSet<String>,
    output: &mut String,
) {
    use std::fmt::Write;
    let Some((start, key)) = constants(ex).or_else(|| rootless_constants(ex).filter(|_| !packed)) else {
        return;
    };
    for component in 0..key.count {
        let reg = format!("s{}", u32::from(start) + component);
        if regs.contains(&reg) {
            writeln!(output, "    {reg} = {};", constant_name(&key, component)).unwrap();
        }
    }
}

pub(super) fn index(ex: &ShaderExtract, entry: u8) -> Option<DirectSampler> {
    if ex.stage != Stage::Pixel || ex.root_sig.as_ref()?.flags & 0x800 == 0 {
        return None;
    }
    let (start, mut key) = constants(ex)?;
    key.component = u32::from(entry.checked_sub(start)?);
    (key.component < key.count).then_some(key)
}

/// These vertex shaders receive the table in s8 even after their generated fetch call.
pub(super) fn vertex_s8_table(ex: &ShaderExtract) -> bool {
    ex.stage == Stage::Vertex
        && tuxbox_dxbc::compute_table::records(&ex.fssm).is_some_and(|mut rows| {
            rows.len() != 0
                && rows.all(|[class, _, _, _, loc]| matches!(class, 0x302 | 0x300) && loc >> 8 == 8)
        })
}

/// Confirm that the pixel-stage resource uses the table slot in the shared raster root.
pub(super) fn matches_table(
    ex: &ShaderExtract,
    offset: u32,
    kind: u32,
    reg: u32,
    space: u32,
) -> bool {
    let (class, psv_type) = match kind {
        0 => (0x300, 3),
        1 => (0x303, 6),
        2 => (0x302, 2),
        _ => return false,
    };
    tuxbox_dxbc::compute_table::records(&ex.fssm).is_some_and(|mut rows| {
        rows.any(|[ty, pos, slot, _, _]| {
            ty == class && pos.checked_mul(4) == Some(offset) && slot == reg
        })
    }) && ex
        .resources
        .iter()
        .any(|r| (r.res_type, r.lower, r.upper, r.space) == (psv_type, reg, reg, space))
}

/// Requires the actual resource heap, its FSSM user-data location, and the
/// runtime's private guest-to-host descriptor index map in the raster root.
pub(super) fn resource_base(ex: &ShaderExtract) -> Option<u8> {
    let root = ex.root_sig.as_ref()?;
    if ex.stage != Stage::Pixel
        || ex.required_features? & (3 << 25) != 1 << 25
        || root.flags & 0x400 == 0
        || !matches!(root.params.as_slice(), [_, _, map] if matches!(map.kind, RootParamKind::Descriptor { kind: 3, register: 0, space: 32766 }))
    {
        return None;
    }
    let (_, c) = constants(ex)?;
    tuxbox_dxbc::compute_table::scalar_heap_operand(&ex.isa, c.count)?;
    u8::try_from(tuxbox_dxbc::compute_table::pixel_heap_base(
        &ex.fssm, c.count,
    )?)
    .ok()
}

/// Match the literal RDNA2 state of the normalized, isotropic samplers used
/// with direct textures to a static sampler in the supplied root signature.
/// Require every serialized state field before selecting a register. A static
/// sampler is immutable, so any register with identical state samples alike;
/// one the shader declares (PSV) is preferred. `compare` selects comparison
/// samplers (depth-compare image ops) and only those.
pub(super) fn inline_sampler(ex: &ShaderExtract, literal: [u32; 4], compare: bool) -> Option<(u32, u32)> {
    let state = |s: &[u32; 13]| -> Option<(u32, u32)> {
        let vis = s[12];
        if !(vis == 0 || (vis == 5 && ex.stage == Stage::Pixel) || (vis == 7 && ex.stage == Stage::Mesh)) {
            return None;
        }
        tuxbox_dxbc::inline_sampler::matches_static(literal, s, compare)
            .then_some((s[10], s[11]))
    };
    let Some(root) = ex.root_sig.as_ref() else {
        // Root-less programs: the title's own root signature supplies the static
        // sampler, so bind the inline words to the one sampler PSV declares. Only
        // accept words shaped like a static sampler (word 1 = MAX_LOD 12, no bias).
        let mut declared = ex.resources.iter().filter(|r| r.res_type == 1);
        let sampler = declared.next()?;
        if declared.next().is_some() || sampler.lower != sampler.upper || literal[1] != 0x00c00000 {
            return None;
        }
        return Some((sampler.lower, sampler.space));
    };
    let samplers = &root.static_samplers;
    let declared = |&(slot, space): &(u32, u32)| {
        ex.resources.iter().any(|r| (r.res_type, r.lower, r.upper, r.space) == (1, slot, slot, space))
    };
    samplers.iter().filter_map(state).find(declared)
        .or_else(|| samplers.iter().find_map(state))
}

/// An empty EXEC branch can skip inline sampler initialization only when the
/// immediately following image instruction is also inactive. In a full
/// compute wave ANDing EXEC with the preserved launch mask changes no bits.
pub(super) fn guarded_words(
    insns: &[super::Insn],
    image: usize,
    base: u8,
    group: [u32; 3],
) -> Option<[u32; 4]> {
    use super::{Emitter, Enc, Opnd};
    use crate::rdna2::clobbers_scalar;
    let n = group.into_iter().try_fold(1u32, |n, w| n.checked_mul(w))?;
    if n == 0 || n % 64 != 0 {
        return None;
    }
    let restore = image.checked_sub(1)?;
    let r = &insns[restore];
    let [Opnd::Sgpr(saved)] = *r.src.as_slice() else {
        return None;
    };
    if r.mnemonic != "s_mov_b64" || r.dst != [Opnd::ExecLo] {
        return None;
    }
    let target = |i: &super::Insn| i.offset as i64 + 4 + i.aux.imm * 4;
    let branch = insns[..restore]
        .iter()
        .rposition(|i| i.mnemonic == "s_cbranch_execz" && target(i) == r.offset as i64)?;
    let guard = &insns[branch.checked_sub(1)?];
    let [Opnd::Sgpr(entry)] = *guard.src.as_slice() else {
        return None;
    };
    if guard.mnemonic != "s_and_saveexec_b64" || guard.dst != [Opnd::Sgpr(saved)] || saved == entry
    {
        return None;
    }
    let capture = insns[..branch - 1].iter().position(|i| {
        i.mnemonic == "s_mov_b64" && i.dst == [Opnd::Sgpr(entry)] && i.src == [Opnd::ExecLo]
    })?;
    if insns[..capture].iter().any(|i| {
        i.mnemonic.contains("branch")
            || i.mnemonic.contains("exec")
            || i.mnemonic.starts_with("v_cmpx")
            || i.dst
                .iter()
                .any(|d| matches!(d, Opnd::ExecLo | Opnd::ExecHi))
    }) {
        return None;
    }
    if insns[capture + 1..branch - 1]
        .iter()
        .any(|i| clobbers_scalar(i, entry) || clobbers_scalar(i, entry + 1))
    {
        return None;
    }
    if insns.iter().enumerate().any(|(j, i)| {
        matches!(
            i.mnemonic.as_str(),
            "s_setpc_b64" | "s_swappc_b64" | "s_call_b64"
        ) || (i.enc == Enc::Sopp
            && Emitter::branch_cond(i.op).is_some()
            && (target(i) <= insns[capture].offset as i64
                || ((guard.offset as i64 + 1..=insns[image].offset as i64).contains(&target(i))
                    && j != branch)))
    }) {
        return None;
    }
    let mut words = [0; 4];
    let block = &insns[branch + 1..restore];
    if block.iter().any(|i| {
        i.mnemonic.contains("branch") || clobbers_scalar(i, saved) || clobbers_scalar(i, saved + 1)
    }) {
        return None;
    }
    for (j, out) in words.iter_mut().enumerate() {
        let reg = base.checked_add(j as u8)?;
        let i = block.iter().rev().find(|op| clobbers_scalar(op, reg))?;
        if i.dst != [Opnd::Sgpr(reg)] {
            return None;
        }
        *out = match (i.mnemonic.as_str(), i.src.as_slice()) {
            ("s_movk_i32", []) => i.aux.imm as i16 as i32 as u32,
            ("s_mov_b32", [Opnd::ConstInt(v)]) => *v as u32,
            ("s_mov_b32", [Opnd::Literal(v)]) => *v,
            ("s_brev_b32", [Opnd::ConstInt(v)]) => (*v as u32).reverse_bits(),
            ("s_brev_b32", [Opnd::Literal(v)]) => v.reverse_bits(),
            _ => return None,
        };
    }
    Some(words)
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::psv::PsvResource;

    /// FSSM with user-data rows and used-resource rows.
    fn fssm(user: &[[u32; 5]], used: &[[u32; 5]]) -> Vec<u8> {
        let mut program = vec![0u8; 0x110];
        program[..8].copy_from_slice(b"FSSM\x01\x01\0\0");
        for (rows, field, count) in [(user, 0xe8, 0xf8), (used, 0xf0, 0xfc)] {
            let offset = (program.len() - 16) as u64;
            program[field..field + 8].copy_from_slice(&offset.to_le_bytes());
            program[count..count + 4].copy_from_slice(&(rows.len() as u32).to_le_bytes());
            program.extend(rows.iter().flatten().copied().flat_map(u32::to_le_bytes));
        }
        let size = (program.len() - 16) as u32;
        program[8..12].copy_from_slice(&size.to_le_bytes());
        program
    }

    const CONSTANTS: [u32; 5] = [0, 0, 2, 500, 0x30a]; // two root constants in s10:s11
    const RESOURCE_HEAP: [u32; 5] = [5, 0, 1, 0, 0x0e];
    const SAMPLER_HEAP: [u32; 5] = [6, 0, 1, 0, 0x10];
    const USED: [u32; 5] = [2, 0, 500, 0, 0x30a04];

    /// A root-less pixel program with root constants and both heaps.
    fn pixel(words: &[u32]) -> ShaderExtract {
        let isa: Vec<u8> = words.iter().flat_map(|w| w.to_le_bytes()).collect();
        let mut ex = super::super::mesh::heap_extract(&[], 0, &isa);
        ex.stage = Stage::Pixel;
        ex.shader_model = (6, 6);
        ex.required_features = Some(3 << 25);
        ex.root_sig = None;
        ex.has_root_sig = false;
        ex.fssm = fssm(&[CONSTANTS, RESOURCE_HEAP, SAMPLER_HEAP], &[USED]);
        ex.resources = vec![PsvResource { res_type: 2, space: 0, lower: 500, upper: 500 }];
        ex
    }

    const SAMPLE: [u32; 9] = [
        0x8f00850a, // s0 = s10 << 5
        0x8f01840b, // s1 = s11 << 4
        0xf40c0107, 0x00000000, // s[4:11] = heap[s14:15][s0]
        0xf4080508, 0x02000000, // s[20:23] = samplers[s16:17][s1]
        0xf0800108, 0x00a10704, // v7 = image_sample(v[4:5], s[4:11], s[20:23])
        0xbf810000,
    ];

    fn refusal(ex: &ShaderExtract) -> String {
        match super::super::translate(ex) {
            Ok(_) => panic!("translated"),
            Err(reason) => reason,
        }
    }

    #[test]
    fn pixel_heap_indices_come_from_root_constants() {
        let out = super::super::translate(&pixel(&SAMPLE)).unwrap();
        assert_eq!(out.profile, "ps_6_6");
        for line in [
            "cbuffer TBIndex500_0 : register(b500, space0) { uint2 tb_index500_0; };",
            "s10 = tb_index500_0.x;",
            "s11 = tb_index500_0.y;",
            "s4 = tb_heap_index(s0 >> 5)",
            "Texture2D<float4> tb_heap_t2_4;",
            "tb_heap_t2_4 = ResourceDescriptorHeap[NonUniformResourceIndex(s4)];",
            "s20 = (s1 >> 4)",
            "SamplerDescriptorHeap[NonUniformResourceIndex(s20)]",
        ] {
            assert!(out.hlsl.contains(line), "{line}");
        }
    }

    #[test]
    fn pixel_heap_bases_need_one_row_and_the_feature() {
        let base = super::super::pixel_heap_base;
        let ex = pixel(&SAMPLE);
        assert_eq!((base(&ex, 5), base(&ex, 6)), (Some(14), Some(16)));
        let mut samplers = ex.clone();
        samplers.required_features = Some(1 << 26);
        assert_eq!((base(&samplers, 5), base(&samplers, 6)), (None, Some(16)));
        let mut twice = ex.clone();
        twice.fssm = fssm(&[RESOURCE_HEAP, RESOURCE_HEAP], &[]);
        assert_eq!(base(&twice, 5), None);
        let mut wide = ex.clone();
        wide.fssm = fssm(&[[5, 0, 2, 0, 0x0e]], &[]);
        assert_eq!(base(&wide, 5), None);
        for stage in [Stage::Vertex, Stage::Compute, Stage::Mesh] {
            let mut other = ex.clone();
            other.stage = stage;
            assert_eq!(base(&other, 5), None);
        }
        // Without the feature the same program is refused as before.
        let mut plain = ex;
        plain.required_features = Some(0);
        assert!(refusal(&plain).contains("dynamic soffset unsupported"));
    }

    #[test]
    fn pixel_heap_refuses_what_it_cannot_prove() {
        // A first-lane index is the pixel's own one: each pixel is its own wave.
        let mut lane = SAMPLE.to_vec();
        lane[0] = 0x8f008502; // s0 = s2 << 5
        lane.insert(0, 0x7e040505); // s2 = readfirstlane(v5)
        assert!(super::super::translate(&pixel(&lane)).is_ok());
        // The byte offset is not a whole descriptor.
        let mut unscaled = SAMPLE;
        unscaled[0] = 0x8f00840a; // s0 = s10 << 4
        assert!(refusal(&pixel(&unscaled)).contains("dynamic soffset unsupported"));
        let mut unscaled = SAMPLE;
        unscaled[1] = 0x8f01850b; // s1 = s11 << 5
        assert!(refusal(&pixel(&unscaled)).contains("dynamic soffset unsupported"));
        // A descriptor word is replaced after the load.
        let mut patched = SAMPLE.to_vec();
        patched.insert(6, 0x8f06850a); // s6 = s10 << 5
        assert!(refusal(&pixel(&patched)).contains("not an image descriptor"));
        // A buffer descriptor could be a constant buffer or a raw buffer.
        let mut buffer = SAMPLE;
        buffer[2] = 0xf4080107; // s[4:7] = heap[s14:15][s0]
        assert!(refusal(&pixel(&buffer)).contains("heap buffer in a pixel shader"));
        // The texel type of a view is unknown.
        let mut load = SAMPLE;
        load[6] = 0xf0000108; // image_load
        assert!(refusal(&pixel(&load)).contains("unknown component type"));
    }

    #[test]
    fn sampled_value_must_not_keep_sampler_offset_proof() {
        // v7 = v5 << 4; sample overwrites v7; readfirstlane s1,v7.
        // The second sampler fetch must not treat sample bits as an aligned offset.
        let mut words = SAMPLE[..6].to_vec();
        words.push(0x340e0a84);
        words.extend([SAMPLE[6],SAMPLE[7],0x7e020507,SAMPLE[4],SAMPLE[5],
                      SAMPLE[6],SAMPLE[7],SAMPLE[8]]);
        assert!(refusal(&pixel(&words)).contains("dynamic soffset unsupported"));
    }

    #[test]
    fn second_sample_component_must_kill_offset_proof() {
        // A two-component image result starts at v7 and overwrites v8 as well.
        let mut words = SAMPLE[..6].to_vec();
        words.push(0x34100a84); // v8 = v5 << 4
        words.extend([SAMPLE[6] | 0x200,SAMPLE[7],0x7e020508,SAMPLE[4],SAMPLE[5],
                      SAMPLE[6],SAMPLE[7],SAMPLE[8]]);
        assert!(refusal(&pixel(&words)).contains("dynamic soffset unsupported"));
    }

    #[test]
    fn sampler_heap_can_track_a_vector_offset_copy() {
        // v6 = v5 << 4; v7 = v6; s1 = readfirstlane(v7).
        // The aligned sampler offset survives a genuine copy.
        let mut words = vec![SAMPLE[0],0x340c0a84,0x7e0e0306,0x7e020507];
        words.extend(&SAMPLE[2..]);
        assert!(super::super::translate(&pixel(&words)).is_ok());
    }

    #[test]
    fn mixed_texture_words_are_not_an_intact_heap_descriptor() {
        // Two independent indices and loads, then replace word 1 of A with
        // word 1 of B. The old proof only checked the ordinal of each word.
        let mut words = vec![SAMPLE[0],SAMPLE[1],0x8f02850b,0xf40c0607,0x04000000];
        words.extend(&SAMPLE[2..6]);
        words.extend([0xbe850319,SAMPLE[6],SAMPLE[7],SAMPLE[8]]);
        assert!(refusal(&pixel(&words)).contains("not an image descriptor"));
    }

    #[test]
    fn mixed_sampler_words_are_not_an_intact_heap_descriptor() {
        let mut words = vec![SAMPLE[0],SAMPLE[1],0x8f03840a,0xf4080608,0x06000000];
        words.extend(&SAMPLE[2..6]);
        words.extend([0xbe950319,SAMPLE[6],SAMPLE[7],SAMPLE[8]]);
        assert!(refusal(&pixel(&words)).contains("not a sampler descriptor"));
    }

    #[test]
    fn complete_texture_descriptor_copy_stays_supported() {
        let mut words = vec![SAMPLE[0],SAMPLE[1],0x8f02850b,0xf40c0607,0x04000000];
        words.extend(&SAMPLE[2..6]);
        words.extend([0xbe840418,0xbe86041a,0xbe88041c,0xbe8a041e,
                      SAMPLE[6],SAMPLE[7],SAMPLE[8]]);
        assert!(super::super::translate(&pixel(&words)).is_ok());
    }

    #[test]
    fn branch_must_not_hide_mixed_texture_words() {
        let mut words = vec![SAMPLE[0],SAMPLE[1],0x8f02850b,0xf40c0607,0x04000000];
        words.extend(&SAMPLE[2..6]);
        // Unconditional branch to the next instruction, then mixed word.
        words.extend([0xbf820000,0xbe850319,SAMPLE[6],SAMPLE[7],SAMPLE[8]]);
        assert!(refusal(&pixel(&words)).contains("not an image descriptor"));
    }

    #[test]
    fn branch_can_select_complete_texture_descriptors() {
        // SCC from s1 = s11 << 4. If SCC=0 skip A; otherwise use A.
        // Each branch loads all eight words at the same destination.
        let words = [SAMPLE[0],0x8f02850b,SAMPLE[1],0xbf840003,
            SAMPLE[2],SAMPLE[3],0xbf820002,0xf40c0107,0x04000000,
            SAMPLE[4],SAMPLE[5],SAMPLE[6],SAMPLE[7],SAMPLE[8]];
        assert!(super::super::translate(&pixel(&words)).is_ok());
    }

    #[test]
    fn branch_must_not_mix_only_on_one_path() {
        let mut words = vec![SAMPLE[0],SAMPLE[1],0x8f02850b,0xf40c0607,0x04000000];
        words.extend(&SAMPLE[2..6]);
        words.extend([0xbf840001,0xbe850319,SAMPLE[6],SAMPLE[7],SAMPLE[8]]);
        assert!(refusal(&pixel(&words)).contains("not an image descriptor"));
    }

    #[test]
    fn loop_reloads_must_not_keep_previous_generation_words() {
        let mut words = SAMPLE[..6].to_vec();
        // Save old word 1, reload the complete texture descriptor, decrement
        // a counter and loop to the save. At exit graft the previous word.
        words.extend([0xbe830382,0xbe990305,0x8900a000,SAMPLE[2],SAMPLE[3],
                      0x80838103,0xbf068003,0xbf84fff9,
                      0xbe850319,SAMPLE[6],SAMPLE[7],SAMPLE[8]]);
        assert!(refusal(&pixel(&words)).contains("not an image descriptor"));
        words.remove(words.len()-4);
        assert!(super::super::translate(&pixel(&words)).is_ok());
    }

    #[test]
    fn scalar_bitwise_offset_alignment_is_preserved_only_when_proven() {
        for op in [0x8900a000, 0x8800a000, 0x87008100] {
            let mut words = SAMPLE.to_vec();
            words.insert(2, op);
            assert!(super::super::translate(&pixel(&words)).is_ok(), "{op:#x}");
        }
        for op in [0x89008100, 0x88008100] {
            let mut words = SAMPLE.to_vec();
            words.insert(2, op);
            assert!(refusal(&pixel(&words)).contains("dynamic soffset unsupported"));
        }
    }

    #[test]
    fn root_less_constants_must_agree_everywhere() {
        let ex = pixel(&SAMPLE);
        let (start, key) = rootless_constants(&ex).unwrap();
        assert_eq!((start, key.register, key.space, key.count), (10, 500, 0, 2));
        let with = |user: &[[u32; 5]], used: &[[u32; 5]]| {
            let mut ex = ex.clone();
            ex.fssm = fssm(user, used);
            rootless_constants(&ex)
        };
        assert!(with(&[CONSTANTS], &[]).is_none());
        assert!(with(&[CONSTANTS, CONSTANTS], &[USED]).is_none());
        assert!(with(&[CONSTANTS], &[USED, USED]).is_none());
        assert!(with(&[CONSTANTS], &[[2, 0, 501, 0, 0x30a04]]).is_none());
        assert!(with(&[CONSTANTS], &[[2, 0, 500, 0, 0x30b04]]).is_none());
        let mut unbound = ex.clone();
        unbound.resources.clear();
        assert!(rootless_constants(&unbound).is_none());
        let mut twice = ex.clone();
        twice.resources.push(PsvResource { res_type: 2, space: 1, lower: 500, upper: 500 });
        assert!(rootless_constants(&twice).is_none());
        // A heap index from an unresolved root constant would read as zero.
        assert!(refusal(&unbound).contains("root constant s10"));
    }

    #[test]
    fn more_than_four_constants_are_declared_without_padding() {
        let mut ex = pixel(&SAMPLE);
        ex.fssm = fssm(&[[0, 0, 5, 500, 0x30a], [5, 0, 1, 0, 0x12], SAMPLER_HEAP], &[USED]);
        // The heap base moves to s18 so that it does not overlap the constants.
        ex.isa[8..12].copy_from_slice(&0xf40c0109u32.to_le_bytes());
        ex.isa_data = ex.isa.clone();
        let out = super::super::translate(&ex).unwrap();
        assert!(out.hlsl.contains("{ uint tb_index500_0_0; uint tb_index500_0_1; uint tb_index500_0_2; uint tb_index500_0_3; uint tb_index500_0_4; };"));
        assert!(out.hlsl.contains("s10 = tb_index500_0_0;"));
    }

    #[test]
    fn vertex_heap_loads_take_their_type_from_the_first_use() {
        let program = |used: &[u32]| {
            let mut words = vec![
                0x8f00850a, // s0 = s10 << 5
                0xf4080107, 0x00000000, // s[4:7] = heap[s14:15][s0]
                0xe0002000, 0x80010100, // v1 = buffer_load_format_x(index v0, s[4:7])
                0xbf820000, // s_branch to the next instruction
            ];
            words.extend(used);
            words.push(0xbf810000);
            let mut ex = pixel(&words);
            ex.stage = Stage::Vertex;
            super::super::translate(&ex)
        };
        let out = program(&[0xd5690002, 0x0002020a]).unwrap(); // v2 = s10 * v1, integer
        assert!(out.hlsl.contains("Buffer<uint4> tb_heap_bufu_4;"));
        assert!(out.hlsl.contains("s10 = tb_index500_0.x;"));
        let out = program(&[0x0e040301]).unwrap(); // v2 = v1 * v1, float
        assert!(out.hlsl.contains("Buffer<float4> tb_heap_buf_4;"));
        // One path reads an integer, the other a float.
        let mixed = program(&[0xbf850002, 0xd5690002, 0x0002020a, 0x0e040301]);
        assert!(mixed.is_err_and(|reason| reason.contains("ambiguous")));
    }

    #[test]
    fn compute_group_ids_follow_the_last_launch_argument() {
        let mut ex = pixel(&[0xbf810000]);
        ex.stage = Stage::Compute;
        let end = |rows: &[[u32; 5]]| {
            let mut ex = ex.clone();
            ex.fssm = fssm(rows, &[]);
            user_data_end(&ex)
        };
        let table = [3, 0x40, 12, 7, 0x105];
        assert_eq!(end(&[table]), Some(6));
        assert_eq!(end(&[table, [4, 0, 0, 1, 0xf00]]), Some(6)); // static sampler
        assert_eq!(end(&[table, [1, 0, 2, 7, 0x206]]), Some(8)); // root descriptor
        assert_eq!(end(&[table, CONSTANTS, [5, 0, 1, 0, 0x0c], [6, 0, 1, 0, 0x0d]]), Some(14));
        assert_eq!(end(&[table, [9, 0, 1, 0, 0x0c]]), None); // unknown argument
        assert_eq!(end(&[]), None);
    }
}
