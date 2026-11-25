use super::{pixel_depth_before_shader, pixel_system_vgpr, position_vgprs, PixelSystemInput};

#[test]
fn only_explicit_color_depth_order_suppresses_fragment_writes() {
    let mut fssm = [0; 0x400];
    fssm[..8].copy_from_slice(b"FSSM\x01\x01\0\0");
    fssm[16..24].copy_from_slice(&6u64.to_le_bytes());
    fssm[0x40..0x48].copy_from_slice(&0xc0u64.to_le_bytes());
    for (control, expected) in [(0x10u32, false), (0x240, false),
                               (0x1010, true), (0x1410, true), (0x1450, true),
                               (0x1610, false), (0x1411, false)] {
        fssm[0x25c..0x260].copy_from_slice(&control.to_le_bytes());
        assert_eq!(pixel_depth_before_shader(&fssm), expected);
    }
    assert!(!pixel_depth_before_shader(&fssm[..0x25e]));
}

#[test]
fn sparse_fragment_position_follows_enabled_barycentrics() {
    let mut fssm = [0; 0x24c];
    fssm[..8].copy_from_slice(b"FSSM\x01\x01\0\0");
    for (mask, read, slots) in [
        (0x402u32, 4, Some([None, None, Some(2), None])),
        (0x702, 7, Some([Some(2), Some(3), Some(4), None])),
        (0x528, 5, Some([Some(5), None, Some(6), None])),
        (0x402, 3, None),
        (0xb02, 11, Some([Some(2), Some(3), None, Some(4)])),
        (0xc02, 12, Some([None, None, Some(2), Some(3)])),
        (0x1f02, 15, Some([Some(2), Some(3), Some(4), Some(5)])),
        (0x20, 0, None),
    ] {
        fssm[0x248..].copy_from_slice(&mask.to_le_bytes());
        assert_eq!(position_vgprs(&fssm, read), slots);
    }
}

#[test]
fn ancillary_location_follows_enabled_inputs() {
    let mut fssm = [0; 0x24c];
    fssm[..8].copy_from_slice(b"FSSM\x01\x01\0\0");
    for (mask, slot) in [(0x2020u32, Some(2)), (0x2302, Some(4)),
                         (0x3fff, Some(21)), (0x0302, None)] {
        fssm[0x248..].copy_from_slice(&mask.to_le_bytes());
        assert_eq!(pixel_system_vgpr(&fssm, PixelSystemInput::Ancillary), slot);
    }
    assert_eq!(pixel_system_vgpr(&fssm[..24], PixelSystemInput::Ancillary), None);
}

#[test]
fn front_face_follows_only_enabled_position_components() {
    let mut fssm = [0; 0x24c];
    fssm[..8].copy_from_slice(b"FSSM\x01\x01\0\0");
    for (mask, slot) in [(0x1302u32, Some(4)), (0x1702, Some(5)),
                         (0x1f02, Some(6)), (0x3000, Some(0)), (0x0f02, None)] {
        fssm[0x248..].copy_from_slice(&mask.to_le_bytes());
        assert_eq!(pixel_system_vgpr(&fssm, PixelSystemInput::FrontFace), slot);
    }
}
