//! Immediate samplers: the console's four sampler words (the GPU's public
//! `SQ_IMG_SAMP_WORD0-3` layout) as a D3D12 static sampler.
//!
//! Word 0: address modes x, y, z (bits 0-8, 3 each), log2 of the maximum
//! anisotropy (9-11), comparison function (12-14), reduction (29-30: 0
//! filtering, 1 minimum, 2 maximum). Word 1: minimum and maximum LOD
//! (0-11, 12-23, unsigned 4.8). Word 2: LOD bias (0-13, signed 5.8),
//! magnification and minification filters (20-21, 22-23: point, linear,
//! anisotropic point, anisotropic linear), mip filter (26-27: none, point,
//! linear). Word 3: border colour (30-31: transparent black, opaque black,
//! opaque white, custom).
//!
//! Comparison is the sampling instruction's, not the sampler's: the console
//! encodes a comparison function in every sampler (D3D's default, less or
//! equal, when unused).

/// A static sampler in D3D12 terms; the integers are the `D3D12_FILTER`,
/// `D3D12_TEXTURE_ADDRESS_MODE`, `D3D12_COMPARISON_FUNC` and
/// `D3D12_STATIC_BORDER_COLOR` values.
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct StaticSampler {
    pub filter: u32,
    /// U, V, W.
    pub address: [u32; 3],
    pub mip_lod_bias: f32,
    pub max_anisotropy: u32,
    pub comparison: u32,
    pub border: u32,
    pub min_lod: f32,
    pub max_lod: f32,
}

// The floats come from fixed-point fields: never NaN.
impl Eq for StaticSampler {}

impl StaticSampler {
    /// The filter supported by the host device. Anisotropic point-mip
    /// filtering is optional (`D3D12_OPTIONS19`); older devices retain the
    /// previous linear-mip approximation. Comparison/min/max reduction
    /// bits and the sampler's LOD bounds are unchanged.
    pub fn filter_for_host(&self, aniso_point_mip_supported: bool) -> u32 {
        if !aniso_point_mip_supported && self.filter & !0x180 == 0x54 {
            self.filter | 1
        } else {
            self.filter
        }
    }

    /// Decodes the four sampler words.
    pub fn from_words(w: [u32; 4]) -> Self {
        let bits = |word: u32, at: u32, n: u32| word >> at & ((1 << n) - 1);
        // wrap, mirror, clamp, mirror once; the half-border modes have no
        // D3D form and become their nearest.
        let address = |m: u32| [1, 2, 3, 5, 3, 5, 4, 5][m as usize];
        let (mag, min, mip) = (bits(w[2], 20, 2), bits(w[2], 22, 2), bits(w[2], 26, 2));
        let reduction = match bits(w[0], 29, 2) {
            1 => 2,
            2 => 3,
            _ => 0,
        };
        let filter = if mag >= 2 || min >= 2 {
            0x54 | u32::from(mip == 2) | reduction << 7
        } else {
            (min & 1) << 4 | (mag & 1) << 2 | u32::from(mip == 2) | reduction << 7
        };
        let lod = |v: u32| v as f32 / 256.0;
        // Without a mip filter the base level is sampled.
        let (min_lod, max_lod) = if mip == 0 {
            (0.0, 0.0)
        } else {
            (lod(bits(w[1], 0, 12)), lod(bits(w[1], 12, 12)))
        };
        StaticSampler {
            filter,
            address: [0, 3, 6].map(|at| address(bits(w[0], at, 3))),
            mip_lod_bias: ((bits(w[2], 0, 14) << 18) as i32 >> 18) as f32 / 256.0,
            max_anisotropy: 1 << bits(w[0], 9, 3).min(4),
            comparison: bits(w[0], 12, 3) + 1,
            // shortcut: a custom border colour (type 3, a table entry) becomes
            // transparent black; upgrade when a title uses one.
            border: bits(w[3], 30, 2) % 3,
            min_lod,
            max_lod,
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn console_compiled_samplers_decode() {
        // Words the console compiler wrote for static samplers of known
        // descriptions.
        let point = StaticSampler::from_words([0x3088, 0x0054_0100, 0x1400_0080, 0x8000_0000]);
        assert_eq!(
            point,
            StaticSampler {
                filter: 0,
                address: [1, 2, 3],
                mip_lod_bias: 0.5,
                max_anisotropy: 1,
                comparison: 4,
                border: 2,
                min_lod: 1.0,
                max_lod: 5.25,
            }
        );
        let aniso = StaticSampler::from_words([0x369E, 0x00FF_F000, 0x18F0_3E00, 0x8000_0000]);
        assert_eq!(
            (aniso.filter, aniso.address, aniso.max_anisotropy),
            (0x55, [4, 5, 3], 8)
        );
        assert_eq!((aniso.mip_lod_bias, aniso.max_lod), (-2.0, 4095.0 / 256.0));
        let compare = StaticSampler::from_words([0x3006, 0x00FF_F000, 0x1450_0000, 0x4000_0000]);
        assert_eq!(
            (compare.filter, compare.address, compare.border),
            (0x14, [4, 1, 1], 1)
        );
        let minimum =
            StaticSampler::from_words([0x2000_3003, 0x00FF_F000, 0x1850_0000, 0x8000_0000]);
        assert_eq!((minimum.filter, minimum.address[0]), (0x115, 5));
        let mag_mip = StaticSampler::from_words([0x3002, 0, 0x1810_0000, 0x8000_0000]);
        assert_eq!((mag_mip.filter, mag_mip.max_lod), (0x05, 0.0));
        let max_aniso =
            StaticSampler::from_words([0x4000_3800, 0x00FF_F000, 0x18F0_0F00, 0x8000_0000]);
        assert_eq!(
            (
                max_aniso.filter,
                max_aniso.max_anisotropy,
                max_aniso.mip_lod_bias
            ),
            (0x1D5, 16, 15.0)
        );
    }

    #[test]
    fn anisotropic_samplers_preserve_mip_point_filtering() {
        for (reduction, base) in [(0u32, 0x54u32), (1, 0x154), (2, 0x1D4)] {
            let w0 = reduction << 29;
            for mip in [0u32, 1] {
                let w2 = (mip << 26) | 0x00F0_0000; // mag = 3, min = 3
                let s = StaticSampler::from_words([w0, 0x00FF_F000, w2, 0]);
                assert_eq!(s.filter, base, "reduction={reduction} mip={mip}");
            }
            let w2 = (2 << 26) | 0x00F0_0000; // mip = 2 (linear)
            let s = StaticSampler::from_words([w0, 0x00FF_F000, w2, 0]);
            assert_eq!(s.filter, base | 1, "reduction={reduction} mip=2");
        }
        // Mixed anisotropic mag (2) with linear min (1) and point mip (1).
        let mixed = StaticSampler::from_words([0, 0, (1 << 26) | (1 << 22) | (2 << 20), 0]);
        assert_eq!(mixed.filter, 0x54);
    }

    #[test]
    fn host_filter_falls_back_without_changing_reduction_or_lod() {
        for (filter, fallback) in [(0x54, 0x55), (0xD4, 0xD5), (0x154, 0x155), (0x1D4, 0x1D5)] {
            for mip in [0u32, 1] {
                let s = StaticSampler {
                    filter,
                    ..StaticSampler::from_words([0, 0x0054_0100, (mip << 26) | 0x00F0_0000, 0])
                };
                assert_eq!(s.filter_for_host(false), fallback);
                assert_eq!(s.filter_for_host(true), filter);
                assert_eq!(
                    (s.min_lod, s.max_lod),
                    if mip == 0 { (0.0, 0.0) } else { (1.0, 5.25) }
                );
            }
        }
    }

    #[test]
    fn host_filter_keeps_other_filters_unchanged() {
        for reduction in [0, 0x80, 0x100, 0x180] {
            for base in [0, 1, 4, 5, 0x10, 0x11, 0x14, 0x15, 0x55] {
                let s = StaticSampler {
                    filter: base | reduction,
                    ..StaticSampler::from_words([0; 4])
                };
                assert_eq!(s.filter_for_host(false), s.filter);
                assert_eq!(s.filter_for_host(true), s.filter);
            }
        }
    }
}
