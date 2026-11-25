use super::{continuation, resources};
use crate::rootsig::{RootParam, RootParamKind, RootRange, RootSig};

const RETURN: u32 = 0xbe80_201e;
const NOP: u32 = 0xbf80_0000;

fn lift(words: &[u32]) -> Result<String, String> {
    let bytes: Vec<_> = words.iter().flat_map(|w| w.to_le_bytes()).collect();
    let mut out = String::new();
    continuation(
        &bytes,
        0,
        bytes.len(),
        &mut out,
        &resources::Bindings::default(),
        None,
        false,
    )?;
    Ok(out)
}

#[test]
fn nested_back_edges_keep_the_first_entry_to_the_inner_loop() {
    let lowered = lift(&[
        0xbf84_0001,
        NOP, // 0 -> 8 or 4
        0xbf82_0001,
        NOP,         // 8 -> 16, skipping 12 on the first iteration
        0xbf85_fffe, // 16 -> 12 or 20
        0xbf85_fffb, // 20 -> 4 or 24
        RETURN,
    ])
    .unwrap();
    assert!(!lowered.contains("switch"));
    assert_eq!(lowered.matches("[loop] do").count(), 2);
    let edges = [
        "if (scc == 0u) nextBlock = 8u;",
        "if (nextBlock <= 20u) { [loop] do",
        "nextBlock = 16u;\n}",
        "if (nextBlock <= 16u) { [loop] do",
        "if (scc != 0u) nextBlock = 12u;",
        "while (nextBlock >= 12u && nextBlock <= 16u)",
        "if (scc != 0u) nextBlock = 4u;",
        "while (nextBlock >= 4u && nextBlock <= 20u)",
    ];
    let mut rest = lowered.as_str();
    for edge in edges {
        rest = rest.split_once(edge).expect(edge).1;
    }
}

#[test]
fn crossing_back_edges_dispatch_and_invalid_edges_fail() {
    // Reachable back edges [0,8] and [4,12] overlap without nesting.
    let crossing = lift(&[NOP, NOP, 0xbf85_fffd, 0xbf85_fffd, RETURN]).unwrap();
    assert!(crossing.contains("switch (nextBlock)"));
    assert_eq!(crossing.matches("case ").count(), 4);
    assert!(lift(&[]).is_err());
    assert!(lift(&[0xbf82_0001, RETURN]).is_err()); // beyond final instruction
    assert!(lift(&[0xbf82_0001, 0x8036_ff36, 0x1234, RETURN]).is_err()); // literal operand
    assert!(lift(&[NOP]).is_err()); // fallthrough
}

#[test]
fn consecutive_vector_moves_share_the_guard_until_exec_changes() {
    let move_v29 = 0x7e3a_0280; // v_mov_b32 v29, 0
    let move_v45 = 0x7e5a_0280; // v_mov_b32 v45, 0
    let restore_exec = 0xbefe_0335; // s_mov_b32 exec_lo, s53
    let lowered = lift(&[move_v29, NOP, move_v45, restore_exec, move_v45, RETURN]).unwrap();
    assert_eq!(lowered.matches("if (exec != 0u)").count(), 2);
    let (before, after) = lowered.split_once("}\nexec = s[53];\n").unwrap();
    assert!(before.contains("v[29] =") && before.contains("v[45] ="));
    assert!(after.starts_with("if (exec != 0u) {\nv[45] ="));
}

#[test]
fn scalar_bit_operations_preserve_their_native_condition_codes() {
    let reverse = lift(&[3199470388, RETURN]).unwrap();
    assert!(reverse.contains("s[52] = reversebits(s[52]);"));
    assert!(!reverse.contains("scc ="));
    let shift = lift(&[2436071731, RETURN]).unwrap();
    assert!(shift.contains("asint(s[51]) >>"));
    assert!(shift.contains("scc = (s[51] != 0u)"));
    // S_BFE_U32 accepts a seven-bit width. V_BFE_U32 takes five bits.
    let extract = lift(&[0x938b_0300, 0xd548_0001, 0x040a_0300, RETURN]).unwrap();
    assert!(extract.contains("tb_ray_bfe(s[0], s[3], ((s[3] >> 16) & 127u))"));
    assert!(extract.contains("scc = (s[11] != 0u)"));
    assert!(extract.contains("tb_ray_bfe(v[0], v[1], (v[2] & 31u))"));
    assert!(lift(&[3128952961, 0, RETURN]).is_ok());
    assert!(lift(&[3128952961, 1, RETURN]).is_err());
    let count = lift(&[2268364394, 3199471412, 2234810497, RETURN]).unwrap();
    assert!(count.contains("WaveActiveBallot(s[52] != 0u)"));
    assert!(count.contains("votes[WaveGetLaneIndex() >> 5u]"));
    assert!(lift(&[3199471412, 2234810497, RETURN]).is_err());
    assert!(lift(&[2268364394, 3199471412, RETURN]).is_err());
}

#[test]
fn scratch_writes_preserve_partial_words_and_private_spills_are_separate() {
    let half = lift(&[3764916284, 67121959, RETURN]).unwrap();
    assert!(half.contains("spill[i] & ~(0xffffu << shift)"));
    assert!(half.contains("if (shift == 24u) spill[i + 1u] = (spill[i + 1u] & ~255u)"));
    let save = [3765436420, 83888128]; // private scratch[4] = v8
    assert!(lift(&[save[0], save[1], RETURN]).is_err());
    let restored = lift(&[3196388128, save[0], save[1], 3761242116, 83892736, RETURN]).unwrap();
    assert!(restored.contains("privateWord1 = v[8];"));
    assert!(restored.contains("v[26] = privateWord1;"));
}

#[test]
fn cold_branch_islands_keep_spills_on_the_taken_edge() {
    let hlsl = lift(&[
        3196388128,  // s5 = s32, private scratch base
        0xbf85_0002, // branch on SCC to the island at 16
        NOP,
        RETURN,
        3765436420,
        83888128,    // private scratch[4] = v8
        0xbf82_fffc, // jump back to RETURN at 12
    ])
    .unwrap();
    assert!(!hlsl.contains("switch") && !hlsl.contains("[loop]"));
    let taken = hlsl.split_once("if (scc != 0u) {").unwrap().1;
    assert!(taken.starts_with(" if (exec != 0u) { privateWord1 = v[8];"));
    assert!(taken.contains("} nextBlock = 12u; }"));
    assert_eq!(hlsl.matches("privateWord1 = v[8]").count(), 1);
}

#[test]
fn ray_export_uses_declared_register_space_and_table_offsets() {
    let table = RootParam {
        visibility: 0,
        kind: RootParamKind::Table(vec![
            RootRange {
                range_type: 0,
                num: 2,
                base_register: 7,
                space: 3,
                offset_in_table: 2,
            },
            RootRange {
                range_type: 1,
                num: 1,
                base_register: 5,
                space: 6,
                offset_in_table: u32::MAX,
            },
        ]),
    };
    let root = RootSig {
        version: 2,
        flags: 0,
        params: vec![table],
        num_static_samplers: 0,
        static_samplers: vec![],
    };
    let binding = super::super::binding;
    assert_eq!(
        binding(&root, 0, 3, 0).as_deref(),
        Some("register(t8, space3)")
    );
    assert_eq!(
        binding(&root, 0, 4, 1).as_deref(),
        Some("register(u5, space6)")
    );
    assert!(binding(&root, 0, 4, 0).is_none());
    assert!(binding(&root, 0, 0, 0).is_none());
}
