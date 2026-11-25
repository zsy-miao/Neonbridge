//! Packed GFX10 dot-product lowering to SM 6.4 DXIL (`dot4AddPacked`) and
//! RDNA2-exact fused multiply-adds.

use super::{Attr, Bin, Cast, Fields, Inst, Lower, Operand, Reg, V, Why, flags, pred};
use crate::gfx10::Encoding;

impl<'a> Lower<'a> {
    pub(super) fn dot_product(
        &mut self,
        inst: &Inst,
        name: &str,
        guard: Option<V>,
    ) -> Result<(), Why> {
        let Fields::Valu(v) = &inst.fields else {
            unreachable!()
        };
        let c = self.cur;
        let s = |l: &mut Self, k| l.src(inst, k);
        let f = |l: &mut Self, k| l.fsrc(inst, k);
        let float = |l: &mut Self, r: V| l.write_f(inst, r, guard);
        let write_dot_int = |l: &mut Self, r: V| -> Result<(), Why> {
            if !v.clamp {
                return l.write_i(inst, r, guard);
            }
            if v.omod != 0 {
                return Err("integer output modifiers");
            }
            match v.dst {
                Some(Operand::Vector { reg, .. }) => l.write(inst, Reg::V(reg), r, guard),
                Some(Operand::Scalar { code, .. }) => l.write(inst, Reg::S(code), r, None),
                _ => return Err("a result without a register"),
            }
            Ok(())
        };
        let pack_bytes = |l: &mut Self, k: usize| -> Result<V, Why> {
            let x = s(l, k)?;
            let (lo, hi) = (v.opsel >> k & 1 != 0, v.opsel_hi >> k & 1 != 0);
            if (lo, hi) == (false, true) {
                return Ok(x);
            }
            let (k16, low) = (l.s.m.i32(16), l.s.m.i32(0xFFFF));
            let top = l.s.m.bin(c, Bin::LShr, x, k16);
            let bottom = l.s.m.bin(c, Bin::And, x, low);
            let hi_v = l.s.m.bin(c, Bin::Shl, if hi { top } else { bottom }, k16);
            Ok(l.s.m.bin(c, Bin::Or, if lo { top } else { bottom }, hi_v))
        };
        let sat_u32_add = |l: &mut Self, sum: V, acc: V| -> V {
            // Maximum 4x u8*u8 or 8x u4*u4 sum is <= 260,100 < 2^32, so wrapping occurs iff sum < acc.
            let wrapped = l.s.m.cmp(c, pred::IULT, sum, acc);
            let max_u = l.s.m.i32(u32::MAX);
            l.s.m.select(c, wrapped, max_u, sum)
        };
        let sat_64 = |l: &mut Self, sum: V, signed: bool| -> V {
            l.s.flags |= flags::INT64;
            let i64t = l.s.m.int(64);
            let clamped = if signed {
                let min_v = l.s.m.int_const(i64t, i64::from(i32::MIN));
                let max_v = l.s.m.int_const(i64t, i64::from(i32::MAX));
                let lo = l.s.m.cmp(c, pred::SLT, sum, min_v);
                let s1 = l.s.m.select(c, lo, min_v, sum);
                let hi = l.s.m.cmp(c, pred::SGT, s1, max_v);
                l.s.m.select(c, hi, max_v, s1)
            } else {
                let max_v = l.s.m.int_const(i64t, i64::from(u32::MAX));
                let hi = l.s.m.cmp(c, pred::IUGT, sum, max_v);
                l.s.m.select(c, hi, max_v, sum)
            };
            l.s.m.cast(c, Cast::Trunc, clamped, l.s.t.i32)
        };
        let sat_i32_add = |l: &mut Self, d: V, acc: V| -> V {
            l.s.flags |= flags::INT64;
            let i64t = l.s.m.int(64);
            let d64 = l.s.m.cast(c, Cast::SExt, d, i64t);
            let a64 = l.s.m.cast(c, Cast::SExt, acc, i64t);
            let sum = l.s.m.bin(c, Bin::Add, d64, a64);
            sat_64(l, sum, true)
        };

        match name {
            "v_dot4_i32_i8" | "v_dot4_u32_u8" => {
                if v.neg_hi != 0 {
                    return Err("a negated dot product");
                }
                let x0 = pack_bytes(self, 0)?;
                let x1 = pack_bytes(self, 1)?;
                let acc = s(self, 2)?;
                let i32_ty = self.s.t.i32;
                let signed = name == "v_dot4_i32_i8";
                let opcode = if signed { 163 } else { 164 };
                let r = if signed && v.clamp {
                    // dot4AddPacked with acc=0 stays within [-65024, +65536] and cannot overflow i32.
                    let zero = self.s.m.i32(0);
                    let d = self.s.op(
                        c,
                        "dot4AddPacked.i32",
                        Attr::ReadNone,
                        i32_ty,
                        opcode,
                        &[zero, x0, x1],
                    );
                    sat_i32_add(self, d, acc)
                } else {
                    let sum = self.s.op(
                        c,
                        "dot4AddPacked.i32",
                        Attr::ReadNone,
                        i32_ty,
                        opcode,
                        &[acc, x0, x1],
                    );
                    if !signed && v.clamp {
                        sat_u32_add(self, sum, acc)
                    } else {
                        sum
                    }
                };
                write_dot_int(self, r)
            }
            "v_dot4c_i32_i8" => {
                let (s0, s1) = (s(self, 0)?, s(self, 1)?);
                let Some(Operand::Vector { reg, .. }) = v.dst else {
                    return Err("an accumulator");
                };
                let acc = self.i32_of(inst.read(Reg::V(reg)).ok_or("an accumulator")?)?;
                let r = if v.clamp {
                    let zero = self.s.m.i32(0);
                    let d = self.s.op(
                        c,
                        "dot4AddPacked.i32",
                        Attr::ReadNone,
                        self.s.t.i32,
                        163,
                        &[zero, s0, s1],
                    );
                    sat_i32_add(self, d, acc)
                } else {
                    self.s.op(
                        c,
                        "dot4AddPacked.i32",
                        Attr::ReadNone,
                        self.s.t.i32,
                        163,
                        &[acc, s0, s1],
                    )
                };
                write_dot_int(self, r)
            }
            "v_dot2_f32_f16" | "v_dot2c_f32_f16" => {
                let f32t = self.s.t.f32;
                let is_vop3p = inst.machine.encoding == Encoding::Vop3p;
                let (s0_lo, s0_hi, s1_lo, s1_hi) = if is_vop3p {
                    (
                        self.packed_src(inst, 0, false, true)?,
                        self.packed_src(inst, 0, true, true)?,
                        self.packed_src(inst, 1, false, true)?,
                        self.packed_src(inst, 1, true, true)?,
                    )
                } else {
                    // VOP2/VOP3 v_dot2c_f32_f16 lacks opsel_hi: pair 0 reads [15:0], pair 1 reads [31:16].
                    let k16 = self.s.m.i32(16);
                    let mask = self.s.m.i32(0xFFFF);
                    let op0 = *v.src.first().ok_or("a missing operand")?;
                    let op1 = *v.src.get(1).ok_or("a missing operand")?;
                    let x0 = self.f16_operand(inst, &op0)?;
                    let x1 = self.f16_operand(inst, &op1)?;
                    (
                        self.s.m.bin(c, Bin::And, x0, mask),
                        self.s.m.bin(c, Bin::LShr, x0, k16),
                        self.s.m.bin(c, Bin::And, x1, mask),
                        self.s.m.bin(c, Bin::LShr, x1, k16),
                    )
                };

                let mut f_s0_lo =
                    self.s.op(c, "legacyF16ToF32", Attr::ReadNone, f32t, 131, &[s0_lo]);
                let mut f_s0_hi =
                    self.s.op(c, "legacyF16ToF32", Attr::ReadNone, f32t, 131, &[s0_hi]);
                let mut f_s1_lo =
                    self.s.op(c, "legacyF16ToF32", Attr::ReadNone, f32t, 131, &[s1_lo]);
                let mut f_s1_hi =
                    self.s.op(c, "legacyF16ToF32", Attr::ReadNone, f32t, 131, &[s1_hi]);

                let z = self.s.m.f32(0x8000_0000);
                if v.neg & 1 != 0 {
                    f_s0_lo = self.s.m.bin(c, Bin::Sub, z, f_s0_lo);
                    if !is_vop3p {
                        f_s0_hi = self.s.m.bin(c, Bin::Sub, z, f_s0_hi);
                    }
                }
                if is_vop3p && (v.neg_hi & 1 != 0) {
                    f_s0_hi = self.s.m.bin(c, Bin::Sub, z, f_s0_hi);
                }
                if (v.neg >> 1) & 1 != 0 {
                    f_s1_lo = self.s.m.bin(c, Bin::Sub, z, f_s1_lo);
                    if !is_vop3p {
                        f_s1_hi = self.s.m.bin(c, Bin::Sub, z, f_s1_hi);
                    }
                }
                if is_vop3p && ((v.neg_hi >> 1) & 1 != 0) {
                    f_s1_hi = self.s.m.bin(c, Bin::Sub, z, f_s1_hi);
                }

                let mut acc = if name == "v_dot2c_f32_f16" {
                    let Some(Operand::Vector { reg, .. }) = v.dst else {
                        return Err("an accumulator");
                    };
                    let acc_val = inst.read(Reg::V(reg)).ok_or("an accumulator")?;
                    match self.floats.get(&acc_val) {
                        Some(&x) => x,
                        None => {
                            let x = self.i32_of(acc_val)?;
                            self.as_float(x)
                        }
                    }
                } else {
                    f(self, 2)?
                };
                if v.neg >> 2 & 1 != 0 {
                    acc = self.s.m.bin(c, Bin::Sub, z, acc);
                }

                // Chained fused multiplies match RDNA2 rounding; write_f applies v.clamp and v.omod.
                let inner = self.fmad(f_s0_lo, f_s1_lo, acc);
                let r = self.fmad(f_s0_hi, f_s1_hi, inner);
                float(self, r)
            }
            "v_dot2_i32_i16" | "v_dot2_u32_u16" => {
                let signed = name == "v_dot2_i32_i16";
                let k16 = self.s.m.i32(16);
                let mut half = |k: usize, hi: bool| -> Result<V, Why> {
                    let x = self.packed_src(inst, k, hi, false)?;
                    if signed {
                        let t = self.s.m.bin(c, Bin::Shl, x, k16);
                        Ok(self.s.m.bin(c, Bin::AShr, t, k16))
                    } else {
                        Ok(x)
                    }
                };
                let (s0_lo, s0_hi, s1_lo, s1_hi) = (
                    half(0, false)?,
                    half(0, true)?,
                    half(1, false)?,
                    half(1, true)?,
                );
                let s2 = s(self, 2)?;
                let r = if v.clamp {
                    self.s.flags |= flags::INT64;
                    let i64t = self.s.m.int(64);
                    let ext = if signed { Cast::SExt } else { Cast::ZExt };
                    let w0 = self.s.m.cast(c, ext, s0_lo, i64t);
                    let w1 = self.s.m.cast(c, ext, s1_lo, i64t);
                    let w2 = self.s.m.cast(c, ext, s0_hi, i64t);
                    let w3 = self.s.m.cast(c, ext, s1_hi, i64t);
                    let w_acc = self.s.m.cast(c, ext, s2, i64t);
                    let p0 = self.s.m.bin(c, Bin::Mul, w0, w1);
                    let p1 = self.s.m.bin(c, Bin::Mul, w2, w3);
                    let sum = self.s.m.bin(c, Bin::Add, p0, p1);
                    let total = self.s.m.bin(c, Bin::Add, sum, w_acc);
                    sat_64(self, total, signed)
                } else {
                    let p0 = self.s.m.bin(c, Bin::Mul, s0_lo, s1_lo);
                    let p1 = self.s.m.bin(c, Bin::Mul, s0_hi, s1_hi);
                    let sum = self.s.m.bin(c, Bin::Add, p0, p1);
                    self.s.m.bin(c, Bin::Add, sum, s2)
                };
                write_dot_int(self, r)
            }
            "v_dot8_u32_u4" => {
                let (s0, s1, s2) = (s(self, 0)?, s(self, 1)?, s(self, 2)?);
                let mask = self.s.m.i32(0xF);
                let mut dot = self.s.m.i32(0);
                for i in 0..8 {
                    let sh = self.s.m.i32(i * 4);
                    let a_sh = self.s.m.bin(c, Bin::LShr, s0, sh);
                    let a = self.s.m.bin(c, Bin::And, a_sh, mask);
                    let b_sh = self.s.m.bin(c, Bin::LShr, s1, sh);
                    let b = self.s.m.bin(c, Bin::And, b_sh, mask);
                    let prod = self.s.m.bin(c, Bin::Mul, a, b);
                    dot = self.s.m.bin(c, Bin::Add, dot, prod);
                }
                let mut r = self.s.m.bin(c, Bin::Add, dot, s2);
                if v.clamp {
                    r = sat_u32_add(self, r, s2);
                }
                write_dot_int(self, r)
            }
            _ => Err("an unhandled dot product"),
        }
    }
}
