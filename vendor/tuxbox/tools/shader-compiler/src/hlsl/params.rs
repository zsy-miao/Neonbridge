//! Which signature elements each hardware parameter (interpolated attribute) carries.
use std::collections::BTreeSet;

use super::{Enc, Insn};
use crate::{meta::SigElement, ShaderExtract};

/// One four-channel parameter.
#[derive(Debug, Clone, PartialEq, Eq)]
pub(super) enum Param {
    /// One signature register, one component per channel.
    Full(u32),
    /// Interpolated half-float registers, two components per channel, each register starting
    /// at its own channel.
    Half(Vec<(u32, usize)>),
}

impl Param {
    pub fn registers(&self) -> Vec<u32> {
        match self {
            Param::Full(register) => vec![*register],
            Param::Half(rows) => rows.iter().map(|r| r.0).collect(),
        }
    }

    /// Channel of a register's component, and whether it is the upper half of that channel.
    pub fn channel(&self, register: u32, component: usize) -> Option<(usize, Option<bool>)> {
        match self {
            Param::Full(r) => (*r == register).then_some((component, None)),
            Param::Half(rows) => rows.iter().find(|r| r.0 == register)
                .map(|r| (r.1 + component / 2, Some(component % 2 != 0))),
        }
    }

    /// The register and component in one half of a channel.
    pub fn component(&self, channel: usize, high: bool) -> Option<(u32, usize)> {
        let Param::Half(rows) = self else { return None };
        let (register, first) = rows.iter().rev().find(|r| r.1 <= channel)?;
        Some((*register, (channel - first) * 2 + usize::from(high)))
    }
}

/// Assign the registers `keep` selects to parameters in signature order. A half-float register
/// that is interpolated shares the latest half-float parameter while its channels fit.
fn assign(elements: &[SigElement], flat: &[u32], keep: impl Fn(&SigElement) -> bool) -> Vec<Param> {
    let mut params: Vec<Param> = Vec::new();
    let mut seen = Vec::new();
    let mut open: Option<(usize, usize)> = None;
    for e in elements.iter().filter(|e| keep(e)) {
        if seen.contains(&e.register) {
            continue;
        }
        seen.push(e.register);
        let row = || elements.iter().filter(|o| o.register == e.register && keep(o));
        if flat.contains(&e.register) || row().any(|o| o.comp_type != 6) {
            params.push(Param::Full(e.register));
            continue;
        }
        let mask = row().fold(0u8, |m, o| m | o.mask);
        let channels = (8 - mask.leading_zeros() as usize).div_ceil(2);
        match open {
            Some((at, used)) if used + channels <= 4 => {
                if let Param::Half(rows) = &mut params[at] {
                    rows.push((e.register, used));
                }
                open = (used + channels < 4).then_some((at, used + channels));
            }
            _ => {
                open = (channels < 4).then_some((params.len(), channels));
                params.push(Param::Half(vec![(e.register, 0)]));
            }
        }
    }
    params
}

/// Parameters a vertex or mesh program exports for `elements`. A clip distance takes one too.
pub(super) fn outputs(ex: &ShaderExtract, elements: &[SigElement]) -> Vec<Param> {
    assign(elements, &ex.flat_outputs, |e| matches!(e.sysval, 0 | 2))
}

/// Parameters a pixel program reads. Programs differ in whether an input they never read keeps
/// its place; the interpolant count in the program's hardware state and the components its
/// instructions read tell them apart.
pub(super) fn inputs(ex: &ShaderExtract, insns: &[Insn]) -> Vec<Param> {
    let sig = &ex.input_sig;
    // A register counts as read if any element in it is, including a system value.
    let read = |register: u32| sig.iter().any(|o| o.register == register && o.rw_mask != 0);
    let used = assign(sig, &ex.flat_inputs, |e| e.sysval == 0 && read(e.register));
    let all = assign(sig, &ex.flat_inputs, |e| e.sysval == 0);
    let count = ex.fssm.get(0x238..).and_then(|b| b.first_chunk()).map(|b| u32::from_le_bytes(*b) as usize);
    let paired: Vec<Param> = all.iter().filter(|p| p.registers().into_iter().any(read)).cloned().collect();
    let reads = reads(insns);
    let mut fits: Vec<_> = [used.clone(), paired, all].into_iter().filter(|p| Some(p.len()) == count).collect();
    // The first of equals.
    fits.sort_by_key(|p| reads.iter().filter(|&&r| !carries(p, sig, r)).count());
    fits.into_iter().next().unwrap_or(used)
}

/// Parameter, channel and half (`None` for the whole channel) of each interpolation.
fn reads(insns: &[Insn]) -> BTreeSet<(usize, usize, Option<bool>)> {
    insns.iter().filter_map(|ins| match ins.enc {
        Enc::Vinterp => Some((ins.aux.imm as usize, ins.aux.imm2 as usize, None)),
        Enc::Vop3 if ins.mnemonic.starts_with("v_interp_p") && ins.mnemonic.ends_with("_f16") => {
            let (attr, channel, high) = super::Emitter::f16_interp_descriptor(*ins.raw.get(1)?);
            Some((attr, channel, Some(high)))
        }
        _ => None,
    }).collect()
}

/// Whether `params` carries a component the program declares it reads there.
fn carries(params: &[Param], sig: &[SigElement], (attr, channel, half): (usize, usize, Option<bool>)) -> bool {
    let Some(param) = params.get(attr) else { return false };
    let (register, component) = match (param, half) {
        (Param::Full(register), _) => (*register, channel),
        (Param::Half(_), None) => return false,
        (Param::Half(_), Some(high)) => match param.component(channel, high) {
            Some(found) => found,
            None => return false,
        },
    };
    sig.iter().any(|e| e.register == register && e.rw_mask >> component & 1 != 0)
}

/// Registers whose elements the pipeline validation part marks as constant over a primitive:
/// inputs, then outputs.
pub(crate) fn flat_registers(psv: &[u8]) -> Option<(Vec<u32>, Vec<u32>)> {
    fn word(b: &[u8], at: usize) -> Option<usize> {
        Some(u32::from_le_bytes(*b.get(at..)?.first_chunk()?) as usize)
    }
    let runtime = psv.get(4..)?.get(..word(psv, 0)?)?;
    let (inputs, outputs) = (*runtime.get(28)? as usize, *runtime.get(29)? as usize);
    let mut at = 4 + runtime.len();
    let resources = word(psv, at)?;
    at += 4;
    if resources != 0 {
        at += 4 + word(psv, at)?.checked_mul(resources)?;
    }
    for scale in [1, 4] {
        at += 4 + word(psv, at)?.checked_mul(scale)?;
    }
    let size = word(psv, at)?;
    at += 4;
    let mut flat = (Vec::new(), Vec::new());
    for n in 0..inputs + outputs {
        let e = psv.get(at + n.checked_mul(size)?..)?.get(..size)?;
        let (rows, start, kind, mode) = (*e.get(8)?, *e.get(9)?, *e.get(11)?, *e.get(13)?);
        if kind == 0 && mode == 1 {
            let list = if n < inputs { &mut flat.0 } else { &mut flat.1 };
            list.extend((0..u32::from(rows)).map(|r| u32::from(start) + r));
        }
    }
    Some(flat)
}

#[cfg(test)]
mod tests {
    use super::*;

    fn element(register: u32, comp_type: u32, mask: u8) -> SigElement {
        SigElement { name: "TEXCOORD".into(), index: register, sysval: 0, comp_type, register, mask, rw_mask: mask }
    }

    #[test]
    fn half_registers_share_parameters_by_channel() {
        let sig = [element(1, 6, 15), element(2, 1, 3), element(3, 6, 7), element(4, 6, 3), element(5, 6, 7), element(6, 6, 15)];
        let params = assign(&sig, &[6], |_| true);
        assert_eq!(params, vec![
            Param::Half(vec![(1, 0), (3, 2)]), Param::Full(2), Param::Half(vec![(4, 0), (5, 1)]), Param::Full(6),
        ]);
        assert_eq!(params[0].component(3, false), Some((3, 2)));
        assert_eq!(params[2].component(1, true), Some((5, 1)));
        assert_eq!(params[2].channel(5, 2), Some((2, Some(false))));
        assert_eq!(params[3].channel(6, 3), Some((3, None)));
    }
}
