//! Emit a synthetic compute DS_SWIZZLE_B32 via the actual RDNA2 translator.
use tuxbox_xshader::{ShaderExtract, Stage, hlsl};

mod fixture {
    include!(concat!(env!("CARGO_MANIFEST_DIR"), "/examples/support/compute_swizzle_fixture.rs"));
}

fn main() {
    let offset = std::env::args().nth(1).expect("swizzle offset");
    let offset = u32::from_str_radix(offset.trim_start_matches("0x"), 16).expect("hexadecimal offset");
    assert!(offset <= 0xffff);
    let translated = hlsl::translate(&fixture::shader(offset, 0)).expect("translate synthetic swizzle");
    print!("{}", translated.hlsl);
}
