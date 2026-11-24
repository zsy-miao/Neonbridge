//! Emit native single-wave extrema using the same verified DPP shuffle shape.
use tuxbox_xshader::{ShaderExtract, Stage, hlsl};
mod fixture {
    include!(concat!(env!("CARGO_MANIFEST_DIR"), "/examples/support/wave_guard_fixture.rs"));
}
fn main() {
    let args: Vec<_> = std::env::args().collect();
    let width: u32 = args[1].parse().expect("wave width");
    assert!(matches!(width, 32 | 64));
    let shader = fixture::extrema(width, &args[2]);
    let out = hlsl::translate(&shader).expect("translate native extrema");
    assert_eq!(out.required_wave_size, (width==32).then_some(32));
    print!("{}",out.hlsl);
}
