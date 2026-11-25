use std::{env, fs};
use tuxbox_xshader::{extract, hlsl::translate_lds_exchange_probe, rdna2::instructions};
fn main() -> Result<(), Box<dyn std::error::Error>> {
    let args: Vec<_> = env::args().collect();
    let ex = extract(&fs::read(&args[1])?)?;
    let op = instructions(&ex.isa)
        .find(|i| i.offset == 0x4ec)
        .ok_or("missing")?;
    if op.raw != [0xd8b40000, 0x2d002d2b] {
        return Err("unexpected opcode".into());
    }
    fs::write(&args[2], translate_lds_exchange_probe(&ex, 0x4ec)?)?;
    Ok(())
}
