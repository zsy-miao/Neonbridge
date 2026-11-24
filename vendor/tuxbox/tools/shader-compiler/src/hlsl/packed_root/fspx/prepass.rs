//! Register provenance for FSPX prepass and sharpening shaders. Descriptor contents
//! never enter shader arithmetic except for the compiler's raw-view conversion.
use super::*;
use crate::meta::RootRange;

#[derive(Clone, Copy, PartialEq, Eq, Debug)]
enum Scalar {
    Unknown,
    Data,
    Literal(u32),
    Table(bool),
    Cbv(u32, usize),
    Image(Binding, usize),
    Buffer(Binding, usize),
    Stride(Binding),
    NonzeroStride(Binding),
    ByteCount(Binding),
    NoStride(Binding),
    ClearedOob(Binding),
    RawOob(Binding),
}

impl Scalar {
    fn data(self) -> bool {
        matches!(self, Self::Data | Self::Literal(_))
    }
}

type Registers = [Scalar; 106];

fn operand(regs: &Registers, o: Opnd) -> Scalar {
    match o {
        Opnd::Sgpr(n) => regs.get(n as usize).copied().unwrap_or(Scalar::Unknown),
        Opnd::ConstInt(v) => Scalar::Literal(v as u32),
        Opnd::Literal(v) => Scalar::Literal(v),
        _ => Scalar::Data,
    }
}

fn sg(o: Opnd) -> Result<usize, String> {
    if let Opnd::Sgpr(n @ 0..=105) = o {
        Ok(n as usize)
    } else {
        Err("expected user SGPR".into())
    }
}

fn has(ex: &ShaderExtract, ty: u32, slot: u32) -> bool {
    ex.resources
        .iter()
        .any(|v| v.space == 0 && v.res_type == ty && (v.lower..=v.upper).contains(&slot))
}

fn raw_view(words: &[Scalar], b: Binding) -> bool {
    use Scalar::*;
    words == [Buffer(b, 0), NoStride(b), ByteCount(b), RawOob(b)]
}

fn table_register(ranges: &[RootRange], index: u32) -> Result<u32, String> {
    let mut next = 0;
    let mut slot = None;
    for range in ranges {
        let start = if range.offset_in_table == u32::MAX {
            next
        } else {
            range.offset_in_table
        };
        next = start
            .checked_add(range.num)
            .ok_or("descriptor range overflow")?;
        if (start..next).contains(&index) {
            if slot.is_some() {
                return Err("overlapping descriptor ranges".into());
            }
            slot = Some(
                range
                    .base_register
                    .checked_add(index - start)
                    .ok_or("descriptor register overflow")?,
            );
        }
    }
    slot.ok_or_else(|| "descriptor outside root range".into())
}

fn smem(
    ex: &ShaderExtract,
    ranges: [&[RootRange]; 2],
    r: &mut Registers,
    plan: &mut PackedRoot,
    idx: usize,
    i: &Insn,
) -> Result<(), String> {
    use Scalar::*;
    let d = sg(i.dst[0])?;
    let a = sg(i.src[0])?;
    let [op, offset] = i.raw[..] else {
        return Err("invalid SMEM length".into());
    };
    if offset & 0xffe0_0003 != 0xfa00_0000 || op & 0x0003_e000 != 0 {
        return Err("unsupported SMEM addressing".into());
    }
    let off = offset & 0x1f_ffff;
    let n = 1usize << (i.op & 7);
    if d + n > r.len() || a + 4 > r.len() {
        return Err("SMEM register span".into());
    }
    if i.op <= 4 {
        let Table(uav) = r[a] else {
            return Err("unresolved descriptor table".into());
        };
        if r[a + 1] != Literal(0) || off % 32 != 0 {
            return Err("invalid descriptor table address".into());
        }
        for k in 0..n {
            let index = off / 32 + (k / 8) as u32;
            let slot = table_register(ranges[usize::from(uav)], index)?;
            let b = Binding { slot, uav };
            r[d + k] = if n >= 8 && has(ex, if uav { 6 } else { 3 }, slot) {
                Image(b, k % 8)
            } else if n == 4 && has(ex, if uav { 7 } else { 4 }, slot) {
                Buffer(b, k)
            } else {
                return Err("descriptor width or type disagrees with PSV".into());
            };
        }
        plan.loads.insert(idx, vec![None; n]);
    } else if (8..=12).contains(&i.op) {
        match r[a] {
            Cbv(slot, 0)
                if r[a + 1..a + 4] == [Cbv(slot, 1), Literal(u32::MAX), Literal(0x31016fac)]
                    && has(ex, 2, slot)
                    && off / 4 + n as u32 <= 16384 =>
            {
                plan.loads.insert(
                    idx,
                    (0..n).map(|k| Some((slot, off / 4 + k as u32))).collect(),
                );
            }
            Buffer(b, 0) if !b.uav && (0..4).all(|k| r[a + k] == Buffer(b, k)) => {
                plan.raw_scalar.insert(idx, (b, off, n as u8));
            }
            _ => return Err("unresolved scalar buffer view".into()),
        }
        r[d..d + n].fill(Data);
    } else {
        return Err("unsupported FSPX scalar memory opcode".into());
    }
    Ok(())
}

fn scalar_op(i: &Insn, r: &mut Registers) -> Result<(), String> {
    use Scalar::*;
    let args: Vec<_> = i.src.iter().map(|&o| operand(r, o)).collect();
    let d = i.dst.first().and_then(|&o| sg(o).ok());
    let value = match (i.mnemonic.as_str(), args.as_slice()) {
        ("s_mov_b32", [v]) => *v,
        ("s_movk_i32", []) => Literal(i.aux.imm as i32 as u32),
        ("s_bfm_b32", [Literal(n), Literal(shift)]) => {
            Literal(((1u32 << (n & 31)) - 1) << (shift & 31))
        }
        ("s_mov_b64", [lo @ (Literal(_) | Cbv(_, 0))]) => {
            let d = d
                .filter(|d| d + 1 < r.len())
                .ok_or("invalid literal pair")?;
            r[d + 1] = operand(r, hi(&i.src[0]));
            *lo
        }
        ("s_bfe_u32", [Buffer(b, 1), Literal(0xe0010)]) => Stride(*b),
        ("s_max_u32", [Stride(b), Literal(1)]) => NonzeroStride(*b),
        ("s_mul_i32", [Buffer(b, 2), NonzeroStride(c)]) if b == c => ByteCount(*b),
        ("s_and_b32", [Buffer(b, 1), Literal(0xc000ffff)]) => NoStride(*b),
        ("s_and_b32", [Buffer(b, 3), Literal(0xcfffffff)]) => ClearedOob(*b),
        ("s_or_b32", [ClearedOob(b), Literal(0x30000000)]) => RawOob(*b),
        ("s_bitset1_b32", _) if !d.is_some_and(|d| r[d].data()) => {
            return Err("bit set on opaque register".into())
        }
        (
            "s_add_u32" | "s_and_b32" | "s_and_saveexec_b32" | "s_andn2_b32" | "s_bfe_u32"
            | "s_bfm_b32" | "s_bitset1_b32" | "s_cmp_eq_i32" | "s_cmp_lg_i32" | "s_cselect_b32"
            | "s_lshl_b32" | "s_max_u32" | "s_mul_i32" | "s_nor_b32" | "s_or_b32",
            _,
        ) if args.iter().all(|v| v.data()) => Data,
        _ => return Err("opaque scalar used as arithmetic data".into()),
    };
    if let Some(d) = d {
        r[d] = value;
    } else if !value.data() {
        return Err("opaque word written to a special register".into());
    }
    Ok(())
}

fn image(
    ex: &ShaderExtract,
    r: &Registers,
    i: &Insn,
) -> Result<(Binding, u32, Option<u32>), String> {
    use Scalar::*;
    let [.., resource, sampler] = i.src[..] else {
        return Err("missing image operands".into());
    };
    let a = sg(resource)?;
    let Some(Image(b, 0)) = r.get(a).copied() else {
        return Err("missing image descriptor".into());
    };
    if a + 8 > r.len() || !(0..8).all(|k| r[a + k] == Image(b, k)) || i.aux.imm != 1 {
        return Err("incomplete 2D image descriptor".into());
    }
    let samp = match (i.mnemonic.as_str(), b.uav) {
        ("image_load", false) | ("image_store", true) => None,
        ("image_sample_lz", false) => {
            let a = sg(sampler)?;
            // The compiler inlines this linear clamp descriptor. Check the
            // runtime root sampler including register, space and visibility.
            if r.get(a..a + 4)
                != Some(
                    &[
                        Literal(0x3092),
                        Literal(0xfff000),
                        Literal(0x18500000),
                        Literal(0),
                    ][..],
                )
                || !has(ex, 1, 0)
                || ex
                    .root_sig
                    .as_ref()
                    .ok_or("missing root signature")?
                    .static_samplers
                    .as_slice()
                    != [[0x15, 3, 3, 3, 0, 16, 4, 2, 0, 0x7f7fffff, 0, 0, 0]]
            {
                return Err("unsupported static sampler".into());
            }
            Some(0)
        }
        _ => return Err("unsupported prepass image operation".into()),
    };
    Ok((b, i.aux.imm as u32, samp))
}

pub(super) fn bindings(
    ex: &ShaderExtract,
    insns: &[Insn],
    uav: &RootRange,
    srvs: &[RootRange],
    cbv_count: usize,
) -> Result<PackedRoot, String> {
    use Scalar::*;
    let mut r = [Unknown; 106];
    r[4] = Table(true);
    r[5] = Table(false);
    for slot in 0..cbv_count {
        for k in 0..2 {
            r[6 + 2 * slot + k] = Cbv(slot as u32, k);
        }
    }
    let group = 6 + 2 * cbv_count;
    r[group..group + 2].fill(Data);
    let mut plan = PackedRoot {
        abi: RootAbi::RegisterWave32,
        ..Default::default()
    };
    let mut joins = BTreeMap::<usize, Registers>::new();
    let merge = |dst: &mut Registers, src: &Registers| {
        for (d, s) in dst.iter_mut().zip(src) {
            if *d != *s {
                *d = if d.data() && s.data() { Data } else { Unknown };
            }
        }
    };
    let mut cmp_scc = false;
    for (idx, i) in insns.iter().enumerate() {
        if let Some(other) = joins.remove(&i.offset) {
            merge(&mut r, &other);
            cmp_scc = false;
        }
        let result = (|| -> Result<(), String> {
            match i.enc {
                Enc::Sopp if i.mnemonic == "s_cbranch_execz" => {
                    let t = i.offset as i64 + 4 + 4 * i.aux.imm;
                    if t <= i.offset as i64 {
                        return Err("backward branch".into());
                    }
                    joins
                        .entry(t as usize)
                        .and_modify(|v| merge(v, &r))
                        .or_insert(r);
                }
                Enc::Sopp if i.mnemonic == "s_barrier" => {
                    if joins.keys().any(|&t| {
                        !insns
                            .last()
                            .is_some_and(|end| end.offset == t && end.mnemonic == "s_endpgm")
                    }) {
                        return Err("group barrier bypassed by a non-retiring branch".into());
                    }
                }
                Enc::Sopp if i.mnemonic == "s_endpgm" && idx + 1 == insns.len() => {}
                Enc::Sopp
                    if matches!(
                        i.mnemonic.as_str(),
                        "s_waitcnt" | "s_inst_prefetch" | "s_clause"
                    ) => {}
                Enc::Smem => smem(
                    ex,
                    [srvs, std::slice::from_ref(uav)],
                    &mut r,
                    &mut plan,
                    idx,
                    i,
                )?,
                Enc::Mubuf => {
                    let a = sg(i.src[1])?;
                    let Some(Buffer(b, 0)) = r.get(a).copied() else {
                        return Err("unresolved vector buffer".into());
                    };
                    if a + 4 > r.len()
                        || !raw_view(&r[a..a + 4], b)
                        || i.aux.mask != 1
                        || i.src[2] != Opnd::ConstInt(0)
                        || !matches!(
                            (i.mnemonic.as_str(), b.uav),
                            ("buffer_load_dwordx4", false) | ("buffer_store_dword", true)
                        )
                    {
                        return Err("unverified raw buffer access".into());
                    }
                    plan.buffers.insert(idx, (b, 0));
                }
                Enc::Mimg => {
                    plan.images.insert(idx, image(ex, &r, i)?);
                }
                Enc::Sop1 | Enc::Sop2 | Enc::Sopk | Enc::Sopc => {
                    if i.mnemonic == "s_cselect_b32" && !cmp_scc {
                        return Err("unresolved SCC".into());
                    }
                    scalar_op(i, &mut r)?;
                    if i.mnemonic != "s_cselect_b32" {
                        cmp_scc = i.enc == Enc::Sopc;
                    }
                }
                Enc::Vopc | Enc::Vop1 | Enc::Vop2 | Enc::Vop3 | Enc::Ds => {
                    if i.mnemonic == "v_nop" {
                        return Ok(());
                    }
                    // A 64-bit scalar source consumes the next SGPR too. The
                    // prepass only uses this shift as a literal or VGPR-pair move.
                    if i.mnemonic == "v_lshrrev_b64"
                        && !matches!(i.src[1], Opnd::Vgpr(_) | Opnd::ConstInt(0))
                    {
                        return Err("unverified 64-bit source".into());
                    }
                    if i.enc == Enc::Vop3
                        && matches!(i.dst.first(), Some(Opnd::Sgpr(_)))
                        && i.op >= 0x100
                    {
                        return Err("unverified vector-to-scalar transfer".into());
                    }
                    let inputs = if i.enc == Enc::Vop3
                        && (i.op < 0x100
                            || matches!(
                                i.mnemonic.as_str(),
                                "v_mul_f32"
                                    | "v_pk_mul_f16"
                                    | "v_lshrrev_b64"
                                    | "v_pack_b32_f16"
                                    | "v_pk_add_u16"
                                    | "v_pk_sub_u16"
                                    | "v_pk_add_f16"
                                    | "v_pk_min_f16"
                                    | "v_pk_max_f16"
                            )) {
                        &i.src[..2]
                    } else {
                        &i.src
                    };
                    if !inputs.iter().all(|&o| operand(&r, o).data()) {
                        return Err("opaque vector input".into());
                    }
                    if let Some(d) = i.dst.first().and_then(|&o| sg(o).ok()) {
                        r[d] = Data;
                    }
                }
                _ => return Err("unsupported prepass instruction".into()),
            }
            Ok(())
        })();
        result.map_err(|e| format!("FSPX {} at {:#x}: {e}", i.mnemonic, i.offset))?;
    }
    if !joins.is_empty() {
        return Err("invalid forward branch target".into());
    }
    Ok(plan)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn maps_descriptor_indices_across_ranges_and_rejects_ambiguous_slots() {
        let range = |base_register, num, offset_in_table| RootRange {
            range_type: 0,
            num,
            base_register,
            space: 0,
            offset_in_table,
        };
        let ranges = [range(6, 1, 0), range(17, 2, u32::MAX)];
        assert_eq!(table_register(&ranges, 0).unwrap(), 6);
        assert_eq!(table_register(&ranges, 1).unwrap(), 17);
        assert_eq!(table_register(&ranges, 2).unwrap(), 18);
        assert!(table_register(&ranges, 3).is_err());
        let ranges = [range(4, 2, 2), range(9, 1, u32::MAX)];
        assert!(table_register(&ranges, 1).is_err());
        assert_eq!(table_register(&ranges, 4).unwrap(), 9);
        assert!(table_register(&[range(0, 2, 0), range(8, 1, 1)], 1).is_err());
        assert!(table_register(&[range(u32::MAX, 2, 0)], 1).is_err());
    }
}
