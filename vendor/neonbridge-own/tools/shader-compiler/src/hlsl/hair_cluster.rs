//! Frosty MainClusterCullingCS, global entry 506. Native lanes with nonzero
//! vertex counts reserve one contiguous range for the wave and accumulate
//! twice their combined vertex counts for the draw-indirect buffer. The cooked
//! group has exactly 64 invocations; workgroup collectives also span two wave32s.
use super::{compute_group, packed_root::matches_program, Emitter, Enc, Insn, Opnd, ShaderExtract, Stage};

pub(super) fn prepare(ex: &ShaderExtract, insns: &mut Vec<Insn>) -> bool {
    if ex.stage != Stage::Compute || compute_group(ex) != [64, 1, 1]
        || ex.entry.as_deref() != Some("MainClusterCullingCS")
        || !matches_program(&ex.isa, 1816, 0xe1743632f2236dac)
    { return false; }
    // Preserve the entire bounds/frustum/LOD calculation and every earlier
    // branch to the original 0x714 exit. Save the four live export registers
    // only when control reaches the first election with an active invocation.
    insns.retain(|i| i.offset <= 0x5d0 || i.offset == 0x714);
    let index = insns.len() - 2;
    let save = &mut insns[index];
    save.enc = Enc::Vop1;
    save.mnemonic = "tb_hair_capture".into();
    save.src = [0, 3, 5, 4].into_iter().map(Opnd::Vgpr).collect();
    save.dst.clear();
    true
}

pub(super) fn emit(em: &mut Emitter<'_>, ins: &Insn) -> bool {
    if ins.mnemonic != "tb_hair_capture" { return false; }
    for r in &ins.src { em.touch(r); }
    em.line("if (exec_lo & 1u) hair_record = uint4(v0, v3, v5, v4);");
    true
}

pub(super) const DECLARATIONS: &str = r#"
groupshared uint hair_mask[2], hair_vertices, hair_base;
RWBuffer<uint> hair_ids : register(u0);
RWBuffer<uint> hair_offsets : register(u1);
RWBuffer<uint> hair_counts : register(u2);
RWBuffer<float> hair_radii : register(u3);
RWBuffer<uint> hair_group_count : register(u4);
RWBuffer<uint> hair_draw_args : register(u5);
"#;

pub(super) const EPILOGUE: &str = r#"
    uint lane = gtid.x, half_lane = lane & 31u;
    if (lane == 0) { hair_mask[0] = 0; hair_mask[1] = 0; hair_vertices = 0; }
    GroupMemoryBarrierWithGroupSync();
    if (hair_record.z != 0u) {
        InterlockedOr(hair_mask[lane >> 5u], 1u << half_lane);
        InterlockedAdd(hair_vertices, hair_record.z << 1u);
    }
    GroupMemoryBarrierWithGroupSync();
    if (lane == 0) {
        uint count = countbits(hair_mask[0]) + countbits(hair_mask[1]);
        if (count != 0u) {
            uint base;
            InterlockedAdd(hair_group_count[0], count, base);
            hair_base = base;
            InterlockedAdd(hair_draw_args[0], hair_vertices);
        }
    }
    GroupMemoryBarrierWithGroupSync();
    if (hair_record.z != 0u) {
        uint rank = countbits(hair_mask[lane >> 5u] & ((1u << half_lane) - 1u));
        if (lane >= 32u) rank += countbits(hair_mask[0]);
        uint slot = hair_base + rank;
        hair_ids[slot] = hair_record.x;
        hair_offsets[slot] = hair_record.y;
        hair_counts[slot] = hair_record.z;
        hair_radii[slot] = asfloat(hair_record.w);
    }
"#;
