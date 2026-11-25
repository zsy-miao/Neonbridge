//! Rewrites of the instruction list made before translation.
use super::*;

mod quad;
mod reductions;
mod lanes;
mod election;
pub(in crate::hlsl) use quad::*;
pub(in crate::hlsl) use reductions::*;
pub(in crate::hlsl) use lanes::*;
pub(in crate::hlsl) use election::*;
