//! SetSampleMaskX's quad mask for translated pixel shaders (test145: six
//! masks at guest RVA 0x329e9, every 16-bit lane 0x0000 or 0xffff, so the
//! title keeps or kills whole pixels by their place in a 2x2 quad, which
//! D3D12's sample mask cannot say). A pixel shader declaring
//! `cbuffer tb_quad_mask : register(b0, space32766) { uint lo; uint hi; }`
//! gets two root constants: the mask's low and high words, 0xffffffff both
//! when none is set. Lane p = ((y & 1) << 1) | (x & 1) from SV_Position is
//! bits 16p..16p+15 of hi:lo; the shader discards when that lane is 0.
//! TUXBOX_QUAD_MASK=0 binds nothing (sample_mask ignores the mask again).

pub(crate) const REGISTER: u32 = 0;
pub(crate) const SPACE: u32 = 32766;

pub(crate) fn enabled() -> bool {
    static ON: std::sync::OnceLock<bool> = std::sync::OnceLock::new();
    *ON.get_or_init(|| crate::env_toggle!("TUXBOX_QUAD_MASK") != Some("0"))
}

/// Whether a translated stage declares tb_quad_mask (a CBV, PSV0 type 2).
pub(crate) fn declared(code: &[u8]) -> bool {
    enabled()
        && super::null_kinds::bindings(code).iter().any(|&[resource, space, lower, upper, _]| {
            resource == 2 && space == SPACE && (lower..=upper).contains(&REGISTER)
        })
}

/// The two root constants for `mask` (None: no mask set, every pixel kept).
pub(crate) fn words(mask: Option<u64>) -> [u32; 2] {
    let m = mask.unwrap_or(u64::MAX);
    [m as u32, (m >> 32) as u32]
}
