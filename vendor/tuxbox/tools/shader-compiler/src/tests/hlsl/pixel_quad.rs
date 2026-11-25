use super::*;
use crate::rdna2::disassemble;

#[test]
fn distinguishes_saved_coverage_from_scalar_bits() {
    // save EXEC, mask it with VCC, subtract it from the saved coverage,
    // skip an empty result, expand to whole quads, restore working EXEC.
    let words: [u32; 6] = [
        0xbe9a047e, 0xbe8c246a, 0x8a9a7e1a, 0xbf840002, 0xbe8e0a1a, 0x87fe0e0c,
    ];
    let bytes: Vec<_> = words.into_iter().flat_map(u32::to_le_bytes).collect();
    let mut code = disassemble(&bytes);
    assert!(guards_expansion(&code, 2));
    assert!(saved_exec(&code, 4));
    code[0].src[0] = Opnd::Sgpr(30);
    assert!(!guards_expansion(&code, 2));
    assert!(!saved_exec(&code, 4));
}
