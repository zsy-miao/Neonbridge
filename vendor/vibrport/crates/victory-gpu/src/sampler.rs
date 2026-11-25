//! Samplers as the host describes them, from the console's sampler
//! description (`CreateSamplerX`), which names the hardware's sampler
//! fields. (Immediate samplers in shader code come decoded with the
//! translation.)

use windows::Win32::Graphics::Direct3D12::*;

/// The fields both forms share.
#[derive(Debug, Clone, Copy, PartialEq)]
struct Fields {
    /// Point 0, bilinear 1, anisotropic point 2, anisotropic bilinear 3.
    mag: u32,
    min: u32,
    /// None 0, point 1, linear 2.
    mip: u32,
    /// Plain 0, minimum 1, maximum 2.
    mode: u32,
    /// Host address modes.
    address: [u32; 3],
    anisotropy: u32,
    /// Host comparison function.
    compare: u32,
    min_lod: f32,
    max_lod: f32,
    bias: f32,
    border: [f32; 4],
}

impl Fields {
    fn host(&self) -> D3D12_SAMPLER_DESC {
        let linear = |f: u32| u32::from(f == 1 || f == 3);
        let aniso = self.mag >= 2 || self.min >= 2;
        // The console compares when the instruction says so, with the
        // sampler's function; the host needs a comparison sampler for that.
        // A sampler given a real test (less ... greater-or-equal) is one;
        // never, always and none are plain samplers.
        const LESS: u32 = 2;
        const GREATER_EQUAL: u32 = 7;
        // The host takes its own comparison functions (never 1 ... always 8)
        // and address modes (wrap 1 ... mirror once 5), and rejects a
        // description naming anything else; the words come from the title,
        // so clamp them into range rather than lose the whole sampler.
        let compare = self.compare.clamp(1, 8);
        let compares = (LESS..=GREATER_EQUAL).contains(&compare);
        let address = |i: usize| D3D12_TEXTURE_ADDRESS_MODE(self.address[i].clamp(1, 5) as i32);
        let reduction = match self.mode {
            1 => 2,
            2 => 3,
            _ => u32::from(compares),
        };
        let filter = if aniso {
            0x55
        } else {
            linear(self.min) << 4 | linear(self.mag) << 2 | u32::from(self.mip == 2)
        };
        D3D12_SAMPLER_DESC {
            Filter: D3D12_FILTER((reduction << 7 | filter) as i32),
            AddressU: address(0),
            AddressV: address(1),
            AddressW: address(2),
            MipLODBias: self.bias,
            MaxAnisotropy: if aniso {
                self.anisotropy.clamp(1, 16)
            } else {
                1
            },
            ComparisonFunc: D3D12_COMPARISON_FUNC(compare as i32),
            BorderColor: self.border,
            MinLOD: self.min_lod,
            // Without a mip filter the console samples mip 0 only.
            MaxLOD: if self.mip == 0 {
                self.min_lod
            } else {
                self.max_lod
            },
        }
    }
}

/// The console's sampler description (27 words): flags, magnification,
/// minification, mip and depth filters, filter mode, three address modes,
/// minimum and maximum LOD, LOD bias, secondary bias, anisotropy, its bias
/// and threshold, comparison, border color, then tuning words.
pub(crate) fn console(words: &[u32; 27]) -> D3D12_SAMPLER_DESC {
    let f = |i: usize| f32::from_bits(words[i]);
    Fields {
        mag: words[1],
        min: words[2],
        mip: words[3],
        mode: words[5],
        address: [words[6], words[7], words[8]],
        min_lod: f(9),
        max_lod: f(10),
        bias: f(11),
        anisotropy: words[13],
        compare: words[16],
        border: [f(17), f(18), f(19), f(20)],
    }
    .host()
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn console_descriptions_decode() {
        let mut w = [0u32; 27];
        w[1] = 1; // bilinear magnification
        w[2] = 0; // point minification
        w[3] = 2; // linear mips
        w[6..9].copy_from_slice(&[3, 3, 3]);
        w[10] = 1000.0f32.to_bits();
        let d = console(&w);
        assert_eq!(d.Filter.0, 0x5, "min point, mag and mip linear");
        assert_eq!(d.AddressW, D3D12_TEXTURE_ADDRESS_MODE_CLAMP);
        assert_eq!(d.MaxLOD, 1000.0);
        // A less-or-equal test makes a comparison sampler; never does not.
        w[16] = 4;
        assert_eq!(console(&w).Filter.0, 0x85);
        assert_eq!(console(&w).ComparisonFunc, D3D12_COMPARISON_FUNC_LESS_EQUAL);
        w[16] = 1;
        assert_eq!(console(&w).Filter.0, 0x5);
    }

    /// A description with wrap addressing and nothing else set.
    fn description() -> [u32; 27] {
        let mut w = [0u32; 27];
        w[6..9].fill(1);
        w
    }

    #[test]
    fn filters_follow_the_console_fields() {
        let mut w = description();
        // Point minification, magnification and mips is the host's zero.
        assert_eq!(console(&w).Filter.0, 0x00);
        w[1] = 1; // bilinear magnification
        assert_eq!(console(&w).Filter.0, 0x04);
        w[2] = 1; // bilinear minification
        assert_eq!(console(&w).Filter.0, 0x14);
        w[3] = 1; // point mips stay point
        assert_eq!(console(&w).Filter.0, 0x14);
        w[3] = 2; // linear mips
        assert_eq!(console(&w).Filter.0, 0x15);
        // Anisotropic point and anisotropic bilinear both become the host's
        // anisotropic filter, which is linear in every direction.
        for (mag, min) in [(2, 0), (3, 0), (0, 2), (3, 3)] {
            w[1] = mag;
            w[2] = min;
            assert_eq!(console(&w).Filter.0, 0x55, "{mag}/{min}");
        }
    }

    #[test]
    fn comparison_samplers_use_the_host_reduction() {
        let mut w = description();
        // Never (1), none (0) and always (8) are not tests.
        for compare in [0, 1, 8] {
            w[16] = compare;
            let d = console(&w);
            assert_eq!(d.Filter.0, 0, "compare {compare}");
            assert_eq!(
                d.ComparisonFunc,
                D3D12_COMPARISON_FUNC(compare.max(1) as i32)
            );
        }
        // Every real test is, with the host's comparison reduction.
        for compare in 2..=7 {
            w[16] = compare;
            let d = console(&w);
            assert_eq!(d.Filter.0, 0x80, "compare {compare}");
            assert_eq!(d.ComparisonFunc, D3D12_COMPARISON_FUNC(compare as i32));
        }
        // Comparison and anisotropic add up to the host's comparison
        // anisotropic filter.
        w[1] = 2;
        w[16] = 4;
        assert_eq!(console(&w).Filter.0, 0xD5);
        w[1] = 0;
        // The console's minimum and maximum reductions replace it, even
        // when a test is set.
        w[5] = 1;
        assert_eq!(console(&w).Filter.0, 0x100);
        w[5] = 2;
        assert_eq!(console(&w).Filter.0, 0x180);
        w[16] = 0;
        assert_eq!(console(&w).Filter.0, 0x180);
    }

    #[test]
    fn comparison_function_stays_in_the_host_range() {
        let mut w = description();
        // A zeroed word means "no test": the host's never.
        w[16] = 0;
        assert_eq!(console(&w).ComparisonFunc, D3D12_COMPARISON_FUNC_NEVER);
        // Words past always are not host functions; clamp them.
        for compare in [9, 15, u32::MAX] {
            w[16] = compare;
            let d = console(&w);
            assert_eq!(d.Filter.0, 0, "not a real test");
            assert_eq!(
                d.ComparisonFunc, D3D12_COMPARISON_FUNC_ALWAYS,
                "{compare:#x}"
            );
        }
    }

    #[test]
    fn anisotropy_is_clamped_to_the_host_range() {
        let mut w = description();
        w[1] = 3; // anisotropic bilinear
        for (word, expected) in [(0, 1), (1, 1), (3, 3), (16, 16), (17, 16), (u32::MAX, 16)] {
            w[13] = word;
            assert_eq!(console(&w).MaxAnisotropy, expected, "{word}");
        }
        // Point and bilinear filters have no anisotropy at all.
        w[1] = 1;
        w[13] = 8;
        assert_eq!(console(&w).MaxAnisotropy, 1);
    }

    #[test]
    fn lod_bounds_bias_and_border_reach_the_description() {
        let mut w = description();
        w[3] = 2; // linear mips
        w[9] = 1.0f32.to_bits();
        w[10] = 6.5f32.to_bits();
        w[11] = (-2.5f32).to_bits();
        w[17] = 1.0f32.to_bits();
        w[18] = 0.5f32.to_bits();
        w[19] = 0.25f32.to_bits();
        let d = console(&w);
        assert_eq!((d.MinLOD, d.MaxLOD), (1.0, 6.5));
        assert_eq!(d.MipLODBias, -2.5);
        assert_eq!(d.BorderColor, [1.0, 0.5, 0.25, 0.0]);
        // Without a mip filter one level is sampled, whatever the bounds.
        w[3] = 0;
        let d = console(&w);
        assert_eq!((d.MinLOD, d.MaxLOD), (1.0, 1.0));
    }

    #[test]
    fn address_modes_are_host_modes() {
        let mut w = description();
        w[6..9].copy_from_slice(&[1, 2, 3]);
        let d = console(&w);
        assert_eq!(
            (d.AddressU, d.AddressV, d.AddressW),
            (
                D3D12_TEXTURE_ADDRESS_MODE_WRAP,
                D3D12_TEXTURE_ADDRESS_MODE_MIRROR,
                D3D12_TEXTURE_ADDRESS_MODE_CLAMP,
            )
        );
        // The rest of the host's modes pass through.
        for (word, mode) in [
            (4, D3D12_TEXTURE_ADDRESS_MODE_BORDER),
            (5, D3D12_TEXTURE_ADDRESS_MODE_MIRROR_ONCE),
        ] {
            w[6..9].fill(word);
            let d = console(&w);
            assert_eq!((d.AddressU, d.AddressV, d.AddressW), (mode, mode, mode));
        }
        // Anything else is not a host mode; clamp it into range instead of
        // making the host reject the description.
        for (word, mode) in [
            (0, D3D12_TEXTURE_ADDRESS_MODE_WRAP),
            (6, D3D12_TEXTURE_ADDRESS_MODE_MIRROR_ONCE),
            (u32::MAX, D3D12_TEXTURE_ADDRESS_MODE_MIRROR_ONCE),
        ] {
            w[6..9].fill(word);
            assert_eq!(console(&w).AddressU, mode, "{word:#x}");
        }
    }
}
