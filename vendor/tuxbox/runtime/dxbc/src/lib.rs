//! DirectX shader containers, signatures, and raster-stage linkage.

mod container;
#[cfg(feature = "native-bindings")]
pub mod rdna2;
#[cfg(feature = "native-bindings")]
pub mod fssm;
mod fspx;
#[cfg(feature = "native-bindings")]
pub mod xdat;
#[cfg(feature = "native-bindings")]
pub mod compute_table;
#[cfg(feature = "native-bindings")]
pub mod inline_sampler;
#[cfg(feature = "native-bindings")]
pub mod mesh_table;
mod raster;
pub mod ray;
pub mod rootsig;
mod signature;

pub use container::{Chunk, Dxbc};
pub use fspx::{Fspx, compute_threads, parse_fspx, shader_compute_threads, wave_lane_range};
pub use raster::{
    exports_stencil_reference, has_packing_mismatch, pixel_input_order, raster_outputs,
};
pub use signature::{SigElement, parse_signature, parse_signature_part};

#[cfg(test)]
#[path = "tests/lib.rs"]
mod tests;
