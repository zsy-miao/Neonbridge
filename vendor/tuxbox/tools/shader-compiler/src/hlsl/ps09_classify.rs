//! Wave64 material classification for the verified 8x8 PS09 compute PSO 439.
//! Its two DPP OR reductions and first-active-lane election span an entire
//! workgroup, including on a host that uses 32-lane subgroups.
use super::{
    compute_group, packed_root::matches_program, Emitter, Enc, Insn, Opnd, Res, ShaderExtract,
    Stage,
};

const COLLECT: u32 = 0x7fd0;
const BIT_REPLICATE: u32 = 0x7fd1;

pub(super) fn prepare(ex: &ShaderExtract, insns: &mut [Insn]) -> bool {
    if ex.stage != Stage::Compute
        || compute_group(ex) != [8, 8, 1]
        || !matches_program(&ex.isa, 968, 0x4cd10031edbd1352)
    {
        return false;
    }
    for ins in insns {
        match ins.offset {
            // Every invocation must reach the collective. Keep the original EXEC
            // updates, so out-of-bounds pixels and zero IDs contribute no key.
            0x50 | 0x8c => nop(ins),
            // S_BITREPLICATE_B64_B32 VCC, 0x33333333 duplicates each source
            // bit. Workgroup columns 0..3 and 4..7 therefore use different keys.
            0x14c => {
                ins.enc = Enc::Vop1;
                ins.op = BIT_REPLICATE;
                ins.mnemonic = "tb_class_columns".into();
            }
            0x1b8 => {
                ins.enc = Enc::Vop1;
                ins.op = COLLECT;
                ins.mnemonic = "tb_class_collect".into();
                ins.dst = vec![Opnd::Sgpr(16), Opnd::Sgpr(17)];
                ins.src = vec![Opnd::Vgpr(6), Opnd::Vgpr(4), Opnd::ExecLo];
            }
            0x1bc..=0x274 => nop(ins),
            // One lane is active after the election. Choosing the first active
            // lane again preserves EXEC, while its prefix population count is 0.
            0x28c | 0x290 | 0x2c4 | 0x2dc | 0x330 | 0x334 | 0x368 | 0x380 => nop(ins),
            0x2f8 | 0x300 | 0x3a0 | 0x3a8 => {
                ins.enc = Enc::Vop1;
                ins.mnemonic = "v_mov_b32".into();
                ins.op = 1;
                ins.src = vec![Opnd::ConstInt(0)];
            }
            // The only consumer of the atomic's returned value is the issuing
            // lane. Its scalar copy can use that lane's own VGPR on either host.
            0x2f4 | 0x39c => {
                ins.enc = Enc::Sop1;
                ins.mnemonic = "s_mov_b32".into();
                ins.op = 3;
            }
            _ => {}
        }
    }
    true
}

fn nop(ins: &mut Insn) {
    ins.enc = Enc::Vop1;
    ins.op = 0;
    ins.mnemonic = "v_nop".into();
    ins.dst = vec![Opnd::Vgpr(0)];
    ins.src.clear();
}

pub(super) const DECLARATIONS: &str =
    "groupshared uint tb_class_left, tb_class_right, tb_class_first;\n";

pub(super) fn emit(em: &mut Emitter<'_>, ins: &Insn) -> bool {
    match (ins.enc, ins.op) {
        (Enc::Vop1, BIT_REPLICATE) => {
            em.line("vcc_lo = (gtid.x < 4u); vcc_hi = 0u;");
        }
        (Enc::Vop1, COLLECT) => {
            for s in &ins.src {
                em.touch(s);
            }
            for d in &ins.dst {
                em.touch(d);
            }
            em.line("uint lane = gtid.y * 8u + gtid.x;");
            em.line("if (lane == 0u) { tb_class_left = 0u; tb_class_right = 0u; tb_class_first = 64u; }");
            em.line("GroupMemoryBarrierWithGroupSync();");
            em.line("if (exec_lo & 1u) { InterlockedOr(tb_class_left, v6); InterlockedOr(tb_class_right, v4); InterlockedMin(tb_class_first, lane); }");
            em.line("GroupMemoryBarrierWithGroupSync();");
            em.line("s16 = tb_class_left; s17 = tb_class_right;");
            em.line("exec_lo = (lane == tb_class_first); exec_hi = 0u;");
        }
        _ => return false,
    }
    true
}

/// The program converts the t71/t72 structured records to byte addressing and
/// uses u24 for both ordinary stores and atomics. Validate the table resolution
/// before assigning the element sizes from PS09 binding record 12267.
pub(super) fn resources(em: &mut Emitter<'_>) -> Result<(), String> {
    for (idx, i) in em.insns.iter().enumerate() {
        let resource = match i.offset {
            0x24 => Res::Cb(17),
            0x60 => Res::Tex(237, 1),
            0xf4 | 0xfc | 0x104 => Res::Buf(71, false),
            0x144 => Res::Buf(72, false),
            0x2d4 | 0x378 => Res::TBuf(24, true),
            0x314 | 0x3bc => Res::Buf(24, true),
            _ => continue,
        };
        let resolved = em.res.get(&idx).and_then(|v| v.first()).copied();
        // With PSV resource metadata the generic resolver already knows the
        // atomic's view is structured. Without it the resolver assumes typed.
        if matches!(resource, Res::TBuf(24, true)) && resolved == Some(Res::Buf(24, true)) {
            continue;
        }
        if resolved != Some(resource) {
            return Err(format!(
                "unexpected material classifier binding at {:#x}",
                i.offset
            ));
        }
        if matches!(resource, Res::TBuf(24, true)) {
            em.res.insert(idx, vec![Res::Buf(24, true)]);
        }
    }
    em.sbuf
        .extend([((false, 71), 84), ((false, 72), 44), ((true, 24), 1)]);
    em.tbuf.remove(&(true, 24));
    em.tbuf_uint.remove(&24);
    em.img_ty.insert((false, 237), 1);
    Ok(())
}
