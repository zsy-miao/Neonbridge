use super::*;

fn run(words: &[u32]) -> HashMap<u16, Sym> {
    let isa: Vec<u8> = words.iter().flat_map(|w| w.to_le_bytes()).collect();
    let mut sym = HashMap::new();
    entry(&mut sym);
    for ins in crate::rdna2::disassemble(&isa) {
        step(&ins, &mut sym, false);
    }
    sym
}

#[test]
fn launch_arguments_and_scalar_arithmetic_stay_clean() {
    let sym = run(&[0x8f00850a]); // s0 = s10 << 5
    assert!(clean(&sym, 0) && clean(&sym, 10));
}

#[test]
fn a_lane_value_taints_what_is_computed_from_it() {
    let sym = run(&[
        0x7e000505, // s0 = readfirstlane(v5)
        0x8f018500, // s1 = s0 << 5
        0x8f02850a, // s2 = s10 << 5
    ]);
    assert!(!clean(&sym, 0) && !clean(&sym, 1));
    assert!(clean(&sym, 2));
    // An independent value makes the register clean again.
    assert!(clean(&run(&[0x7e000505, 0x8f00850a]), 0));
}

#[test]
fn a_scalar_load_is_tainted_by_its_offset() {
    // s3 = buffer[s28:31][s0], from a lane value and from a launch argument.
    assert!(!clean(&run(&[0x7e000505, 0xf42000ce, 0x00000000]), 3));
    assert!(clean(&run(&[0xf42000ce, 0x00000000]), 3));
}

#[test]
fn comparison_masks_are_per_lane_bits_and_drop_heap_symbols() {
    let isa: Vec<u8> = [0xd4030000u32, 0x00010016].into_iter().flat_map(u32::to_le_bytes).collect();
    let ins = crate::rdna2::disassemble(&isa); // s[0:1] = v_cmp_le_f32(s22, 0)
    let mut sym = HashMap::from([(0, Sym::HeapOffset), (1, Sym::HeapWord(1, Some(0))), (2, Sym::HeapOffset)]);
    entry(&mut sym);
    step(&ins[0], &mut sym, false);
    assert!(clean(&sym, 0));
    assert!(!sym.contains_key(&0) && !sym.contains_key(&1) && sym.contains_key(&2));
}
