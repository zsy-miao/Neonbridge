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
mod tests {
    use super::*;

    fn file() -> Vec<u8> {
        let words = [
            u32::from_le_bytes(*b"FSPX"),
            1,
            2,
            108,
            6,
            76,
            32,
            0, // Deliberately place code first in the directory.
            0,
            48,
            28,
            0,
            1,
            2,
            3,
            4,
            5,
            6,
            7,
            0xbf85_0001,
            0xbf81_0000,
            0xbe80_03ff,
            0xbf81_0000, // an endpgm literal
            0xbf81_0000,
            0xbf9f_0000,
            0x1a01,
            0xbf9f_0000,
        ];
        words.iter().flat_map(|word| word.to_le_bytes()).collect()
    }

    #[test]
    fn extracts_code_and_preserves_the_complete_native_section() {
        let bytes = file();
        let program = extract(&bytes, [32, 8, 1]).unwrap();
        assert_eq!(program.isa, bytes[76..96]);
        assert_eq!(program.isa_data, bytes[76..108]);
        assert_eq!(program.isa_preamble, [0, 32, 8, 1]);
        assert_eq!(program.fspx_config, Some([1, 2, 3, 4, 5, 6, 7]));
    }

    #[test]
    fn rejects_damaged_directory_code_and_configuration() {
        let bytes = file();
        for n in 0..bytes.len() {
            assert!(extract(&bytes[..n], [1; 3]).is_err());
        }
        for (at, bad) in [
            (0, 0u32),
            (4, 2),
            (8, u32::MAX),
            (12, 40),
            (16, 5),
            (20, 32),
            (20, 72),
            (20, u32::MAX),
            (24, 31),
            (32, 6),
            (40, 24),
            (76, 0xbf82_7fff),
            (76, 0xbf82_0004),
        ] {
            let mut damaged = bytes.clone();
            damaged[at..at + 4].copy_from_slice(&bad.to_le_bytes());
            assert!(extract(&damaged, [1; 3]).is_err(), "field {at} value {bad}");
        }
    }
}
