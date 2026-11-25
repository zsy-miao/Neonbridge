//! TUXBOX_BVH_RAW_SRV (default on, =0 off), root form: a translated compute stage that
//! declares an SRV at t5.. in space32766 gets one raw root SRV per guest
//! root SRV parameter, in parameter order (k-th guest root SRV -> t(5+k)),
//! over the title memory at the address the title bound there, so the
//! exact node test (five-star-recomp rdna_bvh.hlsli, GPL-3.0, credit) reads
//! ByteAddressBuffer nodes at (ptr & ~7) << 3. A parameter whose address is
//! no placed acceleration structure reads the null sink. No length is
//! passed: the shader bounds its own reads. The heap form is views.rs
//! bvh_raw_srv.
pub(crate) const REGISTER: u32 = tuxbox_dxbc::reserved::BVH_RAW.register;
pub(crate) const SPACE: u32 = tuxbox_dxbc::reserved::BVH_RAW.space;

pub(crate) fn enabled() -> bool {
    crate::env_toggle!("TUXBOX_BVH_RAW_SRV") != Some("0")
}

/// The stage declares an SRV binding at t5 or above in space32766.
pub(crate) fn declared(code: &[u8]) -> bool {
    enabled()
        && super::null_kinds::bindings(code).iter().any(|&[resource, space, _lower, upper, _]| {
            matches!(resource, 3..=5) && space == SPACE && upper >= REGISTER
        })
}
