//! Xbox Series shaders to host shaders.
//!
//! An Xbox Series shader is a standard DXBC container ([`dxbc`]). Its `DXIL`
//! part ([`dxil`]) may carry Xbox additions around the LLVM bitcode: an
//! `XDAT` table before it indexing blobs after it, among them `xbop`
//! (compiler options) and the zlib-compressed GFX10 code. The bitcode may use
//! console-only intrinsics, so the GFX10 code is translated in general.
//! Precompiled programs without a `DXIL` part keep the code in an `ACCC`
//! part instead ([`fspx`]).

pub mod bvh;
pub mod dxbc;
pub mod dxil;
pub mod emit;
pub mod fspx;
pub mod gfx10;
pub mod handoff;
pub mod io;
pub mod ir;
pub mod merged;
pub mod native;
pub mod resource;
pub mod spill;
pub mod structure;

/// Synthetic shader containers, for the tests of crates that use this one
/// (feature `synthetic`): programs written for the tests, not the title's.
#[cfg(feature = "synthetic")]
pub mod synthetic {
    /// The vertex container (ES half) and the geometry container (GS half,
    /// three emits) of a merged stage.
    pub fn merged_stage() -> (Vec<u8>, Vec<u8>) {
        (
            crate::emit::gs::tests::es_container(),
            crate::emit::gs::tests::gs_container(3),
        )
    }

    /// A pixel container whose program does nothing.
    pub fn pixel_container() -> Vec<u8> {
        crate::native::tests::container(&[(crate::native::HwStage::Ps, &0xBF81_0000u32.to_le_bytes())])
    }
}

pub use dxbc::{Container, FourCC, Part};
pub use dxil::{DxilProgram, ShaderKind, XboxExtras};
pub use native::{
    HwStage, NativeProgram, ResourceType, RootParam, RootType, UsedResource, native_programs,
    program_part,
};

#[derive(Debug, thiserror::Error, PartialEq, Eq)]
pub enum ShaderError {
    #[error("not a DXBC container")]
    NotDxbc,
    #[error("malformed shader: {0}")]
    Malformed(&'static str),
    #[error("GFX10 code at {offset:#x}: {reason}")]
    Isa { offset: usize, reason: &'static str },
    /// Valid code the translator cannot handle yet: an opcode name or a construct.
    #[error("GFX10 code at {offset:#x}: {what} is not supported")]
    Unsupported { offset: usize, what: &'static str },
}
