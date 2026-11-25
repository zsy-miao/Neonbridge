//! Append-style wave code with every lane as a wave of its own.
//!
//! One elected lane adds the number of lanes that want an element to a counter, the
//! base is broadcast with an additive DPP reduction, and each lane adds the number of
//! wanting lanes before it. A one-lane wave allocates its own element instead: the same
//! elements are appended, in another order. Anything that could observe the wave
//! (a lane index, a float reduction, a plain store by the elected lane) is refused.
//!
//! OR, AND, MIN and MAX over the wave let one scalar loop cover every lane's lights or
//! bins; each lane still tests its own. One lane needs only its own.
use super::{Enc, Insn, Opnd, may_touch_sgpr};
use std::cell::Cell;

thread_local!(static ON: Cell<bool> = const { Cell::new(false) });

pub(super) fn on() -> bool { ON.with(Cell::get) }

pub(super) fn retry<T>(run: impl FnOnce() -> T) -> T {
    ON.with(|c| c.set(true));
    let result = run();
    ON.with(|c| c.set(false));
    result
}

fn dpp(ins: &Insn) -> bool { matches!(ins.src.first(), Some(Opnd::Raw(233 | 234 | 250))) }

/// Sources without the unused third field of two-source VOP3 forms.
fn sources(ins: &Insn) -> &[Opnd] {
    let two = ins.mnemonic.starts_with("v_cmp") || ins.mnemonic.starts_with("v_mbcnt_") || ins.mnemonic.starts_with("v_readlane");
    &ins.src[..if two { ins.src.len().min(2) } else { ins.src.len() }]
}

fn idempotent(m: &str) -> bool {
    matches!(m, "v_or_b32" | "v_and_b32" | "v_min_u32" | "v_max_u32" | "v_min_i32" | "v_max_i32")
}

/// Whether the other half of the wave, copied to `o`, joins an idempotent reduction.
fn joins_idempotent(insns: &[Insn], at: usize, o: &Opnd) -> bool {
    insns[at + 1..].iter().find(|ins| ins.src.contains(o)).is_some_and(|ins| idempotent(&ins.mnemonic))
}

pub(super) fn prepare(insns: &mut [Insn]) -> Result<(), String> {
    // DXC needs minutes for the HLSL of larger programs.
    if insns.len() > 6000 { return Err("lone lane: program too large".into()); }
    let mut elections = 0;
    for i in 0..insns.len() {
        let m = insns[i].mnemonic.as_str();
        let at = insns[i].offset;
        if matches!(m, "ds_swizzle_b32" | "ds_permute_b32" | "ds_bpermute_b32" | "ds_ordered_count") || m.starts_with("ds_append") || m.starts_with("ds_consume") {
            return Err(format!("lone lane: {m} at {at:#x}"));
        }
        if m.starts_with("s_ff1_i32_b") && insns[i].src.first() == Some(&Opnd::ExecLo) {
            let Some(&Opnd::Sgpr(lane)) = insns[i].dst.first() else { return Err("lone lane: election register".into()) };
            let cmp = (i + 1..insns.len())
                .find(|&j| sources(&insns[j]).iter().chain(&insns[j].dst).any(|o| *o == Opnd::Sgpr(lane) || matches!(insns[j].enc, Enc::Smem) && may_touch_sgpr(o, lane)))
                .ok_or("lone lane: unused election")?;
            let c = &insns[cmp];
            let eq = matches!(c.mnemonic.as_str(), "v_cmp_eq_i32" | "v_cmp_eq_u32" | "v_cmpx_eq_i32" | "v_cmpx_eq_u32");
            if !eq || c.src.first() != Some(&Opnd::Sgpr(lane)) {
                // The lane of a first-lane read.
                if c.mnemonic == "v_readlane_b32" && c.src.get(1) == Some(&Opnd::Sgpr(lane)) { continue; }
                return Err(format!("lone lane: election at {at:#x} used by {}", c.mnemonic));
            }
            let start = if c.mnemonic.starts_with("v_cmpx") { cmp + 1 } else {
                match (cmp + 1..insns.len()).find(|&j| insns[j].mnemonic.starts_with("s_and_saveexec")) {
                    Some(save) => save + 1,
                    None => return Err(format!("lone lane: elected mask at {at:#x}")),
                }
            };
            // A whole-wave section inside the branch saves and restores EXEC itself.
            let mut nested = 0;
            let mut end = insns.len();
            for j in start..insns.len() {
                let e = &insns[j];
                if e.mnemonic.starts_with("s_or_saveexec") { nested += 1; }
                if e.mnemonic.starts_with("s_mov_b64") && e.dst.first() == Some(&Opnd::ExecLo) {
                    if nested == 0 { end = j; break; }
                    nested -= 1;
                }
            }
            let mut atomics = 0;
            for e in &insns[start..end] {
                let n = e.mnemonic.as_str();
                if n.contains("_atomic_") || n.starts_with("ds_add") || n.starts_with("ds_sub") || n.starts_with("ds_or") || n.starts_with("ds_and")
                    || n.starts_with("ds_min") || n.starts_with("ds_max") || n.starts_with("ds_inc") || n.starts_with("ds_dec") {
                    atomics += 1;
                } else if n.contains("store") || n.starts_with("ds_write") || n.starts_with("exp") {
                    return Err(format!("lone lane: {n} in an elected branch at {:#x}", e.offset));
                }
            }
            if atomics == 0 { return Err(format!("lone lane: no atomic in the elected branch at {at:#x}")); }
            elections += 1;
        }
    }
    let shared = insns.iter().any(|ins| ins.enc == Enc::Vop2 && dpp(ins) && idempotent(&ins.mnemonic));
    if elections == 0 && !shared { return Err("lone lane: no election or reduction".into()); }
    // A lane index may only meet the elected lane.
    for i in 0..insns.len() {
        if !insns[i].mnemonic.starts_with("v_mbcnt_lo") || insns[i].src.first() != Some(&Opnd::ConstInt(-1)) { continue; }
        let Some(&Opnd::Vgpr(v)) = insns[i].dst.first() else { continue };
        let mut tracked = vec![v];
        for j in i + 1..insns.len() {
            let ins = &insns[j];
            let reads = sources(ins).iter().any(|o| matches!(o, Opnd::Vgpr(n) if tracked.contains(n)));
            let m = ins.mnemonic.as_str();
            if reads {
                if m.starts_with("v_mbcnt_hi") && ins.src.first() == Some(&Opnd::ConstInt(-1)) {
                    if let Some(&Opnd::Vgpr(n)) = ins.dst.first() { if !tracked.contains(&n) { tracked.push(n); } continue; }
                } else if !(m.starts_with("v_cmp") && m.contains("_eq_") && matches!(ins.src.first(), Some(Opnd::Sgpr(_)))) {
                    return Err(format!("lone lane: lane index used by {m} at {:#x}", ins.offset));
                }
            }
            tracked.retain(|n| !ins.dst.contains(&Opnd::Vgpr(*n)));
            if tracked.is_empty() { break; }
        }
    }
    let spilled: Vec<u8> = insns.iter().filter(|i| i.mnemonic == "v_writelane_b32")
        .filter_map(|i| if let Some(&Opnd::Vgpr(v)) = i.dst.first() { Some(v) } else { None }).collect();
    for k in 0..insns.len() {
        let other_half = match insns[k].dst.first() {
            // A lane read's scalar destination decodes as a VGPR.
            Some(&Opnd::Vgpr(n)) if insns[k].mnemonic == "v_readlane_b32" => joins_idempotent(insns, k, &Opnd::Sgpr(n)),
            Some(o) => joins_idempotent(insns, k, &o.clone()),
            None => false,
        };
        let ins = &mut insns[k];
        let at = ins.offset;
        let m = ins.mnemonic.clone();
        if ins.enc == Enc::Vop2 && dpp(ins) {
            // Signed extrema can be observed outside the append loop: a lane-local
            // copy loses their cross-lane result if exact lowering has failed.
            if matches!(m.as_str(), "v_min_i32" | "v_max_i32") || m != "v_add_nc_u32" && !idempotent(&m) { return Err(format!("lone lane: {m} across lanes at {at:#x}")); }
            // The other lanes hold zero.
            let own = ins.src.get(1).cloned().ok_or("lone lane: operand")?;
            ins.mnemonic = "v_mov_b32".into();
            ins.enc = Enc::Vop1;
            ins.op = 1;
            ins.src = vec![own];
        } else if dpp(ins) && ins.enc == Enc::Vop1 {
            return Err(format!("lone lane: {m} across lanes at {at:#x}"));
        } else if m.starts_with("v_permlane") {
            ins.mnemonic = "v_mov_b32".into();
            ins.enc = Enc::Vop1;
            ins.op = 1;
            ins.src = vec![if other_half { ins.src[0].clone() } else { Opnd::ConstInt(0) }];
        } else if m == "v_readlane_b32" {
            let Some(&Opnd::Vgpr(v)) = ins.src.first() else { return Err(format!("lone lane: {m} at {at:#x}")) };
            if spilled.contains(&v) { continue; }
            match ins.src.get(1) {
                // Lane 0 itself, or the last lane of a half of an inclusive scan, which holds
                // this lane's value too.
                Some(Opnd::ConstInt(0 | 31 | 63)) | Some(Opnd::Sgpr(_)) => {
                    ins.mnemonic = "v_readfirstlane_b32".into();
                    ins.src.truncate(1);
                    ins.enc = Enc::Vop1;
                    ins.op = 2;
                    if let Some(Opnd::Vgpr(n)) = ins.dst.first().cloned() { ins.dst = vec![Opnd::Sgpr(n)]; }
                }
                Some(Opnd::ConstInt(32)) if other_half => {
                    ins.mnemonic = "v_readfirstlane_b32".into();
                    ins.src.truncate(1);
                    ins.enc = Enc::Vop1;
                    ins.op = 2;
                    if let Some(Opnd::Vgpr(n)) = ins.dst.first().cloned() { ins.dst = vec![Opnd::Sgpr(n)]; }
                }
                Some(Opnd::ConstInt(32)) => {
                    let Some(dst) = ins.dst.first().cloned() else { return Err("lone lane: readlane".into()) };
                    ins.dst = vec![if let Opnd::Vgpr(n) = dst { Opnd::Sgpr(n) } else { dst }];
                    ins.mnemonic = "s_mov_b32".into();
                    ins.enc = Enc::Sop1;
                    ins.op = 3;
                    ins.src = vec![Opnd::ConstInt(0)];
                }
                other => return Err(format!("lone lane: readlane {other:?} at {at:#x}")),
            }
        }
    }
    Ok(())
}

/// Nothing may depend on the host's waves.
pub(super) fn check(hlsl: &str) -> Result<(), String> {
    let at = hlsl.find("void main(").or_else(|| hlsl.find(" main(")).unwrap_or(0);
    for word in ["Wave", "QuadRead"] {
        if let Some(p) = hlsl[at..].find(word) {
            if std::env::var_os("TB_DFDEBUG").is_some() {
                let line = hlsl[..at + p].rfind('\n').map_or(0, |n| n + 1);
                eprintln!("wave use: {}", hlsl[line..].lines().next().unwrap_or(""));
            }
            return Err(format!("lone lane: output uses {}", hlsl[at + p..].split('(').next().unwrap_or(word)));
        }
    }
    Ok(())
}
