//! A direct HLSL -> DXIL writer, in progress, so shaders need not go through
//! dxc (test112: dxc was 98% of shader time, 401 s for 252 compiles).
//!
//! Ported from five-star-recomp's DXIL backend (GPL-3.0-or-later,
//! ThatRecompDev and the Five Star Recomp contributors,
//! src/FiveStar.Shaders/Dxil). Ideas only, no code, from xcl PR #1426
//! (GPL-2.0-only). Port order: the bitstream and container hash (here), then
//! the container parts, the IR and module writer, the HLSL front end and the
//! compute lowering. The launcher keeps dxc as the fallback.
pub mod bitstream;
pub mod container;
pub mod hash;
