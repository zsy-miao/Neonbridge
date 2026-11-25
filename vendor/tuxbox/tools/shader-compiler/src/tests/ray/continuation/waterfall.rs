use super::*;
use crate::rdna2::instructions;

#[test]
fn a_proven_pair_uses_each_invocations_address_and_keeps_the_resource_operation() {
    let words: [u32; 14] = [
        2120746286, 2120877359, // readfirstlane s52,s53 from v46,v47
        3571580982, 154676, 3200072758, // compare address and save EXEC in s61
        4094168346, 4194304000, 3213672575, 3212836864, 3212836864, 3761250312,
        2148346402, // typed descriptor load then one dword read
        2306751870, 3213492210, // remove processed lanes and repeat
    ];
    let bytes: Vec<_> = words.iter().flat_map(|w| w.to_le_bytes()).collect();
    let original: Vec<_> = instructions(&bytes).collect();
    let mut specialized = original.clone();
    specialize(&mut specialized);
    assert_eq!(specialized[0].mnemonic, "s_mov_b32");
    assert_eq!(specialized[1].src, [Opnd::Vgpr(47)]);
    assert_eq!(specialized[2].src, [Opnd::ExecLo]);
    assert_eq!(specialized.last().unwrap().mnemonic, "s_nop");
    for (i, op) in original.iter().enumerate().take(original.len() - 1).skip(3) {
        assert_eq!(specialized[i].raw, op.raw);
        assert_eq!(specialized[i].mnemonic, op.mnemonic);
    }
    let mut mismatched = original.clone();
    mismatched[1].src = vec![Opnd::Vgpr(48)];
    specialize(&mut mismatched);
    assert_eq!(mismatched[0].mnemonic, "v_readfirstlane_b32");
    let mut overwritten = original.clone();
    overwritten[4].dst = vec![Opnd::Sgpr(60)]; // x4 descriptor would clobber saved s61
    specialize(&mut overwritten);
    assert_eq!(overwritten.last().unwrap().mnemonic, "s_cbranch_execnz");
}
