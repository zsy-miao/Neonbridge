use super::*;
use crate::rdna2::disassemble;

fn reaches(words: &[u32]) -> bool {
    let bytes: Vec<_> = words.iter().flat_map(|word| word.to_le_bytes()).collect();
    table_reaches_tail(&disassemble(&bytes), bytes.len(), bytes.len() + 64)
}

#[test]
fn append_table_must_reach_every_entry_to_the_tail() {
    const COPY: u32 = 0xbeb40307; // s52 = incoming UAV table s7
    const ZERO: u32 = 0xbeb50380; // s53 = 0
    assert!(reaches(&[COPY, ZERO]));
    assert!(!reaches(&[COPY, 0xbf820001, ZERO])); // branch over high word
    assert!(!reaches(&[COPY, ZERO, 0xbf820001])); // branch into append
    assert!(!reaches(&[COPY, ZERO, 0xbeb40380])); // overwrite table
    assert!(!reaches(&[0xbe870380, COPY, ZERO])); // overwrite source
    assert!(!reaches(&[COPY, ZERO, 0xf4080c9a, 0xfa000040])); // load s50..53
}
