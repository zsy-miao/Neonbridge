//! Per-lane GFX10 vector ALU instructions used by native ray continuations.
//! Register contents and move/select operands are always raw 32-bit words.
use super::operand;
use crate::rdna2::{Enc, Insn, Opnd};

pub(super) const HLSL: &str = concat!(include_str!("../float_to_integer.hlsl"), r#"
uint tb_ray_ballot(bool active) { return WaveActiveBallot(active)[WaveGetLaneIndex() >> 5]; }
uint tb_ray_first_lane() { return (WaveGetLaneIndex() & ~31u) + firstbitlow(tb_ray_ballot(true)); }
float tb_ray_legacy_mul(float a, float b) {
    if ((asuint(a) & 0x7fffffffu) == 0u || (asuint(b) & 0x7fffffffu) == 0u) return 0.0;
    return a * b;
}
float tb_ray_mac(float a, float b, float c) {
    precise float product = a * b;
    precise float sum = product + c;
    return sum;
}
float tb_ray_legacy_mad(float a, float b, float c) {
    precise float product = tb_ray_legacy_mul(a, b);
    precise float sum = product + c;
    return sum;
}
float tb_ray_median(float a, float b, float c) {
    if (tb_nan_bits(asuint(a)) || tb_nan_bits(asuint(b)) || tb_nan_bits(asuint(c)))
        return min(min(a, b), c);
    return max(min(a, b), min(max(a, b), c));
}
float tb_ray_widen_half(uint bits) {
    // f16tof32 may canonicalize NaNs on desktop GPUs. Keep the sign and
    // payload while quieting signaling NaNs for IEEE-mode conversion.
    if ((bits & 0x7fffu) > 0x7c00u)
        return asfloat(((bits & 0x8000u) << 16u) | 0x7fc00000u | ((bits & 1023u) << 13u));
    return f16tof32(bits & 65535u);
}
uint tb_ray_half(float value) {
    uint bits = asuint(value);
    uint sign = (bits >> 16) & 0x8000u;
    int exponent = int((bits >> 23) & 255u) - 127;
    uint fraction = bits & 0x7fffffu;
    if (exponent == 128 && fraction != 0u) return sign | 0x7e00u | (fraction >> 13);
    if (exponent > 15) return sign | 0x7c00u;
    if (exponent < -25) return sign;
    uint mantissa = fraction | 0x800000u;
    uint shift = uint(max(13, -exponent - 1));
    uint high = mantissa >> shift;
    uint remainder = mantissa - (high << shift);
    uint mid = 1u << (shift - 1u);
    uint rounded = high + uint(remainder > mid || (remainder == mid && (high & 1u) != 0u));
    return sign | (uint(max(0, exponent + 14)) * 1024u + rounded);
}
uint tb_ray_mul24(uint a, uint b, bool signedOperands, bool highWord, bool clampResult) {
    a = signedOperands ? asuint(asint(a << 8) >> 8) : a & 0xffffffu;
    b = signedOperands ? asuint(asint(b << 8) >> 8) : b & 0xffffffu;
    uint low = a * b;
    if (!highWord && !clampResult) return low;

    // Split each operand into a low 16-bit digit and a signed or unsigned
    // upper byte. The cross products and their sum fit in a signed dword.
    int a0 = int(a & 65535u), b0 = int(b & 65535u);
    int a1 = asint(a) >> 16, b1 = asint(b) >> 16;
    int crossTerms = int((uint(a0) * uint(b0)) >> 16) + a1 * b0 + a0 * b1;
    int high = a1 * b1 + (crossTerms >> 16);
    if (highWord) return asuint(high);
    if (!signedOperands) return high == 0 ? low : 0xffffffffu;
    if (high == (asint(low) >> 31)) return low;
    return high < 0 ? 0x80000000u : 0x7fffffffu;
}
uint tb_ray_perm(uint a, uint b, uint selectors) {
    uint result = 0u;
    [unroll] for (uint i = 0u; i < 4u; ++i) {
        uint sel = (selectors >> (8u * i)) & 255u;
        uint byteIndex = sel < 8u ? sel : 2u * sel + 1u;
        uint word = (byteIndex & 4u) != 0u ? a : b;
        uint byteValue = (word >> (8u * (byteIndex & 3u))) & 255u;
        if (sel >= 8u) byteValue = sel >= 13u || (sel < 12u && byteValue >= 128u) ? 255u : 0u;
        result |= byteValue << (8u * i);
    }
    return result;
}
// Normalized face coordinates are computed later by the guest program.
// Preserve the signed major axis and the native Z, Y, X tie ordering.
float4 tb_ray_cube(float x, float y, float z) {
    if (abs(z) >= abs(x) && abs(z) >= abs(y))
        return float4(z < 0.0 ? -x : x, -y, 2.0 * z, z < 0.0 ? 5.0 : 4.0);
    if (abs(y) >= abs(x))
        return float4(x, y < 0.0 ? -z : z, 2.0 * y, y < 0.0 ? 3.0 : 2.0);
    return float4(x < 0.0 ? z : -z, -y, 2.0 * x, x < 0.0 ? 1.0 : 0.0);
}
uint tb_ray_bfe(uint x, uint offset, uint width) {
    uint shifted = x >> (offset & 31u);
    return width >= 32u ? shifted : shifted & ((1u << (width & 31u)) - 1u);
}
uint2 tb_ray_shift_left(uint2 pair, uint bits) {
    bits &= 63u;
    if (bits == 0u) return pair;
    if (bits >= 32u) return uint2(0u, pair.x << (bits - 32u));
    return uint2(pair.x << bits, (pair.y << bits) | (pair.x >> (32u - bits)));
}
uint2 tb_ray_shift_right(uint2 pair, uint bits) {
    bits &= 63u;
    if (bits == 0u) return pair;
    if (bits >= 32u) return uint2(pair.y >> (bits - 32u), 0u);
    return uint2((pair.x >> bits) | (pair.y << (32u - bits)), pair.y >> bits);
}
"#, include_str!("../fract_f32.hlsl"));

fn high_word(base: Opnd) -> Result<String, String> {
    match base {
        Opnd::Vgpr(n) if n < 127 => operand(&Opnd::Vgpr(n + 1)),
        Opnd::Sgpr(n) if n < 105 => operand(&Opnd::Sgpr(n + 1)),
        _ => Err("unsupported ray ALU register pair".into()),
    }
}

pub(super) fn sdwa_field(selector: u32) -> Option<(u32, u32)> {
    match selector {
        byte @ 0..=3 => Some((8, byte * 8)),
        word @ 4..=5 => Some((16, (word - 4) * 16)),
        6 => Some((32, 0)),
        _ => None,
    }
}

// Address and frame recognition may use the ordinary full-word semantics only
// when SDWA leaves the source words and destination unchanged in width.
pub(super) fn whole_sdwa_words(ins: &Insn) -> bool {
    ins.aux.sdwa.is_none_or(|ext| {
        let source_is_whole = |field: u32| matches!(field & 0x7f, 6 | 14);
        matches!((ext >> 8) & 0xff, 6 | 14 | 22)
            && source_is_whole(ext >> 16)
            && (ins.src.len() < 2 || source_is_whole(ext >> 24))
    })
}

fn sdwa_word(word: &str, field: u32) -> Result<String, String> {
    let (width, offset) = sdwa_field(field & 7).ok_or("reserved SDWA source selector")?;
    if width == 32 {
        return Ok(word.into());
    }
    let mask = (1u32 << width) - 1;
    if field & 8 != 0 {
        Ok(format!(
            "asuint(asint(({word}) << {}u) >> {}u)",
            32 - offset - width,
            32 - width
        ))
    } else {
        Ok(format!("((({word}) >> {offset}u) & {mask}u)"))
    }
}

fn widen_half(source: &Opnd, selector: u32, absolute: bool, negate: bool) -> Result<String, String> {
    let mut word = operand(source)?;
    if matches!(source, Opnd::ConstF32(_)) {
        word = format!("f32tof16(asfloat({word}))");
    }
    let mut bits = sdwa_word(&word, selector)?;
    if absolute {
        bits = format!("({bits} & 0x7fffu)");
    }
    if negate {
        bits = format!("({bits} ^ 0x8000u)");
    }
    Ok(format!("tb_ray_widen_half({bits})"))
}

// Pack the low bits of an ALU result into the selected destination field.
fn sdwa_destination(dst: &str, value: &str, control: u32) -> Result<String, String> {
    let unused = control >> 3;
    if unused > 2 {
        return Err("unsupported SDWA destination control".into());
    }
    let (width, shift) = sdwa_field(control & 7).ok_or("reserved SDWA destination selector")?;
    if width == 32 {
        return Ok(value.into());
    }
    let mask = (1u32 << width) - 1;
    let placed = format!("(({value} & {mask}u) << {shift}u)");
    Ok(match unused {
        1 => format!(
            "asuint(asint({value} << {}u) >> {}u)",
            32 - width,
            32 - width - shift
        ),
        2 => format!("({dst} & {:#010x}u) | {placed}", !(mask << shift)),
        _ => placed,
    })
}

pub(super) const SDWA_CLAMP: u32 = 1 << 5;

pub(super) struct Mul24 {
    signed: bool,
    high: bool,
    clamp: bool,
}

impl Mul24 {
    pub(super) fn decode(ins: &Insn) -> Option<Self> {
        let (signed, high) = match ins.mnemonic.as_str() {
            "v_mul_u32_u24" => (false, false),
            "v_mul_i32_i24" => (true, false),
            "v_mul_hi_u32_u24" => (false, true),
            "v_mul_hi_i32_i24" => (true, true),
            _ => return None,
        };
        let clamp = match ins.enc {
            Enc::Vop2 => ins.aux.sdwa.is_some_and(|ext| (ext >> 8) & SDWA_CLAMP != 0),
            Enc::Vop3 => {
                let clamp = ins.aux.imm != 0;
                // Only the low product accepts VOP3 CLAMP. Neither form uses
                // OP_SEL, floating source modifiers or output scaling.
                if ins.raw[0] & 0x7800 != 0 || ins.aux.mask != 0 || ins.aux.imm2 != 0
                    || (high && clamp)
                {
                    return None;
                }
                clamp
            }
            _ => return None,
        };
        Some(Self { signed, high, clamp })
    }

    fn hlsl(&self, a: &str, b: &str) -> String {
        format!("tb_ray_mul24({a}, {b}, {}, {}, {})", self.signed, self.high, self.clamp)
    }

    pub(super) fn fold(&self, a: u32, b: u32) -> u32 {
        let extend = |x: u32| if self.signed {
            i64::from(((x << 8) as i32) >> 8)
        } else {
            i64::from(x & 0xffffff)
        };
        let product = extend(a) * extend(b);
        if self.high {
            (product >> 32) as u32
        } else if self.clamp && self.signed {
            product.clamp(i64::from(i32::MIN), i64::from(i32::MAX)) as u32
        } else if self.clamp {
            product.min(i64::from(u32::MAX)) as u32
        } else {
            product as u32
        }
    }
}

pub(super) fn vector(ins: &Insn) -> Result<String, String> {
    let name = ins.mnemonic.as_str();
    if name == "v_nop" {
        return Ok(String::new());
    }
    let dest = *ins.dst.first().ok_or("missing ray ALU destination")?;
    let dst = operand(&dest)?;
    let source = |i: usize| {
        ins.src
            .get(i)
            .ok_or_else(|| format!("{name} missing source {i}"))
    };
    let raw = |i| operand(source(i)?);
    if let Some(sdwa) = ins.aux.sdwa {
        let control = (sdwa >> 8) & 0xff;
        if ins.enc == Enc::Vopc
            && matches!(name, "v_cmp_eq_u32" | "v_cmp_ne_u32")
            && sdwa & 0x7070_0000 == 0
        {
            let left = sdwa_word(&raw(0)?, sdwa >> 16)?;
            let right = sdwa_word(&raw(1)?, sdwa >> 24)?;
            let cmp = if name == "v_cmp_eq_u32" { "==" } else { "!=" };
            return Ok(format!(
                "{dst} = (exec != 0u && {left} {cmp} {right}) ? ~0u : 0u;"
            ));
        }
        if name == "v_cvt_f32_f16" {
            let src = source(0)?;
            if sdwa & 0xff48_0000 != 0 || matches!(src, Opnd::Literal(_)) {
                return Err(format!("unsupported SDWA source {sdwa:#x} for {name}"));
            }
            let selector = (sdwa >> 16) & 7;
            let value = widen_half(src, selector, sdwa & (1 << 21) != 0, sdwa & (1 << 20) != 0)?;
            let bits = if control & SDWA_CLAMP == 0 {
                format!("asuint({value})")
            } else if control & 7 < 4 {
                // A widened half, including a propagated NaN, always has a zero
                // low byte. Saturation to 0 or 1 also leaves that byte zero.
                "0u".into()
            } else if matches!(src, Opnd::ConstF32(_)) && selector >= 4 {
                // Inline floating constants are normal halves, with a zero high word.
                format!("asuint(saturate({value}))")
            } else {
                return Err("clamped half conversion needs the ray launch FP mode".into());
            };
            let packed = sdwa_destination(&dst, &bits, control & !SDWA_CLAMP)?;
            return Ok(format!("{dst} = {packed};"));
        }
        let from_f32 = match name {
            "v_cvt_f16_f32" => Some("tb_ray_half"),
            "v_cvt_i32_f32" => Some("tb_cvt_i32_f32"),
            "v_cvt_u32_f32" => Some("tb_cvt_u32_f32"),
            _ => None,
        };
        if let Some(convert) = from_f32 {
            // Only source selection, ABS and NEG apply to this floating operand.
            // SDWA has no trailing literal word.
            if sdwa & 0xff48_0000 != 0 || matches!(source(0)?, Opnd::Literal(_)) {
                return Err(format!("unsupported SDWA source {sdwa:#x} for {name}"));
            }
            let mut bits = sdwa_word(&raw(0)?, sdwa >> 16)?;
            if sdwa & (1 << 21) != 0 {
                bits = format!("({bits} & 0x7fffffffu)");
            }
            if sdwa & (1 << 20) != 0 {
                bits = format!("({bits} ^ 0x80000000u)");
            }
            let value = format!("{convert}(asfloat({bits}))");
            let packed = sdwa_destination(&dst, &value, control)?;
            return Ok(format!("{dst} = {packed};"));
        }
        if matches!(name, "v_cvt_f32_i32" | "v_cvt_f32_u32") {
            if sdwa & 0xff70_0000 != 0 || matches!(source(0)?, Opnd::Literal(_)) {
                return Err(format!("unsupported SDWA source {sdwa:#x} for {name}"));
            }
            let selected = sdwa_word(&raw(0)?, sdwa >> 16)?;
            let mut value = if name == "v_cvt_f32_i32" {
                format!("float(asint({selected}))")
            } else {
                format!("float({selected})")
            };
            if control & SDWA_CLAMP != 0 {
                value = format!("saturate({value})");
            }
            let packed = sdwa_destination(&dst, &format!("asuint({value})"), control & !SDWA_CLAMP)?;
            return Ok(format!("{dst} = {packed};"));
        }
        // Integer sources can select and sign-extend a field from either register.
        if sdwa & 0x7070_0000 != 0 || ins.src.iter().any(|src| matches!(src, Opnd::Literal(_))) {
            return Err(format!("unsupported SDWA source {sdwa:#x} for {name}"));
        }
        let a = sdwa_word(&raw(0)?, sdwa >> 16)?;
        let b = sdwa_word(&raw(1)?, sdwa >> 24)?;
        if let Some(mul) = Mul24::decode(ins) {
            let packed = sdwa_destination(&dst, &mul.hlsl(&a, &b), control & !SDWA_CLAMP)?;
            return Ok(format!("{dst} = {packed};"));
        }
        let value = match name {
            "v_min_i32" => format!("asuint(min(asint({a}), asint({b})))"),
            "v_max_i32" => format!("asuint(max(asint({a}), asint({b})))"),
            "v_min_u32" => format!("min({a}, {b})"),
            "v_max_u32" => format!("max({a}, {b})"),
            "v_lshlrev_b32" => format!("({b} << ({a} & 31u))"),
            "v_lshrrev_b32" => format!("({b} >> ({a} & 31u))"),
            "v_ashrrev_i32" => format!("(asuint(asint({b}) >> ({a} & 31u)))"),
            "v_add_nc_u32" => format!("({a} + {b})"),
            "v_sub_nc_u32" => format!("({a} - {b})"),
            "v_xor_b32" => format!("({a} ^ {b})"),
            "v_and_b32" => format!("({a} & {b})"),
            "v_or_b32" => format!("({a} | {b})"),
            _ => return Err(format!("unsupported ray SDWA operation {name}")),
        };
        let packed = sdwa_destination(&dst, &value, control)?;
        return Ok(format!("{dst} = {packed};"));
    }
    if let Some(mul) = Mul24::decode(ins) {
        return Ok(format!("{dst} = {};", mul.hlsl(&raw(0)?, &raw(1)?)));
    }
    if matches!(name, "v_swap_b32" | "v_movrels_b32" | "v_movreld_b32") {
        let (Opnd::Vgpr(d), Opnd::Vgpr(s)) = (dest, *source(0)?) else {
            return Err(format!("ray {name} requires two vector registers"));
        };
        return Ok(match name {
            "v_swap_b32" => format!("{{ uint tmp = v[{d}]; v[{d}] = v[{s}]; v[{s}] = tmp; }}"),
            "v_movrels_b32" => format!("v[{d}] = v[({s}u + m0) & 255u];"),
            _ => format!("v[({d}u + m0) & 255u] = v[{s}];"),
        });
    }
    let bits = |i: usize| -> Result<String, String> {
        let mut word = raw(i)?;
        if ins.aux.mask & (1 << i) != 0 {
            word = format!("({word} & 0x7fffffffu)");
        }
        if ins.aux.mask & (8 << i) != 0 {
            word = format!("({word} ^ 0x80000000u)");
        }
        Ok(word)
    };
    let f = |i| bits(i).map(|s| format!("asfloat({s})"));
    if matches!(name, "v_add_co_ci_u32" | "v_add_co_u32") {
        let carry_out = match ins.enc {
            Enc::Vop2 => "vcc".into(),
            Enc::Vop3 => match (ins.raw[0] >> 8) & 127 {
                reg @ 0..=105 => operand(&Opnd::Sgpr(reg as u8))?,
                106 => "vcc".into(),
                reg => return Err(format!("unsupported ray carry register {reg}")),
            },
            _ => return Err("invalid ray add-with-carry encoding".into()),
        };
        let carry_in = if name == "v_add_co_u32" {
            "0u".into()
        } else if ins.enc == Enc::Vop2 {
            "uint(vcc != 0u)".into()
        } else {
            format!("uint({} != 0u)", raw(2)?)
        };
        return Ok(format!(
            "{{ uint x={}, y={}, c={carry_in}, sum=x+y; if (exec != 0u) {dst}=sum+c; {carry_out}=((exec != 0u && (sum<x || sum+c<sum)) ? ~0u : 0u); }}",
            raw(0)?,
            raw(1)?
        ));
    }
    if matches!(name, "v_fma_mix_f32" | "v_fma_mixlo_f16") {
        let modes = ((ins.raw[1] >> 27) & 3) | ((ins.raw[0] >> 12) & 4);
        let selections = ins.raw[0] >> 11;
        let mut values = Vec::with_capacity(3);
        for i in 0..3 {
            let word = raw(i)?;
            let mut value = if modes & (1 << i) != 0 {
                let shift = 16 * ((selections >> i) & 1);
                format!("f16tof32(({word} >> {shift}u) & 65535u)")
            } else {
                format!("asfloat({word})")
            };
            if ins.aux.mask & (1 << i) != 0 {
                value = format!("abs({value})");
            }
            if ins.aux.mask & (8 << i) != 0 {
                value = format!("(-{value})");
            }
            values.push(value);
        }
        let expr = format!("mad({}, {}, {})", values[0], values[1], values[2]);
        let expr = if ins.aux.imm == 0 {
            expr
        } else {
            format!("saturate({expr})")
        };
        return if name == "v_fma_mixlo_f16" {
            Ok(format!(
                "{dst} = ({dst} & 0xffff0000u) | tb_ray_half({expr});"
            ))
        } else {
            Ok(format!("{dst} = asuint({expr});"))
        };
    }
    if name == "v_cmp_eq_u64" && ins.aux.mask == 0 {
        return Ok(format!(
            "{dst} = (exec != 0u && {} == {} && {} == {}) ? ~0u : 0u;",
            raw(0)?,
            raw(1)?,
            high_word(*source(0)?)?,
            high_word(*source(1)?)?
        ));
    }
    if let Some(compare) = name.strip_prefix("v_cmp_") {
        let (op, ty) = compare.split_once('_').ok_or("invalid vector comparison")?;
        let (a, b) = (bits(0)?, bits(1)?);
        let convert = |s: &str| match ty {
            "f32" => Ok(format!("asfloat({s})")),
            "i32" => Ok(format!("asint({s})")),
            "u32" => Ok(s.to_owned()),
            _ => Err(format!("unsupported ray comparison {name}")),
        };
        let (left, right) = (convert(&a)?, convert(&b)?);
        let cmp = match op {
            "eq" => format!("{left} == {right}"),
            "ne" if ty != "f32" => format!("{left} != {right}"),
            "lt" => format!("{left} < {right}"),
            "le" => format!("{left} <= {right}"),
            "gt" => format!("{left} > {right}"),
            "ge" => format!("{left} >= {right}"),
            "lg" if ty == "f32" => {
                format!("!tb_nan_bits({a}) && !tb_nan_bits({b}) && {left} != {right}")
            }
            "neq" if ty == "f32" => {
                format!("tb_nan_bits({a}) || tb_nan_bits({b}) || {left} != {right}")
            }
            "ngt" if ty == "f32" => {
                format!("tb_nan_bits({a}) || tb_nan_bits({b}) || {left} <= {right}")
            }
            "nge" if ty == "f32" => {
                format!("tb_nan_bits({a}) || tb_nan_bits({b}) || {left} < {right}")
            }
            "nle" if ty == "f32" => {
                format!("tb_nan_bits({a}) || tb_nan_bits({b}) || {left} > {right}")
            }
            _ => return Err(format!("unsupported ray comparison {name}")),
        };
        return Ok(format!(
            "{dst} = (exec != 0u && ({cmp})) ? 0xffffffffu : 0u;"
        ));
    }
    if matches!(name, "v_lshlrev_b64" | "v_lshrrev_b64") {
        let direction = if name == "v_lshlrev_b64" {
            "left"
        } else {
            "right"
        };
        let hi = high_word(*source(1)?)?;
        let dst_hi = high_word(dest)?;
        return Ok(format!(
            "{{ uint2 shifted = tb_ray_shift_{direction}(uint2({}, {hi}), {}); {dst} = shifted.x; {dst_hi} = shifted.y; }}",
            raw(1)?,
            raw(0)?
        ));
    }
    let float = match name {
        "v_add_f32" => Some(format!("{} + {}", f(0)?, f(1)?)),
        "v_sub_f32" => Some(format!("{} - {}", f(0)?, f(1)?)),
        "v_subrev_f32" => Some(format!("{} - {}", f(1)?, f(0)?)),
        "v_mul_f32" => Some(format!("{} * {}", f(0)?, f(1)?)),
        "v_mul_legacy_f32" => Some(format!("tb_ray_legacy_mul({}, {})", f(0)?, f(1)?)),
        "v_fmac_f32" => Some(format!("mad({}, {}, asfloat({dst}))", f(0)?, f(1)?)),
        "v_mac_f32" => Some(format!("tb_ray_mac({}, {}, asfloat({dst}))", f(0)?, f(1)?)),
        "v_madak_f32" if ins.raw.len() == 2 => Some(format!(
            "tb_ray_mac({}, {}, asfloat(0x{:08x}u))",
            f(0)?,
            f(1)?,
            ins.raw[1]
        )),
        "v_cubesc_f32" | "v_cubetc_f32" | "v_cubema_f32" | "v_cubeid_f32" => {
            let component = match name {
                "v_cubesc_f32" => 'x',
                "v_cubetc_f32" => 'y',
                "v_cubema_f32" => 'z',
                _ => 'w',
            };
            Some(format!(
                "tb_ray_cube({}, {}, {}).{component}",
                f(0)?,
                f(1)?,
                f(2)?
            ))
        }
        "v_mad_f32" => Some(format!("tb_ray_mac({}, {}, {})", f(0)?, f(1)?, f(2)?)),
        "v_mad_legacy_f32" => Some(format!("tb_ray_legacy_mad({}, {}, {})", f(0)?, f(1)?, f(2)?)),
        "v_fma_f32" => Some(format!("mad({}, {}, {})", f(0)?, f(1)?, f(2)?)),
        "v_fmaak_f32" if ins.raw.len() == 2 => Some(format!(
            "mad({}, {}, asfloat(0x{:08x}u))",
            f(0)?,
            f(1)?,
            ins.raw[1]
        )),
        "v_med3_f32" => Some(format!("tb_ray_median({}, {}, {})", f(0)?, f(1)?, f(2)?)),
        "v_min_f32" => Some(format!("min({}, {})", f(0)?, f(1)?)),
        "v_max_f32" => Some(format!("max({}, {})", f(0)?, f(1)?)),
        "v_min3_f32" => Some(format!("min(min({}, {}), {})", f(0)?, f(1)?, f(2)?)),
        "v_max3_f32" => Some(format!("max(max({}, {}), {})", f(0)?, f(1)?, f(2)?)),
        "v_ceil_f32" => Some(format!("ceil({})", f(0)?)),
        "v_floor_f32" => Some(format!("floor({})", f(0)?)),
        "v_fract_f32" => Some(format!("tb_fract_f32({}, 0u)", f(0)?)),
        "v_rcp_f32" | "v_rcp_iflag_f32" => Some(format!("rcp({})", f(0)?)),
        "v_sqrt_f32" => Some(format!("sqrt({})", f(0)?)),
        "v_rsq_f32" => Some(format!("rsqrt({})", f(0)?)),
        "v_log_f32" => Some(format!("log2({})", f(0)?)),
        "v_exp_f32" => Some(format!("exp2({})", f(0)?)),
        "v_rndne_f32" => Some(format!("round({})", f(0)?)),
        "v_trunc_f32" => Some(format!("trunc({})", f(0)?)),
        // The hardware takes angles in turns.
        "v_sin_f32" => Some(format!("sin({} * 6.283185307179586f)", f(0)?)),
        "v_cos_f32" => Some(format!("cos({} * 6.283185307179586f)", f(0)?)),
        "v_cvt_f32_u32" => Some(format!("float({})", raw(0)?)),
        "v_cvt_f32_i32" => Some(format!("float(asint({}))", raw(0)?)),
        "v_cvt_f32_f16" => {
            let high = ins.enc == Enc::Vop3 && ins.raw[0] & (1 << 11) != 0;
            Some(widen_half(
                source(0)?,
                if high { 5 } else { 6 },
                ins.aux.mask & 1 != 0,
                ins.aux.mask & 8 != 0,
            )?)
        }
        "v_cvt_f32_ubyte0" | "v_cvt_f32_ubyte1" | "v_cvt_f32_ubyte2" | "v_cvt_f32_ubyte3" => {
            let shift = (name.as_bytes()[name.len() - 1] - b'0') * 8;
            Some(format!("float(({} >> {shift}u) & 255u)", raw(0)?))
        }
        _ => None,
    };
    if let Some(mut expr) = float {
        if ins.enc == Enc::Vop3 {
            let factor = ["", "2.0f", "4.0f", "0.5f"][ins.aux.imm2 as usize];
            if !factor.is_empty() {
                expr = format!("({expr}) * {factor}");
            }
            if ins.aux.imm != 0 {
                expr = format!("saturate({expr})");
            }
        }
        return Ok(format!("{dst} = asuint({expr});"));
    }
    let value = match name {
        "v_mov_b32" => bits(0)?,
        // The ray compiler broadcasts a vector arithmetic result back to an
        // SGPR (for example, the reciprocal used in integer division).
        "v_readfirstlane_b32" if matches!(dest, Opnd::Sgpr(_)) => {
            format!("WaveReadLaneAt({}, tb_ray_first_lane())", raw(0)?)
        }
        "v_mbcnt_lo_u32_b32" => {
            let mask = match source(0)? {
                Opnd::ExecLo => "tb_ray_ballot(exec != 0u)".to_owned(),
                Opnd::VccLo => "tb_ray_ballot(vcc != 0u)".to_owned(),
                Opnd::ConstInt(_) | Opnd::Literal(_) => raw(0)?,
                _ => return Err("unsupported packed mask for ray mbcnt".into()),
            };
            format!("{} + countbits(({mask}) & ((1u << (WaveGetLaneIndex() & 31u)) - 1u))", raw(1)?)
        }
        "v_cndmask_b32" => {
            let condition = if ins.enc == Enc::Vop3 {
                raw(2)?
            } else {
                operand(&Opnd::VccLo)?
            };
            format!("({condition} != 0u) ? {} : {}", bits(1)?, bits(0)?)
        }
        "v_cvt_u32_f32" => format!("tb_cvt_u32_f32({})", f(0)?),
        "v_cvt_i32_f32" => format!("tb_cvt_i32_f32({})", f(0)?),
        "v_cvt_f16_f32" if ins.enc == Enc::Vop1 => format!("({dst} & 0xffff0000u) | tb_ray_half({})", f(0)?),
        "v_perm_b32" => format!("tb_ray_perm({}, {}, {})", raw(0)?, raw(1)?, raw(2)?),
        "v_and_b32" => format!("{} & {}", raw(0)?, raw(1)?),
        "v_or_b32" => format!("{} | {}", raw(0)?, raw(1)?),
        "v_xor_b32" => format!("{} ^ {}", raw(0)?, raw(1)?),
        "v_and_or_b32" => format!("({} & {}) | {}", raw(0)?, raw(1)?, raw(2)?),
        "v_or3_b32" => format!("{} | {} | {}", raw(0)?, raw(1)?, raw(2)?),
        "v_add_nc_u32" => format!("{} + {}", raw(0)?, raw(1)?),
        "v_sub_nc_u32" => format!("{} - {}", raw(0)?, raw(1)?),
        "v_subrev_nc_u32" => format!("{} - {}", raw(1)?, raw(0)?),
        "v_mul_hi_u32" => format!(
            "uint((uint64_t({}) * uint64_t({})) >> 32u)",
            raw(0)?,
            raw(1)?
        ),
        "v_min_i32" => format!("asuint(min(asint({}), asint({})))", raw(0)?, raw(1)?),
        "v_max_i32" => format!("asuint(max(asint({}), asint({})))", raw(0)?, raw(1)?),
        "v_min_u32" => format!("min({}, {})", raw(0)?, raw(1)?),
        "v_max_u32" => format!("max({}, {})", raw(0)?, raw(1)?),
        "v_ffbl_b32" => format!("firstbitlow({})", raw(0)?),
        "v_mad_u32_u24" => format!(
            "({} & 0xffffffu) * ({} & 0xffffffu) + {}",
            raw(0)?,
            raw(1)?,
            raw(2)?
        ),
        "v_add_lshl_u32" => format!("({} + {}) << ({} & 31u)", raw(0)?, raw(1)?, raw(2)?),
        "v_add3_u32" => format!("{} + {} + {}", raw(0)?, raw(1)?, raw(2)?),
        "v_lshl_or_b32" => format!("({} << ({} & 31u)) | {}", raw(0)?, raw(1)?, raw(2)?),
        "v_mul_lo_u32" => format!("{} * {}", raw(0)?, raw(1)?),
        "v_xad_u32" => format!("({} ^ {}) + {}", raw(0)?, raw(1)?, raw(2)?),
        "v_lshlrev_b32" => format!("{} << ({} & 31u)", raw(1)?, raw(0)?),
        "v_lshrrev_b32" => format!("{} >> ({} & 31u)", raw(1)?, raw(0)?),
        "v_ashrrev_i32" => format!("asuint(asint({}) >> ({} & 31u))", raw(1)?, raw(0)?),
        "v_lshl_add_u32" => format!("({} << ({} & 31u)) + {}", raw(0)?, raw(1)?, raw(2)?),
        "v_bfrev_b32" => format!("reversebits({})", raw(0)?),
        "v_bfe_u32" => format!("tb_ray_bfe({}, {}, ({} & 31u))", raw(0)?, raw(1)?, raw(2)?),
        _ => {
            return Err(format!(
                "unsupported ray ALU operation {name} at {:#x}",
                ins.offset
            ));
        }
    };
    Ok(format!("{dst} = {value};"))
}
