use super::*;

#[test]
fn launch_requires_the_packed_group_and_wave64_lane_definitions() {
    let words: [u32; 10] = [
        0xd7650000, 0x000100c1, // v0 = mbcnt_lo(-1, 0)
        0x7e000505, // s0 = readfirstlane(v5)
        0x9381ff03, 0x00040018, // s1 = s3[27:24]
        0xd7660000, 0x000200c1, // v0 = mbcnt_hi(-1, v0)
        0x900a8800, // s10 = s0 >> 8
        0xd7460001, 0x04010c01, // v1 = (s1 << 6) + v0
    ];
    let check = |words: &[u32]| {
        let bytes = words
            .iter()
            .flat_map(|w| w.to_le_bytes())
            .collect::<Vec<_>>();
        validate_launch(&disassemble(&bytes), 8, Opnd::Vgpr(1))
    };
    assert!(check(&words).is_ok());
    let mut wrong_wave = words;
    wrong_wave[4] = 0x00040017;
    assert!(check(&wrong_wave).is_err());
    // An s_load_dwordx4 into s[0:3] clobbers both intermediate values.
    let mut clobbered = words.to_vec();
    clobbered.splice(7..7, [0xf408000d, 0xfa000000]);
    assert!(check(&clobbered).is_err());
    let mut wrong_group = words;
    wrong_group[7] = 0x900a8700;
    assert!(check(&wrong_group).is_err());
}

#[test]
fn only_uniform_values_can_bypass_a_wave_broadcast() {
    let validate = |words: &[u32]| {
        let bytes = words
            .iter()
            .flat_map(|w| w.to_le_bytes())
            .collect::<Vec<_>>();
        { let body = disassemble(&bytes); validate_lane_ops(&body, &waterfall_reads(&body), false) }
    };
    // Entry v5 holds the same packed group identifier in every lane.
    assert!(validate(&[0x7e000505]).is_ok());
    // Counting preceding lanes overwrites v5 with a lane-dependent value.
    assert!(validate(&[0xd7650005, 0x000100c1, 0x7e000505]).is_err());
    // A new constant in v5 is safe to broadcast.
    assert!(validate(&[0x7e0a0281, 0x7e000505]).is_ok());
    // Shared memory would require synchronization among logical Xbox waves.
    assert!(validate(&[0xd8380100, 0x00020100]).is_err());
}

#[test]
fn class5_mesh_pointer_is_eight_above_the_user_location() {
    assert_class5_mesh_pointer();
}

#[test]
fn class5_descriptor_load_uses_the_guest_heap_index() {
    assert_class5_descriptor_load();
}
