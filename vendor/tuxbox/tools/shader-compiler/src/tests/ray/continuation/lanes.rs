use super::*;
use crate::rdna2::instructions;

fn code(words: &[u32]) -> Vec<Insn> {
    let bytes: Vec<_> = words.iter().flat_map(|w| w.to_le_bytes()).collect();
    instructions(&bytes).collect()
}

#[test]
fn image_load_uses_only_its_2d_coordinates() {
    let load = code(&[0xf000_1108, 0x0017_1d2c]);
    assert!(touches(&load[0], 44));
    assert!(touches(&load[0], 45));
    assert!(!touches(&load[0], 46));
    let packed = code(&[0xf000_1108, 0x4017_1d2c]);
    assert!(touches(&packed[0], 44));
    assert!(!touches(&packed[0], 45));
    let sample_l = code(&[0xf090_030a, 0x01ed_4934, 0x294d]);
    assert!(touches(&sample_l[0], 77));
    assert!(!touches(&sample_l[0], 79));
}

#[test]
fn scalar_lane_pair_must_dominate_restore_and_own_the_vgpr() {
    let save = [3613458504, 65544]; // v_writelane_b32 v72, s8, 0
    let restore = [3613392904, 65864]; // v_readlane_b32 s8, v72, 0
    let mut pair = save.to_vec();
    pair.extend(restore);
    let mut output = String::new();
    let ops = code(&pair);
    declarations(&ops, &mut output).unwrap();
    assert_eq!(output, "uint savedLane72_0;\n");
    assert_eq!(
        emit(&ops[1]).unwrap().as_deref(),
        Some("s[8] = savedLane72_0;")
    );
    let mut alternate = save.to_vec();
    alternate.push(0xbf84_0003); // conditional branch to the second restore
    alternate.extend(restore);
    alternate.push(0xbf82_0002);
    alternate.extend(restore);
    alternate.push(0xbe80_201e);
    declarations(&code(&alternate), &mut String::new()).unwrap();
    let mut bypass = vec![0xbf820002]; // branch directly to the restore
    bypass.extend(&pair);
    assert!(declarations(&code(&bypass), &mut String::new()).is_err());
    let mut clobber = ops;
    let mut ordinary = clobber[1].clone();
    ordinary.mnemonic = "v_mov_b32".into();
    clobber.push(ordinary);
    assert!(declarations(&clobber, &mut String::new()).is_err());
}
