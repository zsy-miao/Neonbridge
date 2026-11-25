//! Source modifiers, results and comparisons.
use super::*;

impl<'a> Emitter<'a> {
    /// Float source with VOP3 abs/neg modifiers applied.
    pub(in crate::hlsl) fn fm(&self, ins: &Insn, i: usize) -> Result<String, String> {
        let mask = if ins.enc == Enc::Vop3 { ins.aux.mask } else { 0 };
        Ok(Self::float_source(f(&ins.src[i])?, mask, i))
    }

    pub(in crate::hlsl) fn select_source(ins: &Insn, mut bits: String, source: u8) -> String {
        if ins.enc == Enc::Vop3 {
            if ins.aux.mask & (1 << source) != 0 { bits = format!("({bits} & 0x7fffffffu)"); }
            if ins.aux.mask & (8 << source) != 0 { bits = format!("({bits} ^ 0x80000000u)"); }
        }
        bits
    }

    pub(in crate::hlsl) fn float_source(mut expr: String, mask: u8, i: usize) -> String {
        if mask & (1 << i) != 0 {
            expr = format!("abs({expr})");
        }
        if mask & (8 << i) != 0 {
            expr = format!("(-{expr})");
        }
        expr
    }

    pub(in crate::hlsl) fn float_result(ins: &Insn, mut e: String) -> String {
        if ins.enc == Enc::Vop3 {
            e = match ins.aux.imm2 {
                1 => format!("(({e}) * 2.0)"),
                2 => format!("(({e}) * 4.0)"),
                3 => format!("(({e}) * 0.5)"),
                _ => e,
            };
            if ins.aux.imm != 0 {
                e = format!("saturate({e})");
            }
        }
        e
    }

    pub(in crate::hlsl) fn fres(&self, ins: &Insn, e: String) -> String {
        format!("asuint({})", Self::float_result(ins, e))
    }

    pub(in crate::hlsl) fn pair_high(&mut self, operand: &Opnd) -> Result<String, String> {
        let high = match operand {
            Opnd::Vgpr(n) => Opnd::Vgpr(n.checked_add(1).ok_or("VGPR pair overflows")?),
            Opnd::Sgpr(0..=104) | Opnd::ExecLo | Opnd::VccLo | Opnd::ConstInt(_) | Opnd::Null => hi(operand),
            _ => return Err("unsupported 64-bit operand pair".into()),
        };
        self.touch(&high);
        self.numeric_input(&high)
    }

    pub(in crate::hlsl) fn numeric_input(&self, operand: &Opnd) -> Result<String, String> {
        match self.exec_snapshots.word(self.cur_idx, operand) {
            Some(word) => Ok(word),
            None => r(operand),
        }
    }

    pub(in crate::hlsl) fn int_compare(&self, m: &str, a: &Opnd, b: &Opnd) -> Result<String, String> {
        let mut parts = m.rsplit('_');
        let ty = parts.next().unwrap_or("");
        let cc = parts.next().unwrap_or("");
        let (x, y) = match ty {
            "i16" | "u16" => {
                let half = |o: &Opnd| -> Result<String, String> {
                    if matches!(o, Opnd::ConstF32(_)) {
                        return Err("inline float constant in a 16-bit integer compare".into());
                    }
                    let bits = self.numeric_input(o)?;
                    Ok(if ty == "i16" {
                        format!("(asint({bits} << 16) >> 16)")
                    } else {
                        format!("({bits} & 0xffffu)")
                    })
                };
                (half(a)?, half(b)?)
            }
            "i32" => (format!("asint({})", self.numeric_input(a)?), format!("asint({})", self.numeric_input(b)?)),
            "u32" => (self.numeric_input(a)?, self.numeric_input(b)?),
            other => return Err(format!("compare type {other} unsupported")),
        };
        Ok(match cc {
            "f" => "false".into(),
            "lt" => format!("({x} < {y})"),
            "eq" => format!("({x} == {y})"),
            "le" => format!("({x} <= {y})"),
            "gt" => format!("({x} > {y})"),
            "lg" | "ne" => format!("({x} != {y})"),
            "ge" => format!("({x} >= {y})"),
            "tru" | "t" => "true".into(),
            other => return Err(format!("compare cond {other} unsupported")),
        })
    }
}
