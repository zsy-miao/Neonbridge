//! Recover static samplers from literal RDNA2 image descriptors when the
//! serialized desktop root omits samplers declared by PSV and FSSM.
use crate::{
    Dxbc, compute_table, fssm,
    rdna2::{self, Enc, Opnd},
    xdat,
};

fn word(bytes: &[u8], at: usize) -> Option<u32> {
    Some(u32::from_le_bytes(*bytes.get(at..)?.first_chunk()?))
}

/// Return a standard D3D12_STATIC_SAMPLER_DESC as thirteen serialized dwords.
pub fn pixel(shader: &[u8]) -> Option<[u32; 13]> {
    let d = Dxbc::parse(shader).ok()?;
    let chunk = |tag| {
        let c = d.chunk(tag)?;
        shader.get(c.offset..c.offset + c.size)
    };
    let dxil = chunk(b"DXIL")?;
    if word(dxil, 0)? >> 16 != 0 {
        return None;
    }
    let psv = chunk(b"PSV0")?;
    let resources = psv.get(4usize.checked_add(word(psv, 0)? as usize)?..)?;
    let count = word(resources, 0)? as usize;
    let stride = word(resources, 4)? as usize;
    if stride < 16 || count > 128 {
        return None;
    }
    let mut samplers = resources
        .get(8..)?
        .get(..count.checked_mul(stride)?)?
        .chunks_exact(stride)
        .filter(|r| word(r, 0) == Some(1));
    let row = samplers.next()?;
    let reg = word(row, 8)?;
    let space = word(row, 4)?;
    if word(row, 12)? != reg || samplers.next().is_some() {
        return None;
    }
    let variants = xdat::programs(dxil.get(24..)?).ok()?;
    let [program] = variants.as_slice() else {
        return None;
    };
    let mut used = compute_table::records(program)?.filter(|row| row[0] == 0x401);
    if used.next()?[2] != reg || used.next().is_some() {
        return None;
    }

    // Reuse the same control-flow proof as private sampler recovery. A
    // single immutable state can retain the original PSV binding even if
    // the native program copies its words or uses a loop.
    let recovered = sites(&fssm::decode(program).ok()?.isa, 5)?;
    let [sampler] = recovered.descriptors.as_slice() else {
        return None;
    };
    let mut sampler = *sampler;
    sampler[10] = reg;
    sampler[11] = space;
    Some(sampler)
}

// Match the comparison sample/gather variants handled by the HLSL emitter.
// The sampler words themselves do not distinguish Sample from SampleCmp.
fn comparison(op: u32) -> bool {
    matches!(op, 40..=47 | 56..=63 | 72..=79 | 88..=95)
}

fn descriptor(
    words: [u32; 4],
    reg: u32,
    space: u32,
    visibility: u32,
    compare: bool,
) -> Option<[u32; 13]> {
    let [address, 0xc00000, filters, border_word] = words else {
        return None;
    };
    let (filter, aniso, func, border) = if compare {
        // Invert the exact literal-state mapping in shader-compiler's
        // sampler_heap::inline_sampler. Reject unrecognized reserved fields,
        // biases, anisotropic modes and custom border colors.
        if address & !0x71ff != 0 || border_word & 0x3fff_ffff != 0 {
            return None;
        }
        let filter = match filters {
            0x14000000 => 0x80,
            0x14500000 => 0x94,
            0x18500000 => 0x95,
            _ => return None,
        };
        let border = border_word >> 30;
        if border > 2 {
            return None;
        }
        (filter, 1, ((address >> 12) & 7) + 1, border)
    } else {
        if border_word & 0x3fff_ffff != 0 || border_word >> 30 > 2 {
            return None;
        }
        // A signed 6.8 mip bias sits below the filters; point samplers may carry an
        // anisotropy ratio the filters do not use.
        let (filter, aniso) = match (filters & !0x3fff, address >> 9) {
            (0x14000000, 0) => (0, 1),
            (0x14500000, 0) => (0x14, 1),
            (0x18500000, 0) => (0x15, 1),
            (0x18500000, 1..=4) => (0x15, 1),
            (0x18f00000, ratio @ 1..=4) => (0x55, 1 << ratio),
            _ => return None,
        };
        (filter, aniso, 1, border_word >> 30)
    };
    let axis = |shift: u32| match (address >> shift) & 7u32 {
        0 => Some(1),
        2 => Some(3),
        6 => Some(4),
        _ => None,
    };
    let bias = if compare { 0 } else { (((filters & 0x3fff) as i32) << 18 >> 18) as f32 as i32 };
    let bias = if bias == 0 { 0 } else { (bias as f32 / 256.0).to_bits() };
    Some([
        filter,
        axis(0)?,
        axis(3)?,
        axis(6)?,
        bias,
        aniso,
        func,
        border,
        0,
        12.0_f32.to_bits(),
        reg,
        space,
        visibility,
    ])
}

/// Match a literal state to a serialized D3D12 static sampler. The
/// compiler and root builder share this decoder instead of maintaining
/// independent (and potentially inconsistent) filter tables.
pub fn matches_static(words: [u32; 4], sampler: &[u32; 13], compare: bool) -> bool {
    descriptor(words, sampler[10], sampler[11], sampler[12], compare).as_ref() == Some(sampler)
}

/// Every PC in this map refers to a native MIMG instruction whose sampler is
/// materialized entirely by scalar constant operations.
pub struct InlineSamplers {
    pub descriptors: Vec<[u32; 13]>,
    pub sites: std::collections::BTreeMap<usize, u32>,
}

/// Classify literal states separately from the source registers. The native
/// compiler may fold several equal samplers to the same state, so a recovered
/// root uses private slots keyed by the descriptor words and comparison mode.
pub fn sites(isa: &[u8], visibility: u32) -> Option<InlineSamplers> {
    classify(isa, visibility, false)
}

/// As [`sites`], leaving out samplers that are not literals the classifier knows.
pub fn literal_sites(isa: &[u8], visibility: u32) -> Option<InlineSamplers> {
    classify(isa, visibility, true)
}

/// The private samplers of a program whose own root signature is unknown.
pub fn rootless(shader: &[u8], stage: u32) -> Option<Vec<[u32; 13]>> {
    let d = Dxbc::parse(shader).ok()?;
    let c = d.chunk(b"DXIL")?;
    let dxil = shader.get(c.offset..c.offset + c.size)?;
    let programs = xdat::programs(dxil.get(24..)?).ok()?;
    let [program] = programs.as_slice() else { return None };
    Some(literal_sites(&fssm::decode(program).ok()?.isa, if stage == 0 { 5 } else { 0 })?.descriptors)
}

fn classify(isa: &[u8], visibility: u32, partial: bool) -> Option<InlineSamplers> {
    use std::collections::{BTreeMap, BTreeSet};
    let ops = rdna2::disassemble(isa);
    if ops.len() > 32768 || !matches!(visibility, 0 | 5) {
        return None;
    }
    let jump = |op: &rdna2::Insn| matches!(op.mnemonic.as_str(), "s_setpc_b64" | "s_swappc_b64");
    let mut successors = vec![[None; 2]; ops.len()];
    for (i, op) in ops.iter().enumerate() {
        let name = op.mnemonic.as_str();
        if name.starts_with("s_movrel")
            || name.starts_with("s_subvector_loop")
            || matches!(name, "s_call_b64" | "s_rfe_b64" | "s_trap")
        {
            return None;
        }
        // The target is unknown. Nothing flows on to the next instruction, and an
        // instruction that only such a jump can reach starts with unknown registers.
        if jump(op) {
            continue;
        }
        if name.contains("branch") {
            if op.enc != Enc::Sopp || !matches!(op.op, 2 | 4..=9) {
                return None;
            }
            let to = (op.offset as i64)
                .checked_add(4)?
                .checked_add(op.aux.imm.checked_mul(4)?)?;
            let target = ops
                .binary_search_by_key(&usize::try_from(to).ok()?, |i| i.offset)
                .ok()?;
            successors[i][0] = Some(target);
        }
        if i + 1 < ops.len() && name != "s_branch" && name != "s_endpgm" {
            successors[i][1] = Some(i + 1);
        }
    }
    // A loop header can follow an unconditional branch and alignment NOPs.
    // Such padding has no path from entry and cannot define a sampler word.
    let mut incoming = vec![Vec::new(); ops.len()];
    let mut reachable = vec![false; ops.len()];
    let mut work = vec![0];
    let mut entries = BTreeSet::new();
    if ops.iter().any(jump) {
        // Jump targets are code that follows an unconditional transfer.
        let mut open = false;
        for (i, op) in ops.iter().enumerate() {
            if open && op.mnemonic != "s_nop" {
                entries.insert(i);
                open = false;
            }
            open |= jump(op) || matches!(op.mnemonic.as_str(), "s_branch" | "s_endpgm");
        }
        work.extend(&entries);
    }
    while let Some(pc) = work.pop() {
        if std::mem::replace(reachable.get_mut(pc)?, true) {
            continue;
        }
        for target in successors[pc].into_iter().flatten() {
            incoming[target].push(pc);
            work.push(target);
        }
    }
    // A wave64 mask occupies an even-aligned pair, so an odd VOP3 comparison
    // destination proves one-word masks throughout the program.
    let one_word = ops.iter().any(|op| op.enc == Enc::Vop3 && op.mnemonic.starts_with("v_cmp_")
        && matches!(op.dst[..], [Opnd::Sgpr(n)] if n & 1 != 0));
    // Accept a literal only if every reaching definition supplies exactly
    // that value, including loop backedges and moves between SGPRs.
    let reaching = |pc: usize, reg: u8| -> Option<u32> {
        if pc == 0 || entries.contains(&pc) {
            return None; // No reaching definition at program entry.
        }
        let mut literal = None;
        let mut seen = BTreeSet::new();
        let mut pending: Vec<_> = incoming[pc].iter().map(|&i| (i, reg)).collect();
        while let Some((i, mut r)) = pending.pop() {
            if !seen.insert((i, r)) {
                continue;
            }
            let op = &ops[i];
            // One-word compare masks leave the next register alone.
            let mask = one_word && op.enc == Enc::Vop3 && op.mnemonic.starts_with("v_cmp") && op.dst != [Opnd::Sgpr(r)];
            if rdna2::clobbers_scalar(op, r) && !mask {
                let value = if op.mnemonic == "s_mov_b64" && op.enc == Enc::Sop1 {
                    // Resolve the requested half of an ordinary SGPR pair
                    // copy using pre-instruction definitions, including overlap.
                    let [Opnd::Sgpr(dst)] = op.dst[..] else { return None };
                    let [Opnd::Sgpr(src)] = op.src[..] else { return None };
                    if dst > 104 || src > 104 || dst % 2 != 0 || src % 2 != 0 {
                        return None;
                    }
                    let half = r.checked_sub(dst)?;
                    if half > 1 { return None; }
                    r = src.checked_add(half)?;
                    None
                } else {
                    if op.dst != [Opnd::Sgpr(r)] {
                        return None;
                    }
                    match (op.mnemonic.as_str(), op.src.as_slice()) {
                        ("s_movk_i32", []) => Some(op.aux.imm as i16 as i32 as u32),
                        ("s_mov_b32", [Opnd::ConstInt(v)]) => Some(*v as u32),
                        ("s_mov_b32", [Opnd::Literal(v)]) => Some(*v),
                        ("s_brev_b32", [Opnd::ConstInt(v)]) => Some((*v as u32).reverse_bits()),
                        ("s_brev_b32", [Opnd::Literal(v)]) => Some(v.reverse_bits()),
                        ("s_mov_b32", [Opnd::Sgpr(src)]) => {
                            r = *src;
                            None
                        }
                        _ => return None,
                    }
                };
                if let Some(value) = value {
                    if literal.replace(value).is_some_and(|last| last != value) {
                        return None;
                    }
                    continue;
                }
            }
            if i == 0 || incoming[i].is_empty() || entries.contains(&i) {
                return None;
            }
            pending.extend(incoming[i].iter().map(|&pred| (pred, r)));
        }
        literal
    };
    let mut states = BTreeMap::new();
    for (i, op) in ops.iter().enumerate().filter(|(_, op)| {
        op.enc == Enc::Mimg
            && (op.mnemonic.starts_with("image_sample")
                || op.mnemonic.starts_with("image_gather")
                || op.mnemonic == "image_get_lod")
    }) {
        let Opnd::Sgpr(base) = *op.src.last()? else {
            return None;
        };
        let mut words = [0; 4];
        let literal = words.iter_mut().enumerate().all(|(n, word)| {
            let value = base.checked_add(n as u8).and_then(|reg| reaching(i, reg));
            if let Some(v) = value { *word = v; }
            value.is_some()
        });
        // A sampler loaded from a descriptor keeps the binding it came from.
        let compare = comparison(op.op);
        if !literal || descriptor(words, 0, 32765, visibility, compare).is_none() {
            if !partial { return None; }
            continue;
        }
        states.insert(op.offset, (words, compare));
    }
    let unique: BTreeSet<_> = states.values().copied().collect();
    if unique.is_empty() || unique.len() > 128 {
        return None;
    }
    let mut descriptors = Vec::new();
    let mut slots = BTreeMap::new();
    for (words, compare) in unique {
        let slot = descriptors.len() as u32;
        descriptors.push(descriptor(words, slot, 32765, visibility, compare)?);
        slots.insert((words, compare), slot);
    }
    Some(InlineSamplers {
        descriptors,
        sites: states
            .into_iter()
            .map(|(pc, words)| (pc, slots[&words]))
            .collect(),
    })
}

/// Validate original static sampler declarations in both PSV and FSSM before
/// synthesizing omitted pixel or compute static samplers. Dynamic heap samplers require
/// the guest's actual sampler heap.
pub fn private(shader: &[u8], stage: u32) -> Option<Vec<[u32; 13]>> {
    let d = Dxbc::parse(shader).ok()?;
    let chunk = |tag| {
        let c = d.chunk(tag)?;
        shader.get(c.offset..c.offset + c.size)
    };
    let dxil = chunk(b"DXIL")?;
    if !matches!(stage, 0 | 5)
        || word(dxil, 0)? >> 16 != stage
        || (stage != 5 && u64::from_le_bytes(chunk(b"SFI0")?.try_into().ok()?) & (3 << 25) != 0)
    {
        return None;
    }
    let psv = chunk(b"PSV0")?;
    let resources = psv.get(4usize.checked_add(word(psv, 0)? as usize)?..)?;
    let count = word(resources, 0)? as usize;
    let stride = word(resources, 4)? as usize;
    if count > 128 || stride < 16 {
        return None;
    }
    let mut names = std::collections::BTreeSet::new();
    for r in resources
        .get(8..)?
        .get(..count.checked_mul(stride)?)?
        .chunks_exact(stride)
    {
        if word(r, 0)? == 1
            && (word(r, 8)? != word(r, 12)? || word(r, 4)? == 32765 || !names.insert(word(r, 8)?))
        {
            return None;
        }
    }
    if names.is_empty() {
        return None;
    }
    let programs = xdat::programs(dxil.get(24..)?).ok()?;
    let [program] = programs.as_slice() else {
        return None;
    };
    for row in compute_table::records(program)?.filter(|r| r[0] == 0x401) {
        if !names.remove(&row[2]) {
            return None;
        }
    }
    if !names.is_empty() {
        return None;
    }
    Some(
        sites(
            &fssm::decode(program).ok()?.isa,
            if stage == 0 { 5 } else { 0 },
        )?
        .descriptors,
    )
}

#[cfg(test)]
#[path = "tests/inline_sampler.rs"]
mod tests;
