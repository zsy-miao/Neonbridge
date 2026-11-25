//! Synthetic GFX10 programs with live results, checked against CPU models.

use std::sync::{Mutex, MutexGuard};

use super::host::{Dispatch, Host, Root, View};
use super::opcode_audit::{compute, root_param, sop1, sop2, sopc, vop1};
use super::validator::Validator;
use super::{Container, emit, io, ir, resource, structure};
use crate::gfx10::{Encoding, test_asm::{S_ENDPGM, opcodes}};
use crate::native::{RootType, StageIo};

fn op(encoding: Encoding, name: &str) -> u32 {
    u32::from(opcodes().find(|&(e, _, n)| e == encoding && n == name).unwrap().1)
}

struct Gpu {
    host: Option<Host>,
    validator: Option<Validator>,
    _serial: MutexGuard<'static, ()>,
}

impl Gpu {
    fn new() -> Self {
        // These suites submit hundreds of small jobs to the same adapter.
        static ADAPTER: Mutex<()> = Mutex::new(());
        let serial = ADAPTER.lock().unwrap_or_else(|e| e.into_inner());
        let host = Host::new(false).map_err(|e| eprintln!("no SM 6.6 adapter: {e}")).ok();
        let validator = match Validator::default_path() {
            Some(path) => Some(Validator::load(Some(&path)).unwrap()),
            None => Validator::load(None).ok(),
        };
        Self { host, validator, _serial: serial }
    }

    fn check<const N: usize>(&mut self, name: &str, body: &[u32], results: u32, cases: &[([u32; 8], [u32; N])]) {
        let mut words = vec![sop1(3, 9, 128), 0xF408_0404, 0xFA00_0000]; // s_load_dwordx4 s[16:19], s[8:9], null
        words.extend_from_slice(body);
        for i in 0..results {
            // buffer_store_dword v[24+i], off, s[16:19], 0 offset:4*i
            words.extend([0xE070_0000 | (4 * i), 0x8004_0000 | (24 + i) << 8]);
        }
        words.push(S_ENDPGM);
        let mut native = compute(&words, vec![root_param(RootType::Constants, 0, 8),
            root_param(RootType::Table, 8, 1)], false);
        let StageIo::Cs(ref mut cs) = native.io else { unreachable!() };
        cs.threads = [1, 1, 1];
        let bytes = crate::dxbc::build(&[]);
        let c = Container::parse(&bytes).unwrap();
        let p = ir::build(&native.patched_code(), native.wave64, native.vgprs).unwrap();
        let res = resource::track(&p, &native.layout, native.stage).unwrap();
        let io = io::raise(&p, &native).unwrap();
        let st = structure::structure(&p).unwrap();
        let mut shader = emit(&c, &native, &p, &st, &io, &res, crate::emit::Target::ANY)
            .unwrap_or_else(|e| panic!("{name}: {e}"));
        if let Some(v) = &mut self.validator {
            v.validate(&mut shader.dxil).unwrap_or_else(|e| panic!("{name}: {e}"));
        }
        let Some(host) = &self.host else { return };
        let (root, pso) = host.compute_pipeline(&shader).unwrap_or_else(|e| panic!("{name}: {e}"));
        for &(inputs, expected) in cases {
            let mut ud = vec![0; shader.user_data_dwords().max(1) as usize];
            for u in &shader.user_data {
                if u.root == RootType::Constants {
                    ud[u.index as usize] = inputs[u.dword as usize];
                }
            }
            let out = host.dispatch(&root, &pso, &Dispatch {
                roots: vec![(0, Root::Constants(ud))], views: &[View::RawUav(results)],
                samplers: &[], groups: [1, 1, 1],
            }).unwrap_or_else(|e| panic!("{name}: {e}"));
            assert_eq!(out[0], expected[..results as usize], "{name} with {inputs:08x?}");
        }
    }
}

fn vop3(op: u32, dst_and_mods: u32, a: u32, b: u32, c: u32) -> [u32; 2] {
    [0xD400_0000 | op << 16 | dst_and_mods, a | b << 9 | c << 18]
}

fn vector_words() -> Vec<u32> {
    // v4-v11 source words, v24 the in-place destination for half or MAC ops.
    let mut words: Vec<_> = (0..8).map(|i| vop1(1, 4 + i, i)).collect();
    words.push(vop1(1, 24, 7));
    words
}

#[test]
fn vector_integer_arithmetic() {
    use Encoding::{Vop1, Vop2, Vop3};
    let mut gpu = Gpu::new();
    let samples = [
        [0x7FFF_7FFF, 0x0123_0000, 0x0001_7FFF, 0, 0x7FFF_FFFF, 0, 0, 0xABCD_EF12],
        [0x8000_8000, 0, 0x8000_8000, 0, 0x8000_0000, 0, 0, 0xFEDC_BA98],
        [0x0000_FFFF, 0, 0xFFFF_FFFF, 0, 0x0000_FFFF, 0, 0, 0x7654_3210],
        [0x7FFF_FFFF, 0, 0x0000_0001, 0, u32::MAX, 0, 0, 0xFADE_BABE],
        [0x8012_0001, 0, 0xC001_00FF, 0, 7, 0, 0, 0x1234_5678],
        [0x8000_0000, 0, 1, 0, 0, 0, 0, 0xCAFE_BEEF],
        [0x0100_00FF, 0, 0xF000_0040, 0, 3, 0, 0, 0xCAFE_BEEF],
        [0xFF80_0000, 0, 0xFFFF_FFFF, 0, 0, 0, 0, 0xCAFE_BEEF],
    ];
    for (enc, name, nsrc) in [
        (Vop1, "v_sat_pk_u8_i16", 1), (Vop2, "v_mul_hi_i32_i24", 2),
        (Vop2, "v_mul_hi_u32_u24", 2),
        (Vop3, "v_mad_i32_i16", 3), (Vop3, "v_mad_u32_u16", 3),
        (Vop3, "v_cvt_pk_u16_u32", 2), (Vop3, "v_cvt_pk_i16_i32", 2),
    ] {
        let saturates = name.starts_with("v_mad");
        for clamp in [false, true].into_iter().filter(|&c| !c || saturates) {
            let n = op(enc, name);
            let mut body = vector_words();
            match enc {
                Vop1 => body.push(vop1(n, 24, 260)),
                Vop2 => body.push(n << 25 | 24 << 17 | 6 << 9 | 260),
                _ => {
                    let src0_hi = u32::from(nsrc == 3 && clamp);
                    body.extend(vop3(n, 24 | u32::from(clamp) << 15 | src0_hi << 11,
                        260, 262, if nsrc == 3 { 264 } else { 0 }));
                }
            }
            let cases = samples.map(|input @ [a, _, b, _, d, _, _, _]| {
                let signed24 = |x: u32| ((x << 8) as i32 >> 8) as i64;
                let signed = !name.ends_with("u32") && !name.ends_with("u16");
                let sa = i64::from(a as i32);
                let sb = i64::from(b as i32);
                let r = match name {
                    "v_sat_pk_u8_i16" => i64::from((a as i16).clamp(0, 255))
                        | i64::from(((a >> 16) as i16).clamp(0, 255)) << 8,
                    "v_mul_hi_i32_i24" => (signed24(a) * signed24(b)) >> 32,
                    "v_mul_hi_u32_u24" => (i64::from(a & 0xFF_FFFF) * i64::from(b & 0xFF_FFFF)) >> 32,
                    "v_mad_i32_i16" => {
                        let a = if clamp { a >> 16 } else { a };
                        i64::from(a as i16) * i64::from(b as i16) + i64::from(d as i32)
                    }
                    "v_mad_u32_u16" => {
                        let a = if clamp { a >> 16 } else { a };
                        i64::from(a as u16) * i64::from(b as u16) + i64::from(d)
                    }
                    "v_cvt_pk_u16_u32" => i64::from(a.min(65535) | b.min(65535) << 16),
                    _ => i64::from((sa.clamp(-32768, 32767) as u32 & 65535) | (sb.clamp(-32768, 32767) as u32) << 16),
                };
                let r = if clamp && signed { r.clamp(i32::MIN as i64, i32::MAX as i64) }
                    else if clamp { r.min(u32::MAX as i64) } else { r };
                (input, [r as u32])
            });
            gpu.check(name, &body, 1, &cases);
        }
    }
}

#[test]
fn signed_four_bit_interpolation_offsets() {
    let mut gpu = Gpu::new();
    let name = "v_cvt_off_f32_i4";
    let body = [vop1(op(Encoding::Vop1, name), 24, 0)];
    let cases = std::array::from_fn::<_, 16, _>(|low| {
        let raw = 0xA5A5_FF00 | low as u32;
        let offset = if low < 8 { low as i32 } else { low as i32 - 16 };
        ([raw, 0, 0, 0, 0, 0, 0, 0], [(offset as f32 / 16.0).to_bits()])
    });
    gpu.check(name, &body, 1, &cases);
}

#[test]
fn integer_half_arithmetic_preserves_the_other_half() {
    let mut gpu = Gpu::new();
    let samples = [
        [0x8000_7FFF, 0, 0x8000_FFFF, 0, 0x9000_8000, 0, 0, 0xAAAA_5555],
        [0xFFF0_FFFF, 0, 0xFFFF_7FFE, 0, 0x0090_FFFF, 0, 0, 0xDEAD_BEEF],
        [0x1234_007B, 0, 0x1000_007B, 0, 0x1000_0016, 0, 0, 0x8877_6655],
        [0x8000_7FFF, 0, 0x7FFF_7FFF, 0, 0x7FFF_0080, 0, 0, 0xCAFE_D00D],
    ];
    for name in ["v_mad_i16", "v_mad_u16",
        "v_min3_i16", "v_min3_u16", "v_max3_i16", "v_max3_u16", "v_med3_i16", "v_med3_u16"] {
        let tri = name.contains('3') || name.starts_with("v_mad");
        for (high, clamp) in [(false, false), (true, false), (false, true)] {
            if clamp && !matches!(name, "v_mad_i16" | "v_mad_u16") { continue }
            let mut body = vector_words();
            let select = if high { if tri { 13 } else { 9 } } else { 0 };
            body.extend(vop3(op(Encoding::Vop3, name), 24 | select << 11 | u32::from(clamp) << 15,
                260, 262, if tri { 264 } else { 0 }));
            let cases = samples.map(|input @ [a, _, b, _, d, _, _, old]| {
                let signed = name.ends_with("_i16");
                let half = |x: u32| if signed { (x as i16) as i64 } else { (x as u16) as i64 };
                let (a, b, d) = (half(if high { a >> 16 } else { a }), half(b), half(if high { d >> 16 } else { d }));
                let mut sorted = [a, b, d];
                sorted.sort_unstable();
                let r = match &name[2..5] {
                    "mad" => a * b + d,
                    "min" => sorted[0], "med" => sorted[1], _ => sorted[2],
                };
                let r = if clamp && signed { r.clamp(-32768, 32767) }
                    else if clamp { r.clamp(0, 65535) } else { r } as u32;
                let r = if high { (old & 0xFFFF) | r << 16 } else { (old & 0xFFFF_0000) | (r & 0xFFFF) };
                (input, [r])
            });
            gpu.check(name, &body, 1, &cases);
        }
    }
}

#[test]
fn packed_integer_dot_products() {
    let mut gpu = Gpu::new();
    let samples = [
        [u32::MAX, 0, u32::MAX, 0, u32::MAX, 0, 0, u32::MAX],
        [0x8000_8000, 0, 0x8000_8000, 0, 0x7FFF_FFFF, 0, 0, 0x7FFF_FFFF],
        [0x8000_8080, 0, 0x7FFF_7F7F, 0, 0x8000_0000, 0, 0, 0x8000_0000],
        [0xFF80_3C12, 0, 0x017F_00CA, 0, 0x1234_5678, 0, 0, 0xABCD_1234],
    ];
    for (name, bits, signed, implicit) in [
        ("v_dot2_i32_i16", 16, true, false), ("v_dot2_u32_u16", 16, false, false),
        ("v_dot4_i32_i8", 8, true, false), ("v_dot4_u32_u8", 8, false, false),
        ("v_dot4c_i32_i8", 8, true, true), ("v_dot8_u32_u4", 4, false, false),
    ] {
        for (clamp, swap) in [(false, false), (true, false), (false, true)] {
            if (implicit && clamp) || (bits != 16 && swap) { continue }
            let mut body = vector_words();
            if implicit {
                body.push(op(Encoding::Vop2, name) << 25 | 24 << 17 | 6 << 9 | 260);
            } else {
                let select = if swap { 1 } else { 0 };
                body.extend([0xCC00_0000 | op(Encoding::Vop3p, name) << 16 | u32::from(clamp) << 15
                    | 1 << 14 | select << 11 | 24, (3 - select) << 27 | 264 << 18 | 262 << 9 | 260]);
            }
            let cases = samples.map(|input @ [a, _, b, _, d, _, _, old]| {
                let a = if swap { a.rotate_left(16) } else { a };
                let d = if implicit { old } else { d };
                let d = if signed { i64::from(d as i32) } else { i64::from(d) };
                let component = |x: u32, i| {
                    let x = (x >> (i * bits)) & ((1u32 << bits) - 1);
                    if signed { i64::from(((x << (32 - bits)) as i32) >> (32 - bits)) } else { i64::from(x) }
                };
                let r = d + (0..32 / bits).map(|i| component(a, i) * component(b, i)).sum::<i64>();
                let r = if clamp && signed { r.clamp(i32::MIN as i64, i32::MAX as i64) }
                    else if clamp { r.min(u32::MAX as i64) } else { r };
                (input, [r as u32])
            });
            gpu.check(name, &body, 1, &cases);
        }
    }
}

#[test]
fn sum_of_absolute_differences_and_byte_average() {
    let mut gpu = Gpu::new();
    let samples = [
        [0x1234_56FF, 0xABCD_EFAB, 0x00FF_0100, 0, 0xFF80_FFFF, 0xFFFF_0000, 1, 0],
        [0xFFFF_FFFF, 0x8000_0080, 0, 0, u32::MAX, 0xFFFF_FFFE, 0x1234_0000, 0],
        [0x1213_1415, 0, 0x1112_1314, 0, 0x0100_FF02, 0, 0, 0],
    ];
    for name in ["v_sad_u8", "v_sad_hi_u8", "v_sad_u16", "v_sad_u32", "v_msad_u8", "v_lerp_u8",
        "v_qsad_pk_u16_u8", "v_mqsad_pk_u16_u8", "v_mqsad_u32_u8"] {
        let quad = name.contains("qsad");
        let count = if name == "v_mqsad_u32_u8" { 4 } else if quad { 2 } else { 1 };
        let mut body = vector_words();
        body.extend(vop3(op(Encoding::Vop3, name), 24, 260, 262, 264));
        let cases = samples.map(|input @ [a, hi, b, _, d, d1, d2, d3]| {
            let sad8 = |x: u32| x.to_le_bytes().into_iter().zip(b.to_le_bytes())
                .filter(|&(_, y)| y != 0 || !name.starts_with("v_m"))
                .map(|(x, y)| u32::from(x.abs_diff(y))).sum::<u32>();
            let mut r = [0; 4];
            if quad {
                let src = (u64::from(hi) << 32) | u64::from(a);
                let add = [d, d1, d2, d3];
                for i in 0..4 {
                    let sum = sad8((src >> (i * 8)) as u32);
                    if count == 4 { r[i] = add[i].wrapping_add(sum) }
                    else {
                        let sh = (i % 2) * 16;
                        let k = (add[i / 2] >> sh) as u16;
                        r[i / 2] |= u32::from(k.wrapping_add(sum as u16)) << sh;
                    }
                }
            } else {
                r[0] = match name {
                    "v_lerp_u8" => {
                        let (a, b, d) = (a.to_le_bytes(), b.to_le_bytes(), d.to_le_bytes());
                        u32::from_le_bytes(std::array::from_fn(|i| ((u16::from(a[i]) + u16::from(b[i]) + u16::from(d[i] & 1)) / 2) as u8))
                    }
                    "v_sad_u32" => a.abs_diff(b).wrapping_add(d),
                    "v_sad_u16" => d.wrapping_add(u32::from((a as u16).abs_diff(b as u16)) + (a >> 16).abs_diff(b >> 16)),
                    "v_sad_hi_u8" => d.wrapping_add(sad8(a) << 16),
                    _ => d.wrapping_add(sad8(a)),
                };
            }
            (input, r)
        });
        gpu.check(name, &body, count, &cases);
    }
}

#[test]
fn multiply_add_64_bit_value_and_carry() {
    let mut gpu = Gpu::new();
    for (name, signed) in [("v_mad_u64_u32", false), ("v_mad_i64_i32", true)] {
        let mut body = vector_words();
        body.extend(vop3(op(Encoding::Vop3, name), 24 | 26 << 8, 260, 262, 264));
        // Read the per-lane carry in s26 using v_cndmask_b32_e64.
        body.extend(vop3(0x101, 26, 128, 129, 26));
        let cases = [
            (u32::MAX, u32::MAX, u64::MAX), (1, 1, u64::MAX), (2, 7, 3),
            (0x8000_0000, 0x8000_0000, i64::MAX as u64), (0x8000_0000, 2, i64::MIN as u64),
            (u32::MAX, 5, 4), (u32::MAX, 5, 5),
        ].map(|(a, b, d)| {
            let sum = if signed { i128::from(a as i32) * i128::from(b as i32) + i128::from(d as i64) }
                else { i128::from(a) * i128::from(b) + i128::from(d) };
            ([a, 0, b, 0, d as u32, (d >> 32) as u32, 0, 0], words64(sum as u64, sum >> 64 & 1 != 0))
        });
        gpu.check(name, &body, 3, &cases);
    }
}

#[test]
fn frexp_preserves_ieee_classes_and_normalizes_subnormals() {
    let mut gpu = Gpu::new();
    for (ty, bits, fraction, bias, infinity) in [
        ("f16", 16, 10, 15, 0x7C00u64),
        ("f32", 32, 23, 127, 0x7F80_0000),
        ("f64", 64, 52, 1023, 0x7FF0_0000_0000_0000),
    ] {
        let sign = 1u64 << (bits - 1);
        let half = (bias - 1) << fraction;
        let three_quarters = half | (1 << (fraction - 1));
        let inputs = [
            (0, 0, 0), (sign, sign, 0), (half, half, 0),
            (sign | (bias << fraction), sign | half, 1),
            (1, half, 2 - bias as i32 - fraction),
            (sign | 3, sign | three_quarters, 3 - bias as i32 - fraction),
            (infinity, infinity, 0), (infinity | sign, infinity | sign, 0),
            (infinity | 1, infinity | 1, 0),
            (infinity | (1 << (fraction - 1)), infinity | (1 << (fraction - 1)), 0),
        ];
        for mantissa in [false, true] {
            let name = if mantissa { format!("v_frexp_mant_{ty}") }
                else if bits == 16 { "v_frexp_exp_i16_f16".to_owned() }
                else { format!("v_frexp_exp_i32_{ty}") };
            let mut body = vector_words();
            body.push(vop1(op(Encoding::Vop1, &name), 24, 260));
            let cases = inputs.map(|(x, mant, exp)| {
                let raw = if mantissa { mant } else { exp as u32 as u64 };
                let old = 0xABCD_1234;
                let expected = if bits == 16 { old & 0xFFFF_0000 | raw as u32 & 0xFFFF } else { raw as u32 };
                ([x as u32, (x >> 32) as u32, 0, 0, 0, 0, 0, old], [expected, (raw >> 32) as u32])
            });
            gpu.check(&name, &body, if bits == 64 && mantissa { 2 } else { 1 }, &cases);
        }
    }
}

// A normal compare writes VCC, used by cndmask. A compare into EXEC
// leaves the output at its original zero when the lane is disabled.
fn compare_body(name: &str, b: u32) -> Vec<u32> {
    let mut body = vector_words();
    let cmp = 0x7C00_0000 | op(Encoding::Vopc, name) << 17 | b << 9 | 260;
    if name.starts_with("v_cmpx_") {
        body.extend([cmp, vop1(1, 24, 129)]);
    } else {
        body.extend([vop1(1, 12, 129), cmp, 1 << 25 | 24 << 17 | 12 << 9 | 128]);
    }
    body
}

#[test]
fn float_class_compares_distinguish_signaling_nan_and_signed_zero() {
    let mut gpu = Gpu::new();
    for (ty, sign, normal, inf, quiet) in [
        ("f16", 1 << 15, 0x3C00, 0x7C00, 1 << 9),
        ("f32", 1 << 31, 0x3F80_0000, 0x7F80_0000, 1 << 22),
        ("f64", 1u64 << 63, 0x3FF0_0000_0000_0000, 0x7FF0_0000_0000_0000, 1 << 51),
    ] {
        let classes = [inf | 1, inf | quiet, sign | inf, sign | normal, sign | 1,
            sign, 0, 1, normal, inf];
        let cases: Vec<_> = classes.into_iter().enumerate().flat_map(|(class, x)| {
            [true, false].map(|yes| {
                let mask = 1 << if yes { class } else { (class + 1) % 10 };
                ([x as u32, (x >> 32) as u32, mask, 0, 0, 0, 0, 0], [u32::from(yes)])
            })
        }).collect();
        for prefix in ["v_cmp", "v_cmpx"] {
            let name = format!("{prefix}_class_{ty}");
            gpu.check(&name, &compare_body(&name, 6), 1, &cases);
        }
    }
}

#[test]
fn double_compare_predicates_and_constant_64_bit_integer_compares() {
    let mut gpu = Gpu::new();
    for ty in ["f64", "i64", "u64"] {
        let ops: &[&str] = if ty == "f64" {
            &["f", "lt", "eq", "le", "gt", "lg", "ge", "o", "u", "nge", "nlg", "ngt", "nle", "neq", "nlt", "tru"]
        } else { &["f", "t"] };
        for &pred in ops {
            let cases = [
                (-0.0, 0.0), (1.0, 2.0), (-1.0, -2.0), (-5.0, 5.0),
                (f64::from_bits(1), 0.0), (f64::from_bits(1 << 63 | 1), 0.0),
                (f64::INFINITY, f64::INFINITY), (f64::NEG_INFINITY, f64::INFINITY),
                (f64::NAN, 1.0), (1.0, f64::from_bits(0x7FF0_0000_0000_0001)),
            ].map(|(a, b)| {
                let ord = a.partial_cmp(&b);
                let test = match pred {
                    "f" => false, "tru" | "t" => true, "lt" => a < b, "eq" => a == b,
                    "le" => a <= b, "gt" => a > b, "lg" => ord.is_some_and(|o| o.is_ne()),
                    "ge" => a >= b, "o" => ord.is_some(), "u" => ord.is_none(),
                    "nge" => ord.is_none_or(|o| o.is_lt()), "nlg" => ord.is_none_or(|o| o.is_eq()),
                    "ngt" => ord.is_none_or(|o| o.is_le()), "nle" => ord.is_none_or(|o| o.is_gt()),
                    "neq" => a != b, _ => ord.is_none_or(|o| o.is_ge()),
                };
                let (a, b) = (a.to_bits(), b.to_bits());
                ([a as u32, (a >> 32) as u32, b as u32, (b >> 32) as u32, 0, 0, 0, 0], [u32::from(test)])
            });
            for prefix in ["v_cmp", "v_cmpx"] {
                let name = format!("{prefix}_{pred}_{ty}");
                gpu.check(&name, &compare_body(&name, 6), 1, &cases);
            }
        }
    }
}

fn words64(x: u64, flag: bool) -> [u32; 3] {
    [x as u32, (x >> 32) as u32, u32::from(flag)]
}

fn scalar(gpu: &mut Gpu, enc: Encoding, name: &str, width: u32, scc: bool,
          cases: &[([u32; 8], [u32; 3])]) {
    let n = op(enc, name);
    // SCC supplied by s4, and the old destination in s[6:7].
    let mut body = vec![sop1(4, 24, 6), sopc(7, 4, 128)];
    body.push(match enc {
        Encoding::Sop1 => sop1(n, 24, 0),
        Encoding::Sop2 => sop2(n, 24, 0, 2),
        Encoding::Sopc => sopc(n, 0, 2),
        _ => unreachable!(),
    });
    for i in 0..width {
        body.push(vop1(1, 24 + i, 24 + i));
    }
    if scc {
        body.extend([sop2(0xA, 28, 129, 128), vop1(1, 24 + width, 28)]);
    }
    gpu.check(name, &body, width + u32::from(scc), cases);
}

#[test]
fn scalar_bit_operations() {
    use Encoding::{Sop1, Sop2, Sopc};
    let mut gpu = Gpu::new();
    let samples = [
        [0, 0, 0, 0, 0, 0, 0x8765_4321, 0xFEDC_BA98],
        [u32::MAX, u32::MAX, 95, 0, 1, 0, 0, 0],
        [0x8000_0080, 0, 32, 0, 0, 0, 0xAAAA_5555, 0x8000_0000],
        [u32::MAX, 0x8000_0001, 63, 0, 1, 0, 0x5555_AAAA, u32::MAX],
        [0, 4, 1, 0, 1, 0, 0x7654_3210, 0x89AB_CDEF],
        [0x0000_8001, 0xFFFF_FFFF, 65, 0, 0, 0, 1, 2],
        [0x8000_0000, 0, 0, 0, 1, 0, 0, 0], // absdiff produces INT_MIN
        [0x8000_0000, 0, 1, 0, 0, 0, 0, 0], // subtraction wraps before abs
    ];
    let first = |index: u32, width: u32| if index == width { u32::MAX } else { index };
    for (enc, name, width, scc) in [
        (Sop1, "s_bcnt0_i32_b32", 1, true),
        (Sop1, "s_bcnt0_i32_b64", 1, true), (Sop1, "s_ff0_i32_b32", 1, false),
        (Sop1, "s_ff0_i32_b64", 1, false), (Sop1, "s_flbit_i32_b64", 1, false),
        (Sop1, "s_flbit_i32_i64", 1, false), (Sop1, "s_brev_b64", 2, false),
        (Sop1, "s_sext_i32_i8", 1, false), (Sop1, "s_sext_i32_i16", 1, false),
        (Sop1, "s_bitset0_b64", 2, false), (Sop1, "s_bitset1_b64", 2, false),
        (Sop1, "s_bitreplicate_b64_b32", 2, false), (Sop2, "s_absdiff_i32", 1, true),
        (Sop2, "s_pack_ll_b32_b16", 1, false), (Sop2, "s_pack_lh_b32_b16", 1, false),
        (Sop2, "s_pack_hh_b32_b16", 1, false), (Sopc, "s_bitcmp0_b32", 0, true),
        (Sopc, "s_bitcmp1_b32", 0, true), (Sopc, "s_bitcmp0_b64", 0, true),
        (Sopc, "s_bitcmp1_b64", 0, true),
    ] {
        let cases = samples.map(|inputs @ [a, hi, b, _, _, _, old, old_hi]| {
            let x = u64::from(a) | u64::from(hi) << 32;
            let old64 = u64::from(old) | u64::from(old_hi) << 32;
            let r = match name {
                "s_bcnt0_i32_b32" => [a.count_zeros(), u32::from(a != u32::MAX), 0],
                "s_bcnt0_i32_b64" => [x.count_zeros(), u32::from(x != u64::MAX), 0],
                "s_ff0_i32_b32" => [first(a.trailing_ones(), 32), 0, 0],
                "s_ff0_i32_b64" => [first(x.trailing_ones(), 64), 0, 0],
                "s_flbit_i32_b64" => [first(x.leading_zeros(), 64), 0, 0],
                "s_flbit_i32_i64" => [first(if x >> 63 != 0 { x.leading_ones() } else { x.leading_zeros() }, 64), 0, 0],
                "s_brev_b64" => words64(x.reverse_bits(), false),
                "s_sext_i32_i8" => [(a as i8 as i32) as u32, 0, 0],
                "s_sext_i32_i16" => [(a as i16 as i32) as u32, 0, 0],
                "s_bitset0_b64" => words64(old64 & !(1 << (a & 63)), false),
                "s_bitset1_b64" => words64(old64 | (1 << (a & 63)), false),
                "s_bitreplicate_b64_b32" => words64((0..32).map(|i| (u64::from(a >> i & 1) * 3) << (2 * i)).sum(), false),
                "s_absdiff_i32" => {
                    let r = (a as i32).wrapping_sub(b as i32).wrapping_abs() as u32;
                    [r, u32::from(r != 0), 0]
                }
                "s_pack_ll_b32_b16" => [a & 0xFFFF | b << 16, 0, 0],
                "s_pack_lh_b32_b16" => [a & 0xFFFF | b & 0xFFFF_0000, 0, 0],
                "s_pack_hh_b32_b16" => [a >> 16 | b & 0xFFFF_0000, 0, 0],
                _ => {
                    let mask = if name.ends_with("64") { 63 } else { 31 };
                    let is_one = x >> (b & mask) & 1 != 0;
                    [u32::from(is_one == name.starts_with("s_bitcmp1")), 0, 0]
                }
            };
            (inputs, r)
        });
        scalar(&mut gpu, enc, name, width, scc, &cases);
    }
}

#[test]
fn scalar_wide_bit_fields() {
    let mut gpu = Gpu::new();
    for name in ["s_bfe_i64", "s_bfe_u64", "s_bfm_b64"] {
        let cases: Vec<_> = [
            (0xFEDC_BA98_8765_4321, 1, 31), (0x8000_0000_0000_0000, 70, 63),
            (u64::MAX, 127, 127), (u64::MAX, 0, 0), (u64::MAX, 64, 64),
            (0x0123_4567_89AB_CDEF, 64, 12), (0, 20, 34), (32, 7, 62),
        ].into_iter().map(|(x, w, offset)| {
            let b = if name == "s_bfm_b64" { offset } else { w << 16 | offset };
            let field = |x: u64| if w >= 64 { x } else { x & ((1 << w) - 1) };
            let r = match name {
                "s_bfm_b64" => ((1 << (x & 63)) - 1) << (offset & 63),
                "s_bfe_u64" => field(x >> (offset & 63)),
                _ if w == 0 => 0,
                _ => {
                    let base = field(((x as i64) >> (offset & 63)) as u64);
                    let n = 64 - w.min(64);
                    (((base << n) as i64) >> n) as u64
                }
            };
            let scc = if name == "s_bfm_b64" { true } else { r != 0 };
            ([x as u32, (x >> 32) as u32, b, 0, 1, 0, 0, 0], words64(r, scc))
        }).collect();
        scalar(&mut gpu, Encoding::Sop2, name, 2, true, &cases);
    }
}
