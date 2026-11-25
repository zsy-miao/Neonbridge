use super::*;

#[test]
fn copy_loop_rejects_unbound_descriptors_and_external_entries() {
    // Synthetic loop with one SRV, one UAV, and a scalar iteration limit.
    let words = [
        0x7da80205u32,
        0xbf880010,
        0xbe88037e,
        0xbf820002,
        0x877e080a,
        0xbf800000,
        0xbf800000,
        0xbf800000,
        0xe0381000,
        0x80030201,
        0xbe8a037e,
        0xbf8c3f70,
        0xe0781000,
        0x80000201,
        0x4a020207,
        0x7da60205,
        0x8a087e08,
        0xbf85fff2,
        0xbefe0304,
    ];
    let insns = disassemble(
        &words
            .into_iter()
            .flat_map(u32::to_le_bytes)
            .collect::<Vec<_>>(),
    );
    let mut state = [Word::Data; 106];
    for (base, uav) in [(0, true), (12, false)] {
        let b = Binding { slot: 0, uav };
        state[base..base + 4].copy_from_slice(&[
            Word::AddressLo(b),
            Word::AddressHi(b, 1),
            Word::Immediate(u32::MAX),
            Word::Immediate(0x11014fac),
        ]);
    }
    let run =
        |mut state, joins| PackedRoot::default().copy_loop(3, &insns, &[], &mut state, &joins);
    assert!(run(state, BTreeMap::new()).unwrap().is_some());
    assert!(run(state, BTreeMap::from([(insns[8].offset, state)])).is_err());
    state[13] = Word::Data;
    assert!(run(state, BTreeMap::new()).is_err());
}
