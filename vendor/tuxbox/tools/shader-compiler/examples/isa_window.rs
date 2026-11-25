//! Print RDNA2 instructions of an Xbox shader around a byte offset.
//! usage: isa_window <shader.dxbc> <offset hex> [before] [after]
fn main() {
    let mut args = std::env::args().skip(1);
    let path = args.next().unwrap();
    let at = u32::from_str_radix(args.next().unwrap().trim_start_matches("0x"), 16).unwrap();
    let before: usize = args.next().map_or(30, |v| v.parse().unwrap());
    let after: usize = args.next().map_or(8, |v| v.parse().unwrap());
    let ex = tuxbox_xshader::extract(&std::fs::read(&path).unwrap()).unwrap();
    let insns = tuxbox_dxbc::rdna2::disassemble(&ex.isa);
    let i = insns.iter().position(|x| x.offset as u32 >= at).unwrap_or(0);
    for x in &insns[i.saturating_sub(before)..(i + after).min(insns.len())] {
        println!("{:#06x} {:24} {:?} <- {:?} imm={} mask={} raw={:08x?}", x.offset, x.mnemonic, x.dst, x.src, x.aux.imm, x.aux.mask, x.raw);
    }
}
