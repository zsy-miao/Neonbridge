//! Stand-ins for passes whose results the host cannot produce yet: the
//! title's own program, translated as it is, with every image store writing
//! one constant in every channel. The program keeps its bindings, dispatch
//! shape and the pixels it covers; only the values change.
//!
//! - Ambient light (`VICTORY_AMBIENT_STUB`): the screen-probe temporal
//!   resolve of `indirect_diffuse_lighting`, the last pass of the diffuse
//!   global illumination before the tiled lighting reads it (pass order
//!   measured on a world frame of the title). Its input comes from ray
//!   tracing, which does not run on the host yet, so everything the sun
//!   does not reach is black; a constant there is a floor of ambient light.
//! - Shadow masks (`VICTORY_SHADOWMASK_STUB`): the ray-traced distant
//!   shadow passes (`rtdirectionalshadows`), writing 1 (lit) so that a
//!   trace that finds nothing does not black out the sun.
//!
//! Each output gets a value in its own encoding ([`Kind`], by the output's descriptor slot): float, packed R9G9B9E5 light, or an integer mask.

use super::HostShader;
use crate::ShaderError;

/// `indirect_diffuse_lighting_CS_ScreenProbes_TemporalResolve*` (with and
/// without reflective caustics and the constant-buffer variant).
pub const AMBIENT_HASHES: &[&str] = &[
    "2bed88a03a2f5558",
    "34fb51b8a0312e82",
    "5053352ac4370ca1",
    "2fc5d11ece80c884",
    "3b4f6eb5edb79848",
    "cf76630573822d04",
];

/// The outputs of the ambient stand-in in their own encodings, by descriptor
/// slot ([`Plan::with_overrides`]); what is not named is a float. Measured on
/// a dispatch of `34fb51b8a0312e82` (the same layout as `2bed88a03a2f5558`,
/// whose table has the same 11 outputs; the other variants have fewer or the
/// table shifted by 8 dwords: guesses, the log names the slots they store to):
/// 128 R16_FLOAT, 136 R8_UINT, 144 R8G8_UNORM, 152..=184 and 200 R32_UINT
/// (the light the title packs as R9G9B9E5 and the lighting reads back through
/// placed R9G9B9E5 views), 192 R10G10B10A2_UINT, 208 R16_UINT (the 640x360
/// target the lighting reads).
pub fn ambient_slots(hash: &str) -> &'static str {
    match hash {
        "3b4f6eb5edb79848" | "cf76630573822d04" => {
            "144=mask,160=rgb9e5,168=rgb9e5,176=rgb9e5,184=rgb9e5,192=rgb9e5,208=rgb9e5,200=zero,216=mask"
        }
        _ => "136=mask,152=rgb9e5,160=rgb9e5,168=rgb9e5,176=rgb9e5,184=rgb9e5,200=rgb9e5,192=zero,208=mask",
    }
}

/// The shadow-mask stand-in has float outputs only, as far as measured.
pub const SHADOWMASK_SLOTS: &str = "";

/// The ambient floor written by default.
pub const AMBIENT_VALUE: f32 = 0.1;

/// `rtdirectionalshadows_CS_RTDistantShadowTrace*` (trace, leak plug, and
/// their temporal upsamples).
pub const SHADOWMASK_HASHES: &[&str] = &[
    "a0544eac81df1fc8",
    "a1b20bc47e3c9597",
    "ff6a098566877642",
    "e8c962358d862e3f",
    "d4c32d1ea3b676be",
    "17829ac71aab4e8d",
    "ba263b6ab4b6d619",
    "afeea48bbb9913ad",
];

/// Lit.
pub const SHADOWMASK_VALUE: f32 = 1.0;

/// What an image store of a stand-in writes into one output, by the
/// output's own format: the console's store takes the data as the view's
/// format wants it (the title's code packs integers and light itself), so
/// one float in every channel is only right for float outputs.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Kind {
    /// The value as a float, in every channel (float, unorm and snorm
    /// outputs).
    Float,
    /// The value as grey R9G9B9E5 light ([`rgb9e5`]), in every channel: the
    /// R32_UINT outputs the title reads back as irradiance.
    Rgb9e5,
    /// All bits set: an unsigned integer output at its maximum, whatever
    /// its width (a mask: no occlusion, valid history).
    Mask,
    /// Zero bits.
    Zero,
    /// These bits in every channel.
    Bits(u32),
}

impl Kind {
    /// `f`, `rgb9e5`, `mask`, `zero` or an integer (`0x..` too).
    pub fn parse(s: &str) -> Option<Kind> {
        let s = s.trim().to_ascii_lowercase();
        Some(match s.as_str() {
            "f" | "float" => Kind::Float,
            "rgb9e5" | "e5" => Kind::Rgb9e5,
            "mask" | "ones" => Kind::Mask,
            "zero" | "0" => Kind::Zero,
            _ => Kind::Bits(match s.strip_prefix("0x") {
                Some(h) => u32::from_str_radix(h, 16).ok()?,
                None => s.parse().ok()?,
            }),
        })
    }
}

/// How the stores of one stand-in are filled.
#[derive(Clone, Debug)]
pub struct Plan {
    /// The linear value: floats, and the grey of RGB9E5 outputs.
    pub value: f32,
    /// What an output gets that `slots` does not name.
    pub default: Kind,
    /// By the output's descriptor: the dword offset of its descriptor in
    /// the table the program loads it from (`s_load` offset in dwords plus
    /// the dword of the load), as the title's dispatch dump names it
    /// (`Uav s1+128`).
    pub slots: Vec<(u32, Kind)>,
    /// The outputs seen while translating (offset or none), in order:
    /// filled by the translation.
    pub seen: Vec<Option<u32>>,
}

impl Plan {
    pub fn new(value: f32) -> Plan {
        Plan { value, default: Kind::Float, slots: Vec::new(), seen: Vec::new() }
    }

    /// `"128=f,152=rgb9e5,*=mask"`: kinds by slot offset, `*` the default.
    pub fn with_overrides(mut self, spec: &str) -> Plan {
        for part in spec.split(|c: char| c == ',' || c.is_whitespace()).filter(|p| !p.is_empty()) {
            let Some((k, v)) = part.split_once('=') else { continue };
            let Some(kind) = Kind::parse(v) else { continue };
            match k.trim() {
                "*" | "default" => self.default = kind,
                slot => {
                    if let Ok(slot) = slot.parse::<u32>() {
                        self.slots.retain(|s| s.0 != slot);
                        self.slots.push((slot, kind));
                    }
                }
            }
        }
        self
    }

    /// The four channels' bits a store to `slot` writes.
    pub fn bits(&mut self, slot: Option<u32>) -> [u32; 4] {
        self.seen.push(slot);
        let kind = slot
            .and_then(|s| self.slots.iter().find(|x| x.0 == s))
            .map_or(self.default, |x| x.1);
        let b = match kind {
            Kind::Float => self.value.to_bits(),
            Kind::Rgb9e5 => rgb9e5([self.value; 3]),
            Kind::Mask => u32::MAX,
            Kind::Zero => 0,
            Kind::Bits(b) => b,
        };
        [b; 4]
    }
}

/// `rgb` (linear, not negative) as R9G9B9E5 (shared exponent, bias 15, nine
/// mantissa bits): 0.25 grey is 0x74020100.
pub fn rgb9e5(rgb: [f32; 3]) -> u32 {
    const MAX: f32 = 65408.0; // (511/512) * 2^16
    let c = rgb.map(|x| if x.is_nan() || x < 0.0 { 0.0 } else { x.min(MAX) });
    let top = c[0].max(c[1]).max(c[2]);
    let mut exp = (top.log2().floor().max(-16.0) as i32) + 1 + 15;
    let scale = |e: i32| 2f32.powi(e - 15 - 9);
    if (top / scale(exp) + 0.5).floor() as u32 == 512 {
        exp += 1;
    }
    let s = scale(exp);
    let m = c.map(|x| ((x / s + 0.5).floor() as u32).min(511));
    (exp as u32) << 27 | m[2] << 18 | m[1] << 9 | m[0]
}

/// `container` translated with every image store writing `value`.
pub fn storing(container: &[u8], value: f32) -> Result<HostShader, ShaderError> {
    storing_plan(container, Plan::new(value)).map(|(s, _)| s)
}

/// `container` translated with the stores of `plan`; the plan comes back
/// with the outputs it saw.
pub fn storing_plan(container: &[u8], plan: Plan) -> Result<(HostShader, Plan), ShaderError> {
    super::lower::STORE_PLAN.with(|p| *p.borrow_mut() = Some(plan));
    let r = super::translate(container);
    let plan = super::lower::STORE_PLAN.with(|p| p.borrow_mut().take());
    r.map(|s| (s, plan.unwrap_or_else(|| Plan::new(0.0))))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn the_store_plan_is_this_translation_s_alone() {
        assert!(storing(b"not a container", 0.5).is_err());
        super::super::lower::STORE_PLAN.with(|p| assert!(p.borrow().is_none()));
    }

    #[test]
    fn rgb9e5_packs_as_the_title_reads_it() {
        assert_eq!(rgb9e5([0.25; 3]), 0x7402_0100);
        assert_eq!(rgb9e5([0.5; 3]), 0x7C02_0100);
        assert_eq!(rgb9e5([0.0; 3]), 0);
        // 0.1 grey: exponent 12, 410 for each
        assert_eq!(rgb9e5([0.1; 3]), 0x666B_359A);
        // a warm colour keeps its ratios
        let w = rgb9e5([0.30, 0.22, 0.15]);
        assert_eq!(w, 0x7269_C333);
        // the largest value saturates
        assert_eq!(rgb9e5([1e9; 3]) >> 27, 31);
    }

    #[test]
    fn kinds_by_output_and_overrides() {
        let mut p = Plan::new(0.25).with_overrides("128=rgb9e5, 136=mask,152=0x10,*=zero");
        assert_eq!(p.bits(Some(128)), [0x7402_0100; 4]);
        assert_eq!(p.bits(Some(136)), [u32::MAX; 4]);
        assert_eq!(p.bits(Some(152)), [0x10; 4]);
        assert_eq!(p.bits(Some(999)), [0; 4]);
        assert_eq!(p.bits(None), [0; 4]);
        let mut q = Plan::new(0.25);
        assert_eq!(q.bits(Some(1)), [0.25f32.to_bits(); 4]);
        assert_eq!(q.seen, vec![Some(1)]);
        assert_eq!(Kind::parse("nonsense"), None);
    }
}
