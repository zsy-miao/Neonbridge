use super::*;

#[test]
fn a_skipped_assignment_cannot_resolve_a_root_address() {
    // s_cbranch_execz +1; s_mov_b32 s12, 0; s_mov_b32 s13, s12
    let words = [0xbf880001u32, 0xbe8c0380, 0xbe8d030c];
    let body = disassemble(
        &words
            .into_iter()
            .flat_map(u32::to_le_bytes)
            .collect::<Vec<_>>(),
    );
    assert!(check_alu(&body, 12, 32)
        .unwrap_err()
        .contains("scalar input"));
    assert!(check_alu(&body[1..], 12, 32).is_ok());
}
