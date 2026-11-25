//! One Xbox wave covers one 4x4x4 light grid tile. Elect and reserve once per
//! light or capture, even when the host executes that tile in several subgroups.
use super::{compute_group, packed_root::matches_program, Emitter, Enc, Res, Stage};

pub(super) fn prepare(em: &mut Emitter<'_>) -> Result<bool, String> {
    if em.ex.stage != Stage::Compute || compute_group(em.ex) != [4, 4, 4]
        || em.ex.entry.as_deref() != Some("LightGridInjectionCS")
        || !matches_program(&em.ex.isa, 2092, 0xff3ee237a6592f82)
    {
        return Ok(false);
    }
    // Both loops take their iteration count from CB2[0].xy. Edge cells and
    // lanes outside a volume must still participate with a zero vote. All
    // vector writes following the EXECZ skips are already predicated on EXEC.
    // Other scalar branches inside the intersection tests rejoin before voting.
    for (idx, ins) in em.insns.iter_mut().enumerate() {
        let pc = ins.offset;
        if matches!(pc, 0x628 | 0x7b0)
            && !matches!(em.res.get(&idx).map(Vec::as_slice), Some([Res::TBuf(0, true)]))
        {
            return Err("light grid reservation requires a typed counter at u0".into());
        }
        let reserve = matches!(pc, 0x5f0 | 0x778);
        if matches!(pc, 0x078 | 0x504 | 0x5e8 | 0x658 | 0x770 | 0x7e0)
            || (0x5f0..=0x64c).contains(&pc) || (0x778..=0x7d4).contains(&pc)
        {
            // The entire elected atomic + broadcast + exclusive prefix produces
            // only v7 live-out. The original EXEC at entry remains in effect.
            ins.enc = Enc::Sopp;
            ins.op = 0;
            ins.mnemonic = if reserve { "tb_light_grid_reserve" } else { "s_nop" }.into();
            ins.src.clear();
            ins.dst.clear();
        }
    }
    em.tbuf_uint.insert(0);
    Ok(true)
}

pub(super) const DECLARATIONS: &str = "groupshared uint light_votes[64], light_offsets[8], light_base;\n";

// Each 8-cell block builds a bit mask in logical Xbox lane order. Its leader
// publishes a count; lane zero reserves their combined range once. A group's
// first barrier in the next iteration also protects the preceding base/offset
// reads. Thread identity is SV_GroupThreadID and requires no host wave width.
pub(super) const RESERVE: &str = r#"{
    uint lane = gtid.x + 4u * gtid.y + 16u * gtid.z;
    uint bit = 1u << (lane & 7u);
    light_votes[lane] = (exec_lo & 1u) ? bit : 0u;
    GroupMemoryBarrierWithGroupSync();
    uint mask = 0u;
    [unroll] for (uint k = 0u; k < 8u; ++k) mask |= light_votes[(lane & ~7u) + k];
    if ((lane & 7u) == 0u) light_offsets[lane >> 3] = countbits(mask);
    GroupMemoryBarrierWithGroupSync();
    if (lane == 0u) {
        uint total = 0u, base = 0u;
        [unroll] for (uint k = 0u; k < 8u; ++k) {
            uint count = light_offsets[k];
            light_offsets[k] = total;
            total += count;
        }
        if (total != 0u) InterlockedAdd(utb0[0], total, base);
        light_base = base;
    }
    GroupMemoryBarrierWithGroupSync();
    if (exec_lo & 1u) v7 = light_base + light_offsets[lane >> 3] + countbits(mask & (bit - 1u));
}"#;
