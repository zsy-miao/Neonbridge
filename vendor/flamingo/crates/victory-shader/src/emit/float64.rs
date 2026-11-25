//! 64-bit floats: `v_add_f64`, `v_mul_f64` and `v_cvt_f64_f32` as the host's doubles,
//! `v_cvt_f32_f64` on 32-bit integers, and the 64-bit float wave sums of whole-wave sections
//! ([`super::super::wave::Shape::Sum64`]). The title's water volumetrics count their lanes in
//! doubles (`WaveActiveSum(1.0L)`, `v_add_f64` over DPP-moved dword pairs) and take the count
//! back as a float.
//!
//! The host's doubles are IEEE, rounded to nearest with denormals kept, as the console's FP64
//! mode is. A double is made from its two dwords (`dx.op.makeDouble`) and taken apart again
//! (`dx.op.splitDouble`), so no 64-bit integer is needed. `v_cvt_f32_f64` takes the double's
//! dwords apart itself, rounding as IEEE does with float denormals kept, so a conversion alone
//! asks no double support of the host.

use super::{Attr, Bin, Cast, Fields, Inst, Lower, MaskData, Operand, Reg, V, Value, Why, flags, need, pred};
use crate::gfx10::NULL;

impl Lower<'_> {
    /// The 64-bit float operations, if `name` is one.
    pub(super) fn valu_f64(&mut self, inst: &Inst, name: &str, guard: Option<V>) -> Option<Result<(), Why>> {
        let c = self.cur;
        Some(match name {
            "v_cvt_f32_f64" => self.f64_pair(inst, 0).and_then(|(lo, hi)| {
                let bits = self.f64_to_f32_bits(lo, hi);
                let r = self.as_float(bits);
                self.write_f(inst, r, guard)
            }),
            "v_cvt_f64_f32" => self.fsrc(inst, 0).and_then(|x| {
                let f64t = self.s.m.double();
                self.s.flags |= flags::DOUBLES;
                let r = self.s.m.cast(c, Cast::FpExt, x, f64t);
                self.write_f64(inst, r, guard)
            }),
            "v_add_f64" | "v_mul_f64" => (|| {
                let (a, b) = (self.fsrc64(inst, 0)?, self.fsrc64(inst, 1)?);
                let op = if name == "v_add_f64" { Bin::Add } else { Bin::Mul };
                let r = self.s.m.bin(c, op, a, b);
                self.write_f64(inst, r, guard)
            })(),
            _ => return None,
        })
    }

    /// The 64-bit float `WaveActiveSum` over the console's wave of each lane's own
    /// `mask ? (lo, hi) : 0.0`, as its two dwords: the host's wave operation on doubles, and
    /// on two host waves of 32 their sums added (the lower half's first). Made once per
    /// block, for both dwords.
    /// shortcut: the host adds a wave's terms in its own order, not the console's tree of
    /// lanes; the sum can differ in its last bits where terms round (as for 32-bit sums).
    pub(super) fn sum64(&mut self, lo: Value, hi: Value, mask: Value) -> Result<[V; 2], Why> {
        let key = (lo, hi, mask, self.cur);
        if let Some(&d) = self.sums64.get(&key) {
            return Ok(d);
        }
        if self.mask_data == MaskData::None {
            return Err("a wave reduction in a mesh shader");
        }
        let c = self.cur;
        let &m = self.x.st.masks.get(&mask).ok_or("a wave reduction's untracked mask")?;
        let on = self.boolean(m, c)?;
        let (l, h) = (self.i32_of(lo)?, self.i32_of(hi)?);
        let x = self.masked_double(on, l, h);
        let r = self.wave_sum_f64(x);
        // two host waves of 32 hold the console's 64 lanes: their sums add up
        let r = if self.local || self.exchange.as_ref().is_some_and(|e| e.1) {
            r
        } else if self.exchange.is_some() {
            if !self.all_lanes(self.cur_ir) {
                need(self.cur_ir);
                return Err("a wave reduction where not every lane runs");
            }
            let (rl, rh) = self.split_double(r);
            let c = self.cur;
            let (lower_lo, upper_lo) = self.exchange_halves(c, rl);
            let (lower_hi, upper_hi) = self.exchange_halves(c, rh);
            let lower = self.make_double(lower_lo, lower_hi);
            let upper = self.make_double(upper_lo, upper_hi);
            self.s.m.bin(self.cur, Bin::Add, lower, upper)
        } else {
            r
        };
        let (dl, dh) = self.split_double(r);
        self.sums64.insert(key, [dl, dh]);
        Ok([dl, dh])
    }

    /// [`Self::sum64`] in the paired form: the lower lanes' sum, then the upper lanes' added,
    /// as two host waves add theirs; dword `high` of it, for both lanes of this thread.
    pub(super) fn paired_sum64(&mut self, high: bool, lo: Value, hi: Value, mask: Value) -> Result<[V; 2], Why> {
        let key = (lo, hi, mask, self.cur);
        let d = match self.sums64.get(&key) {
            Some(&d) => d,
            None => {
                let &m = self.x.st.masks.get(&mask).ok_or("a wave reduction's untracked mask")?;
                let c = self.cur;
                let [x0, x1] = self.both(|l| {
                    let on = l.boolean(m, c)?;
                    let (dl, dh) = (l.i32_of(lo)?, l.i32_of(hi)?);
                    Ok(l.masked_double(on, dl, dh))
                })?;
                let (s0, s1) = (self.wave_sum_f64(x0), self.wave_sum_f64(x1));
                let total = self.s.m.bin(self.cur, Bin::Add, s0, s1);
                let (dl, dh) = self.split_double(total);
                self.sums64.insert(key, [dl, dh]);
                [dl, dh]
            }
        };
        let x = d[usize::from(high)];
        Ok([x, x])
    }

    /// `on ? (lo, hi) : +0.0` as a double.
    fn masked_double(&mut self, on: V, lo: V, hi: V) -> V {
        let x = self.make_double(lo, hi);
        let z = self.s.m.i32(0);
        let zero = self.make_double(z, z);
        self.s.m.select(self.cur, on, x, zero)
    }

    /// `WaveActiveSum` of the double `x` over this host wave's active lanes.
    fn wave_sum_f64(&mut self, x: V) -> V {
        self.s.flags |= flags::WAVE_OPS;
        let c = self.cur;
        let f64t = self.s.m.double();
        let (sum, signed) = (self.s.m.i8(0), self.s.m.i8(0));
        self.s.op(c, "waveActiveOp.f64", Attr::NoUnwind, f64t, 119, &[x, sum, signed])
    }

    /// The double whose dwords are `lo` and `hi` (`dx.op.makeDouble`).
    fn make_double(&mut self, lo: V, hi: V) -> V {
        let c = self.cur;
        let f64t = self.s.m.double();
        self.s.flags |= flags::DOUBLES;
        self.s.op(c, "makeDouble.f64", Attr::ReadNone, f64t, 101, &[lo, hi])
    }

    /// The low and high dwords of the double `d` (`dx.op.splitDouble`).
    fn split_double(&mut self, d: V) -> (V, V) {
        let c = self.cur;
        let i32t = self.s.t.i32;
        let pair = self.s.m.named_struct("dx.types.splitdouble", vec![i32t, i32t]);
        let r = self.s.op(c, "splitDouble.f64", Attr::ReadNone, pair, 102, &[d]);
        (self.s.m.extract(c, r, 0), self.s.m.extract(c, r, 1))
    }

    /// A 64-bit float source as a double ([`Self::f64_pair`]).
    fn fsrc64(&mut self, inst: &Inst, k: usize) -> Result<V, Why> {
        let (lo, hi) = self.f64_pair(inst, k)?;
        Ok(self.make_double(lo, hi))
    }

    /// Reads an IEEE-754 binary64 operand as `(lo, hi)` 32-bit words, applying
    /// `abs` and `neg` input modifiers on operand `k`.
    fn f64_pair(&mut self, inst: &Inst, k: usize) -> Result<(V, V), Why> {
        let Fields::Valu(v) = &inst.fields else {
            unreachable!()
        };
        if v.sdwa.is_some() || v.dpp.is_some() {
            return Err("64-bit operand modifiers");
        }
        let op = *v.src.get(k).ok_or("a missing operand")?;
        let pair = |l: &mut Self, r: Reg, r1: Reg| -> Result<(V, V), Why> {
            let lo = l.i32_of(inst.read(r).ok_or("an unread operand")?)?;
            let hi = l.i32_of(inst.read(r1).ok_or("an unread operand")?)?;
            Ok((lo, hi))
        };
        let (lo, mut hi) = match op {
            Operand::Vector { reg, .. } => pair(self, Reg::V(reg), Reg::V(reg + 1))?,
            Operand::Scalar { code, .. } if code != NULL => {
                pair(self, Reg::S(code), Reg::S(code + 1))?
            }
            Operand::Const(128) => (self.s.m.i32(0), self.s.m.i32(0)),
            Operand::Const(c @ 240..=248) => {
                const INLINE_F64: [f64; 9] = [
                    0.5,
                    -0.5,
                    1.0,
                    -1.0,
                    2.0,
                    -2.0,
                    4.0,
                    -4.0,
                    // 1 / (2 pi) as the hardware holds it (the nearest double ends in ...883)
                    f64::from_bits(0x3FC4_5F30_6DC9_C882),
                ];
                let bits = INLINE_F64[(c - 240) as usize].to_bits();
                (self.s.m.i32(bits as u32), self.s.m.i32((bits >> 32) as u32))
            }
            // GFX10 places a 32-bit literal in the high 32 bits of a 64-bit float operand.
            Operand::Literal(x) => (self.s.m.i32(0), self.s.m.i32(x)),
            _ => return Err("a 64-bit float operand"),
        };
        let c = self.cur;
        if v.abs >> k & 1 != 0 {
            let mask = self.s.m.i32(0x7FFF_FFFF);
            hi = self.s.m.bin(c, Bin::And, hi, mask);
        }
        if v.neg >> k & 1 != 0 {
            let sign = self.s.m.i32(0x8000_0000);
            hi = self.s.m.bin(c, Bin::Xor, hi, sign);
        }
        Ok((lo, hi))
    }

    /// Converts IEEE-754 binary64 `(lo, hi)` to binary32 bits with round-to-nearest-even,
    /// without requiring host `DOUBLES` (`f64`) support.
    fn f64_to_f32_bits(&mut self, lo: V, hi: V) -> V {
        let c = self.cur;
        let k0 = self.s.m.i32(0);
        let k1 = self.s.m.i32(1);
        let sign_mask = self.s.m.i32(0x8000_0000);
        let sign = self.s.m.bin(c, Bin::And, hi, sign_mask);

        let k20 = self.s.m.i32(20);
        let k7ff = self.s.m.i32(0x7FF);
        let exp64 = self.s.m.bin(c, Bin::LShr, hi, k20);
        let exp64 = self.s.m.bin(c, Bin::And, exp64, k7ff);

        let frac_mask = self.s.m.i32(0x000F_FFFF);
        let frac_hi = self.s.m.bin(c, Bin::And, hi, frac_mask);
        let frac_any = self.s.m.bin(c, Bin::Or, frac_hi, lo);
        let frac_nz = self.s.m.cmp(c, pred::NE, frac_any, k0);

        let k3 = self.s.m.i32(3);
        let k29 = self.s.m.i32(29);
        let m23_hi = self.s.m.bin(c, Bin::Shl, frac_hi, k3);
        let m23_lo = self.s.m.bin(c, Bin::LShr, lo, k29);
        let m23 = self.s.m.bin(c, Bin::Or, m23_hi, m23_lo);

        let rem_mask = self.s.m.i32(0x1FFF_FFFF);
        let rem = self.s.m.bin(c, Bin::And, lo, rem_mask);
        let half_rem = self.s.m.i32(0x1000_0000);
        let gt_half = self.s.m.cmp(c, pred::IUGT, rem, half_rem);
        let eq_half = self.s.m.cmp(c, pred::EQ, rem, half_rem);
        let m23_odd_bit = self.s.m.bin(c, Bin::And, m23, k1);
        let m23_odd = self.s.m.cmp(c, pred::NE, m23_odd_bit, k0);
        let tie_up = self.s.m.bin(c, Bin::And, eq_half, m23_odd);
        let norm_round = self.s.m.bin(c, Bin::Or, gt_half, tie_up);
        let norm_inc = self.s.m.select(c, norm_round, k1, k0);

        // Normal range: exp64 in [897, 1150] -> exp32 in [1, 254].
        let k896 = self.s.m.i32(896);
        let k23 = self.s.m.i32(23);
        let exp32 = self.s.m.bin(c, Bin::Sub, exp64, k896);
        let exp32_sh = self.s.m.bin(c, Bin::Shl, exp32, k23);
        let norm_body = self.s.m.bin(c, Bin::Add, exp32_sh, m23);
        let norm_body = self.s.m.bin(c, Bin::Add, norm_body, norm_inc);
        let norm_res = self.s.m.bin(c, Bin::Or, sign, norm_body);

        // Subnormal range: exp64 in [874, 896] -> shift in [1, 23].
        let k897 = self.s.m.i32(897);
        let k32 = self.s.m.i32(32);
        let k31 = self.s.m.i32(31);
        let imp = self.s.m.i32(0x0080_0000);
        let sig24 = self.s.m.bin(c, Bin::Or, imp, m23);
        let raw_shift = self.s.m.bin(c, Bin::Sub, k897, exp64);
        // Clamp shift to [1, 23] so shifts never produce poison outside the subnormal branch.
        let lt1 = self.s.m.cmp(c, pred::IULT, raw_shift, k1);
        let shift = self.s.m.select(c, lt1, k1, raw_shift);
        let gt23 = self.s.m.cmp(c, pred::IUGT, shift, k23);
        let shift = self.s.m.select(c, gt23, k23, shift);
        let sub_m = self.s.m.bin(c, Bin::LShr, sig24, shift);
        let inv_shift = self.s.m.bin(c, Bin::Sub, k32, shift);
        let lost = self.s.m.bin(c, Bin::Shl, sig24, inv_shift);
        let round_bit = self.s.m.bin(c, Bin::LShr, lost, k31);
        let round_on = self.s.m.cmp(c, pred::NE, round_bit, k0);
        let low31 = self.s.m.i32(0x7FFF_FFFF);
        let lost_lo = self.s.m.bin(c, Bin::And, lost, low31);
        let sticky = self.s.m.bin(c, Bin::Or, lost_lo, rem);
        let sticky_on = self.s.m.cmp(c, pred::NE, sticky, k0);
        let sub_odd_bit = self.s.m.bin(c, Bin::And, sub_m, k1);
        let sub_odd = self.s.m.cmp(c, pred::NE, sub_odd_bit, k0);
        let sub_tie = self.s.m.bin(c, Bin::Or, sticky_on, sub_odd);
        let sub_round = self.s.m.bin(c, Bin::And, round_on, sub_tie);
        let sub_inc = self.s.m.select(c, sub_round, k1, k0);
        let sub_body = self.s.m.bin(c, Bin::Add, sub_m, sub_inc);
        let sub_res = self.s.m.bin(c, Bin::Or, sign, sub_body);

        // Underflow range: exp64 < 874. Only exp64 == 873 with non-zero fraction rounds up to min subnormal (1).
        let k873 = self.s.m.i32(873);
        let eq873 = self.s.m.cmp(c, pred::EQ, exp64, k873);
        let round_min = self.s.m.bin(c, Bin::And, eq873, frac_nz);
        let min_bit = self.s.m.select(c, round_min, k1, k0);
        let under_res = self.s.m.bin(c, Bin::Or, sign, min_bit);

        // Overflow / Inf / NaN: exp64 >= 1151.
        let is_exp_all = self.s.m.cmp(c, pred::EQ, exp64, k7ff);
        let is_nan = self.s.m.bin(c, Bin::And, is_exp_all, frac_nz);
        let inf_bits = self.s.m.i32(0x7F80_0000);
        let qnan_bits = self.s.m.i32(0x7FC0_0000);
        let nan_body = self.s.m.bin(c, Bin::Or, qnan_bits, m23);
        let over_body = self.s.m.select(c, is_nan, nan_body, inf_bits);
        let over_res = self.s.m.bin(c, Bin::Or, sign, over_body);

        // Select final result by exponent range.
        let k874 = self.s.m.i32(874);
        let k1151 = self.s.m.i32(1151);
        let is_under = self.s.m.cmp(c, pred::IULT, exp64, k874);
        let is_sub = self.s.m.cmp(c, pred::IULT, exp64, k897);
        let is_over = self.s.m.cmp(c, pred::IUGE, exp64, k1151);
        let r = self.s.m.select(c, is_over, over_res, norm_res);
        let r = self.s.m.select(c, is_sub, sub_res, r);
        self.s.m.select(c, is_under, under_res, r)
    }

    /// A 64-bit float result into its VGPR pair.
    fn write_f64(&mut self, inst: &Inst, d: V, guard: Option<V>) -> Result<(), Why> {
        let Fields::Valu(v) = &inst.fields else {
            unreachable!()
        };
        if v.clamp || v.omod != 0 {
            return Err("64-bit float output modifiers");
        }
        let Some(Operand::Vector { reg, .. }) = v.dst else {
            return Err("a 64-bit result");
        };
        let (lo, hi) = self.split_double(d);
        self.write(inst, Reg::V(reg), lo, guard);
        self.write(inst, Reg::V(reg + 1), hi, guard);
        Ok(())
    }
}
