use super::only_lane_zero;
use crate::rdna2::disassemble;

#[test]
fn restore_requires_a_snapshot_on_every_path() {
    let save = 0xbe8a246a; // s_and_saveexec_b64 s[10:11], vcc
    let restore = 0xbefe040a; // s_mov_b64 exec, s[10:11]
    let overwrite_high = 0xbe8b0381; // s_mov_b32 s11, 1
    let skip = 0xbf850001; // s_cbranch_scc1 +1
    let end = 0xbf810000;
    let admitted = |words: &[u32]| {
        let bytes = words
            .iter()
            .flat_map(|w| w.to_le_bytes())
            .collect::<Vec<_>>();
        only_lane_zero(&disassemble(&bytes))
    };
    assert!(admitted(&[save, restore, end]));
    assert!(!admitted(&[skip, save, restore, end]));
    assert!(!admitted(&[save, overwrite_high, restore, end]));
}
