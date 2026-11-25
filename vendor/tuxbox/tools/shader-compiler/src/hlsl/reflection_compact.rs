//! Reflection compaction uses a wave32 exclusive scan twice. In the first
//! scan the guest enables four lanes, then temporarily restores the full wave
//! before permuting; keep every host lane present through the scan.
use super::{compute_group, packed_root::matches_program, Enc, Insn, Opnd, ShaderExtract, Stage};

pub(super) fn prepare(ex: &ShaderExtract, insns: &mut [Insn]) -> bool {
    if ex.stage != Stage::Compute || !matches!(compute_group(ex), [64 | 128 | 256 | 512 | 1024, 1, 1])
        || ex.entry.as_deref() != Some("ReflectionCompactTracesCS")
        || ![(64, 1568, 0x46c5a463fb5f8a4f), (64, 1024, 0xdcce75f30dec50a3),
             (128, 1572, 0x352cb470906c4d15), (256, 1572, 0x186f5256647f7b64),
             (512, 1572, 0xc3c58eb73cf9db1a), (1024, 1572, 0x6b2bfd863f5849da),
             (128, 1028, 0xb3ce5312062492b2), (256, 1028, 0x886315b1e612c676),
             (512, 1028, 0x935eca3a19d65f11), (1024, 1028, 0xafda0300adc5b69c)]
            .into_iter().any(|(x, len, hash)| compute_group(ex) == [x, 1, 1] && matches_program(&ex.isa, len, hash))
    {
        return false;
    }
    let small = matches!(ex.isa.len(), 1024 | 1028);
    let (branch, a, b) = if small { (0x2a0, 0x2c0, 0x334) } else { (0x4c0, 0x4e0, 0x554) };
    let shift = if compute_group(ex)[0] == 64 { 4 } else { 0 };
    let (branch, a, b) = (branch - shift, a - shift, b - shift);
    for op in insns {
        if op.offset == branch {
            // The EXEC-zero branch skips inactive lanes on a per-invocation
            // host. They must remain present for the subsequent wave scan.
            op.mnemonic = "v_nop".into();
            op.enc = Enc::Vop1;
            op.dst = vec![Opnd::Vgpr(0)];
        } else if op.offset == a || op.offset == b {
            op.mnemonic = "tb_reflection_prefix32".into();
            op.src = vec![Opnd::Vgpr(1)];
        } else if (a + 4..=a + 0x3c).contains(&op.offset) || (b + 4..=b + 0x3c).contains(&op.offset) {
            op.mnemonic = "v_nop".into();
            op.enc = Enc::Vop1;
        }
    }
    true
}
