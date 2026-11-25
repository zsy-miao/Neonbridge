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
mod tests {
    use super::*;

    fn signature_container(tag: &[u8; 4], stride: usize, register: u32, mask: u8) -> Vec<u8> {
        let mut data = vec![0; 44 + 8 + stride];
        data[0..4].copy_from_slice(b"DXBC");
        data[28..32].copy_from_slice(&1u32.to_le_bytes());
        data[32..36].copy_from_slice(&36u32.to_le_bytes());
        data[36..40].copy_from_slice(tag);
        data[44..48].copy_from_slice(&1u32.to_le_bytes());
        data[48..52].copy_from_slice(&8u32.to_le_bytes());
        let name_field = if stride == 24 { 52 } else { 56 };
        data[name_field..name_field + 4].copy_from_slice(&((stride + 8) as u32).to_le_bytes());
        data[name_field + 16..name_field + 20].copy_from_slice(&register.to_le_bytes());
        data[name_field + 20] = mask;
        data.extend_from_slice(b"TEXCOORD\0");
        let len = data.len() as u32;
        data[24..28].copy_from_slice(&len.to_le_bytes());
        data[40..44].copy_from_slice(&(len - 44).to_le_bytes());
        data
    }

    #[test]
    fn checks_legacy_and_modern_signature_packing() {
        let pixel = signature_container(b"ISGN", 24, 3, 2);
        assert_eq!(pixel_input_order(&pixel), ["TEXCOORD0"]);
        for (tag, stride) in [(b"OSGN", 24), (b"OSG5", 28), (b"OSG1", 32)] {
            let matching = signature_container(tag, stride, 3, 3);
            assert!(!has_packing_mismatch(&matching, &pixel));
            assert!(has_packing_mismatch(
                &signature_container(tag, stride, 2, 3),
                &pixel
            ));
            assert!(has_packing_mismatch(
                &signature_container(tag, stride, 3, 1),
                &pixel
            ));
        }
    }

    #[test]
    fn chunk_directory_and_payload_must_fit_declared_container() {
        let valid = signature_container(b"ISG1", 32, 0, 1);
        let mut padded = valid.clone();
        padded.extend_from_slice(&[0xff; 16]);
        assert_eq!(
            Dxbc::parse(&padded).unwrap().total_size as usize,
            valid.len()
        );
        for (offset, value) in [
            (24, 24),
            (24, 32),
            (24, valid.len() as u32 - 1),
            (24, u32::MAX),
            (28, u32::MAX),
            (32, 28),
            (32, u32::MAX),
            (40, u32::MAX),
        ] {
            let mut bad = valid.clone();
            bad[offset..offset + 4].copy_from_slice(&value.to_le_bytes());
            assert!(Dxbc::parse(&bad).is_err(), "offset {offset}, value {value}");
        }
    }

    #[test]
    fn rejects_truncated_records_and_invalid_names() {
        let shader = signature_container(b"ISG1", 32, 0, 1);
        let body = &shader[44..];
        assert_eq!(parse_signature(body).unwrap()[0].name, "TEXCOORD");
        for (offset, value) in [
            (0, u32::MAX),
            (4, 4),
            (4, u32::MAX),
            (12, 8),
            (12, u32::MAX),
        ] {
            let mut bad = body.to_vec();
            bad[offset..offset + 4].copy_from_slice(&value.to_le_bytes());
            assert!(
                parse_signature(&bad).is_err(),
                "offset {offset}, value {value}"
            );
        }
        assert!(parse_signature(&body[..body.len() - 1]).is_err());
        assert!(parse_signature(&body[..39]).is_err());
    }
}
