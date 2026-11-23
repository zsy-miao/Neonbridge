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

    let code = fssm::decode(program).ok()?;
    let ops = rdna2::disassemble(&code.isa);
    let mut branches = Vec::new();
    for op in &ops {
        if op.mnemonic == "s_branch" || op.mnemonic.starts_with("s_cbranch_") {
            let to = (op.offset + op.dwords * 4)
                .checked_add_signed(isize::try_from(op.aux.imm.checked_mul(4)?).ok()?)?;
            if op.enc != Enc::Sopp || to <= op.offset {
                return None;
            }
            branches.push((op.offset, to));
        } else if op.mnemonic.starts_with("s_movrel")
            || op.mnemonic.starts_with("s_subvector_loop")
            || matches!(
                op.mnemonic.as_str(),
                "s_setpc_b64" | "s_swappc_b64" | "s_call_b64" | "s_rfe_b64" | "s_trap"
            )
        {
            return None;
        }
    }
    let mut sampler = None;
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
        for (j, dst) in words.iter_mut().enumerate() {
            let r = base.checked_add(j as u8)?;
            let load = ops[..i]
                .iter()
                .rev()
                .find(|x| rdna2::clobbers_scalar(x, r))?;
            if load.dst != [Opnd::Sgpr(r)]
                || branches
                    .iter()
                    .any(|&(from, to)| from < load.offset && to > load.offset && to <= op.offset)
            {
                return None;
            }
            *dst = match (load.mnemonic.as_str(), load.src.as_slice()) {
                ("s_movk_i32", []) => load.aux.imm as i16 as i32 as u32,
                ("s_mov_b32", [Opnd::ConstInt(v)]) => *v as u32,
                ("s_mov_b32", [Opnd::Literal(v)]) => *v,
                ("s_brev_b32", [Opnd::ConstInt(v)]) => (*v as u32).reverse_bits(),
                ("s_brev_b32", [Opnd::Literal(v)]) => v.reverse_bits(),
                _ => return None,
            };
        }
        if sampler.replace(words).is_some_and(|prior| prior != words) {
            return None;
        }
    }
    descriptor(sampler?, reg, space, 5)
}

fn descriptor(words: [u32; 4], reg: u32, space: u32, visibility: u32) -> Option<[u32; 13]> {
    let [address, 0xc00000, filters, 0] = words else {
        return None;
    };
    let (filter, aniso) = match (filters, address >> 9) {
        (0x14000000, 0) => (0, 1),
        (0x14500000, 0) => (0x14, 1),
        (0x18500000, 0) => (0x15, 1),
        (0x18f00000, 2) => (0x55, 4),
        _ => return None,
    };
    let axis = |shift: u32| match (address >> shift) & 7u32 {
        0 => Some(1),
        2 => Some(3),
        6 => Some(4),
        _ => None,
    };
    Some([
        filter,
        axis(0)?,
        axis(3)?,
        axis(6)?,
        0,
        aniso,
        1,
        0,
        0,
        12.0_f32.to_bits(),
        reg,
        space,
        visibility,
    ])
}

/// Every PC in this map refers to a native MIMG instruction whose sampler is
/// materialized entirely by scalar constant operations.
pub struct InlineSamplers {
    pub descriptors: Vec<[u32; 13]>,
    pub sites: std::collections::BTreeMap<usize, u32>,
}

/// Classify literal states separately from the source registers. The native
/// compiler may fold several equal samplers to the same state, so a recovered
/// root uses private slots whose identity comes from the four descriptor words.
pub fn sites(isa: &[u8], visibility: u32) -> Option<InlineSamplers> {
    use std::collections::{BTreeMap, BTreeSet};
    let ops = rdna2::disassemble(isa);
    if ops.len() > 32768 || !matches!(visibility, 0 | 5) {
        return None;
    }
    let mut successors = vec![[None; 2]; ops.len()];
    for (i, op) in ops.iter().enumerate() {
        let name = op.mnemonic.as_str();
        if name.starts_with("s_movrel")
            || name.starts_with("s_subvector_loop")
            || matches!(
                name,
                "s_setpc_b64" | "s_swappc_b64" | "s_call_b64" | "s_rfe_b64" | "s_trap"
            )
        {
            return None;
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
    while let Some(pc) = work.pop() {
        if std::mem::replace(reachable.get_mut(pc)?, true) {
            continue;
        }
        for target in successors[pc].into_iter().flatten() {
            incoming[target].push(pc);
            work.push(target);
        }
    }
    // Accept a literal only if every reaching definition supplies exactly
    // that value, including loop backedges and moves between SGPRs.
    let reaching = |pc: usize, reg: u8| -> Option<u32> {
        let mut literal = None;
        let mut seen = BTreeSet::new();
        let mut pending: Vec<_> = incoming[pc].iter().map(|&i| (i, reg)).collect();
        while let Some((i, mut r)) = pending.pop() {
            if !seen.insert((i, r)) {
                continue;
            }
            let op = &ops[i];
            if rdna2::clobbers_scalar(op, r) {
                if op.dst != [Opnd::Sgpr(r)] {
                    return None;
                }
                let value = match (op.mnemonic.as_str(), op.src.as_slice()) {
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
                };
                if let Some(value) = value {
                    if literal.replace(value).is_some_and(|last| last != value) {
                        return None;
                    }
                    continue;
                }
            }
            if i == 0 || incoming[i].is_empty() {
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
        for (n, word) in words.iter_mut().enumerate() {
            *word = reaching(i, base.checked_add(n as u8)?)?;
        }
        states.insert(op.offset, words);
    }
    let unique: BTreeSet<_> = states.values().copied().collect();
    if unique.is_empty() || unique.len() > 128 {
        return None;
    }
    let mut descriptors = Vec::new();
    let mut slots = BTreeMap::new();
    for words in unique {
        let slot = descriptors.len() as u32;
        descriptors.push(descriptor(words, slot, 32765, visibility)?);
        slots.insert(words, slot);
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
        || u64::from_le_bytes(chunk(b"SFI0")?.try_into().ok()?) & (3 << 25) != 0
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
mod tests {
    #[test]
    fn sampler_words_must_agree_on_every_reachable_loop_path() {
        // s[8:11] describe linear clamped sampling. The preheader skips a
        // conflicting definition of s10 immediately before the loop header.
        let mut words = [
            0xb008_0092u32, // s_movk_i32 s8, 0x92
            0xbe89_03ff,
            0x00c0_0000, // s_mov_b32 s9, max LOD 12
            0xbe8a_03ff,
            0x1450_0000, // s_mov_b32 s10, linear filtering
            0xbe8b_0380, // s_mov_b32 s11, 0
            0xbf82_0002, // s_branch sample
            0xbe8a_03ff,
            0x1400_0000, // unreachable point filtering
            0xf080_0f08,
            0x0040_0a0a, // sample: image_sample ... s[8:11]
            0xbf85_fffd, // s_cbranch_scc1 sample
            0xbf81_0000, // s_endpgm
        ];
        let classify = |code: &[u32]| {
            let isa: Vec<_> = code.iter().flat_map(|x| x.to_le_bytes()).collect();
            super::sites(&isa, 5)
        };
        let sample = classify(&words).unwrap();
        assert_eq!(sample.descriptors.len(), 1);
        assert_eq!(sample.descriptors[0][0], 0x14);
        assert_eq!(sample.descriptors[0][12], 5);
        assert_eq!(sample.sites.get(&36), Some(&0));
        words[6] = 0xbf84_0002; // s_cbranch_scc0 also allows the point filter.
        assert!(classify(&words).is_none());
    }
}
