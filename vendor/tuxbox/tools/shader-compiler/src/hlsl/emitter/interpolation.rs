//! Pixel attribute interpolation.
use super::*;

impl<'a> Emitter<'a> {
    /// Read an interpolated attribute.
    pub(in crate::hlsl) fn emit_vinterp(&mut self, ins: &Insn, is_ps: bool) -> Result<(), String> {
        let s = &ins.src;
        let d = ins.dst.first().cloned();
        if !is_ps {
            return Err("v_interp outside a pixel shader".into());
        }
        let attr = ins.aux.imm as usize;
        let chan = ins.aux.imm2 as usize;
        let Param::Full(reg) = *self.ps_attrs.get(attr).ok_or_else(|| format!("PS attr{attr} has no signature register"))? else {
            return Err(format!("32-bit read of half-float PS attr{attr}"));
        };
        let val = if let Some((ei, k)) = sig_lookup(&self.ex.input_sig, reg, chan) {
            let acc = format!("i.i{ei}.{}", XYZW[k]);
            if matches!(self.ex.input_sig[ei].comp_type, 3 | 6) { format!("asuint({acc})") } else { format!("(uint)({acc})") }
        } else if chan == 3 {
            "0x3f800000u".to_string()
        } else {
            "0u".to_string()
        };
        if self.pulled_attrs.contains(&attr) {
            if let Some((ei, k)) = sig_lookup(&self.ex.input_sig, reg, chan) {
                let (_, n) = sig_type(&self.ex.input_sig[ei]);
                let field = if n == 1 { format!("i.i{ei}") } else { format!("i.i{ei}.{}", XYZW[k]) };
                let vertex = |index| {
                    let bits = format!("GetAttributeAtVertex({field}, {index})");
                    if matches!(self.ex.input_sig[ei].comp_type, 3 | 6) { bits } else { format!("asfloat({bits})") }
                };
                let p0 = vertex(0);
                let delta = |v| format!("({} - {p0})", vertex(v));
                let dst = d.ok_or("no interpolation destination")?;
                let expr = match ins.op {
                    0 => format!("{p0} + {} * {}", f(&s[0])?, delta(1)),
                    1 => format!("{} + {} * {}", f(&dst)?, f(&s[0])?, delta(2)),
                    2 => match ins.raw[0] & 255 {
                        0 => delta(1), 1 => delta(2), 2 => p0, _ => return Err("invalid interpolation parameter".into()),
                    },
                    _ => return Err("unsupported interpolation opcode".into()),
                };
                self.line("#if __SHADER_TARGET_MAJOR >= 6");
                self.wv(&dst, &format!("asuint({expr})"))?;
                self.line("#else");
                self.wv(&dst, &val)?;
                self.line("#endif");
                return Ok(());
            }
            // A missing channel is constant at all three vertices.
            let dst = d.ok_or("no interpolation destination")?;
            let constant = match ins.op {
                1 => return Ok(()), // the second delta is zero
                2 if matches!(ins.raw[0] & 255, 0 | 1) => "0u",
                _ => &val,
            };
            return self.wv(&dst, constant);
        }
        // p1 computes a partial term in hardware; the full interpolated value is
        // already available per-pixel here, so both steps assign it.
        self.wv(&d.ok_or("no dst")?, &val)
    }
}
