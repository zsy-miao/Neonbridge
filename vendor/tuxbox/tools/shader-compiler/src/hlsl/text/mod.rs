//! Spelling HLSL: operand names, small expressions and fixed helper code.
use super::*;

mod operands;
mod helper_code;
pub(in crate::hlsl) use operands::*;
pub(in crate::hlsl) use helper_code::*;
