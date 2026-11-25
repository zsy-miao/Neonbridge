//! Where a program finds its user data and descriptor heaps.
use super::*;

/// User SGPR of a single FSSM user-data record of `class` (5: resource heap base,
/// 6: sampler heap base).
pub(in crate::hlsl) fn compute_user_sgpr(ex: &ShaderExtract, class: u32) -> Option<u8> {
    let mut rows = tuxbox_dxbc::compute_table::user_data_records(&ex.fssm)?.filter(|&[c, ..]| c == class);
    let [_, _, _, _, loc] = rows.next()?;
    if rows.next().is_some() {
        return None;
    }
    u8::try_from(loc & 0xff).ok()
}

/// The sampler heap base of a mesh program, from its one FSSM class-6 row.
pub(in crate::hlsl) fn mesh_sampler_heap_base(ex: &ShaderExtract) -> Option<u8> {
    if ex.stage != Stage::Mesh || ex.root_sig.as_ref()?.flags & 0x800 == 0 {
        return None;
    }
    compute_user_sgpr(ex, 6)?.checked_add(8)
}

/// Whether an instruction uses SCC.
pub(in crate::hlsl) fn reads_scc(ins: &Insn) -> bool {
    ins.src.contains(&Opnd::Scc)
        || matches!(ins.mnemonic.as_str(), "s_cbranch_scc0" | "s_cbranch_scc1"
            | "s_cselect_b32" | "s_cselect_b64" | "s_addc_u32" | "s_subb_u32"
            | "s_cmov_b32" | "s_cmov_b64" | "s_cmovk_i32")
}

/// Whether the SCC that the instruction at `at` uses may come from a lane mask. A scalar
/// comparison or sum gives every lane of the wave the same SCC.
pub(in crate::hlsl) fn scc_from_mask(code: &[Insn], at: usize) -> bool {
    for (i, op) in code[..at].iter().enumerate().rev() {
        let m = op.mnemonic.as_str();
        if m.starts_with("s_cmp_") || m.starts_with("s_cmpk_") || m.starts_with("s_bitcmp") {
            return op.src.iter().chain(&op.dst).any(|o| matches!(o, Opnd::ExecLo | Opnd::ExecHi | Opnd::VccLo | Opnd::VccHi))
                || m.ends_with("_u64");
        }
        let scalar_alu = matches!(op.enc, Enc::Sop1 | Enc::Sop2 | Enc::Sopk);
        if scalar_alu && !matches!(m, "s_mov_b32" | "s_mov_b64" | "s_movk_i32" | "s_cselect_b32" | "s_cselect_b64" | "s_mul_i32" | "s_mul_hi_u32" | "s_mul_hi_i32" | "s_brev_b32" | "s_bitset1_b32" | "s_bitset0_b32" | "s_pack_ll_b32_b16" | "s_bfm_b32" | "s_cmov_b32" | "s_cmov_b64") {
            return true;
        }
        // Another path may arrive here with its own SCC.
        let here = code[i + 1].offset as i64;
        if code.iter().any(|b| b.enc == Enc::Sopp && Emitter::branch_cond(b.op).is_some() && b.offset as i64 + 4 + b.aux.imm * 4 == here) {
            return true;
        }
        if op.enc == Enc::Sopp && Emitter::branch_cond(op.op).is_some() {
            return true;
        }
    }
    true
}

/// Mesh shaders index the resource descriptor heap through one FSSM class-5
/// dword. User data sits above the eight system SGPRs, so the hardware register
/// is `(location & 0xff) + 8`. The row is the shape captured on every mesh
/// launch that failed only on this register: class 5, offset 0, width 1.
pub(in crate::hlsl) fn mesh_heap_base(ex: &ShaderExtract) -> Option<u8> {
    if ex.stage != Stage::Mesh || ex.required_features? & (1 << 25) == 0 {
        return None;
    }
    let root = ex.root_sig.as_ref()?;
    // Root flag 0x400 is RESOURCE_HEAP_DIRECTLY_INDEXED.
    if root.flags & 0x400 == 0 {
        return None;
    }
    let mut rows = tuxbox_dxbc::compute_table::user_data_records(&ex.fssm)?
        .filter(|&[class, ..]| class == 5);
    let Some([5, 0, 1, 0, loc]) = rows.next() else {
        return None;
    };
    if rows.next().is_some() {
        return None;
    }
    u8::try_from(loc & 0xff).ok()?.checked_add(8)
}

/// Whether dwords `1..n` of the heap descriptor at `base` are still the loaded ones.
pub(in crate::hlsl) fn heap_words(sym: &HashMap<u16, Sym>, base: u16, n: u16) -> bool {
    let origin = match sym.get(&base) {
        Some(Sym::HeapDescriptor(_, origin) | Sym::HeapSamplerIndex(origin)) => *origin,
        _ => return false,
    };
    (1..n).all(|k| sym.get(&(base + k)) == Some(&Sym::HeapWord(k as u8, origin)))
}

/// A complete `n`-dword resource descriptor fetched at a run-time heap index.
pub(in crate::hlsl) fn heap_descriptor(sym: &HashMap<u16, Sym>, base: u16, n: u16) -> bool {
    matches!(sym.get(&base), Some(Sym::HeapDescriptor(words, _)) if *words == u32::from(n))
        && heap_words(sym, base, n)
}

/// Pixel shaders receive descriptor heap bases like compute: the user SGPR
/// named by one FSSM user-data row (class 5 the resource heap, 6 the sampler heap),
/// when SFI0 declares that heap directly indexed (bits 25 and 26). Rooted pixel
/// shaders also allow class 5 when `RESOURCE_HEAP_DIRECTLY_INDEXED` (0x400) is set
/// and `sampler_heap::resource_base` does not already claim the root layout.
pub(in crate::hlsl) fn pixel_heap_base(ex: &ShaderExtract, class: u32) -> Option<u8> {
    if ex.stage != Stage::Pixel || ex.required_features? & (1 << (20 + class)) == 0 {
        return None;
    }
    if let Some(root) = ex.root_sig.as_ref() {
        // Class 5 needs CBV_SRV_UAV_HEAP_DIRECTLY_INDEXED (0x400), class 6
        // SAMPLER_HEAP_DIRECTLY_INDEXED (0x800).
        let indexed = match class { 5 => root.flags & 0x400 != 0, 6 => root.flags & 0x800 != 0, _ => false };
        if !indexed || sampler_heap::resource_base(ex).is_some() {
            return None;
        }
    }
    let mut rows = tuxbox_dxbc::compute_table::user_data_records(&ex.fssm)?.filter(|&[c, ..]| c == class);
    let [_, 0, 1, 0, loc @ 0..=0xff] = rows.next()? else { return None };
    if rows.next().is_some() {
        return None;
    }
    u8::try_from(loc).ok()
}

/// Compute and vertex shaders that index the resource descriptor heap receive
/// its base in the user SGPR named by FSSM user-data class 5 (class 6 is the
/// sampler heap). RAGE's skinning and VfxVBuffer vertex shaders fetch
/// `ResourceDescriptorHeap[root constant]` from s18 this way, behind three
/// descriptor tables, a root SRV and five root constants: the same form as
/// compute, rather than the pixel layout `sampler_heap::resource_base` checks.
pub(in crate::hlsl) fn compute_heap_base(ex: &ShaderExtract) -> Option<u8> {
    if !matches!(ex.stage, Stage::Compute | Stage::Vertex) || ex.required_features? & (1 << 25) == 0 {
        return None;
    }
    if ex.root_sig.as_ref().is_some_and(|root| root.flags & 0x400 == 0) {
        return None;
    }
    let mut rows = tuxbox_dxbc::compute_table::user_data_records(&ex.fssm)?
        .filter(|&[class, ..]| class == 5);
    let [_, _, _, _, loc] = rows.next()?;
    if rows.next().is_some() {
        return None;
    }
    u8::try_from(loc & 0xff).ok()
}
