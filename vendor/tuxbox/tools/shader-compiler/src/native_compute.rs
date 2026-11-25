//! FSPX native compute sections observed in the Scarlett FidelityFX upscaler.
//! Version 1 stores a directory immediately after its four-word header. Section
//! addresses are absolute byte offsets from the first byte of the FSPX record.

use super::fssm::executable_extent;
use super::{NativeProgram, Stage};

pub(super) fn extract(body: &[u8], threads: [u32; 3]) -> Result<NativeProgram, String> {
    let tuxbox_dxbc::Fspx { config, code } = tuxbox_dxbc::parse_fspx(body)?;
    let isa_bytes = executable_extent(code)?;
    Ok(NativeProgram {
        stage: Stage::Compute,
        sm: (0, 0),
        bitcode_stripped: false,
        fssm: Vec::new(),
        isa: code[..isa_bytes].to_vec(),
        isa_off: 0,
        isa_preamble: [0, threads[0], threads[1], threads[2]],
        isa_data: code.to_vec(),
        fspx_config: config,
        entry: None,
    })
}

#[cfg(test)]
#[path = "tests/native_compute.rs"]
mod tests;
