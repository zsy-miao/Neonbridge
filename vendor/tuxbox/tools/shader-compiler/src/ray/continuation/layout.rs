//! Validate native scheduler and stack ABI and locate the title's separately
//! callable continuations. The trailing program word encodes its own extent.
use super::super::{alu::whole_sdwa_words, fingerprint};
use super::target;
use crate::rdna2::{Enc, Insn, Opnd, instructions};
use std::ops::Range;
use tuxbox_dxbc::ray::State;

pub(super) struct Layout {
    pub entries: Vec<Range<usize>>,
    pub frame: usize,
    pub forward: bool,
}

fn word(bytes: &[u8], offset: usize) -> Option<usize> {
    Some(u32::from_le_bytes(*bytes.get(offset..)?.first_chunk()?) as usize)
}

fn immediate(arg: &Opnd) -> Option<i64> {
    match arg {
        Opnd::ConstInt(v) => Some(*v),
        Opnd::Literal(v) => Some(i64::from(*v as i32)),
        _ => None,
    }
}

fn frame_adjustment(code: &[Insn], index: usize) -> Option<i64> {
    let ins = &code[index];
    if !whole_sdwa_words(ins) {
        return None;
    }
    let [arg, base @ Opnd::Vgpr(_)] = ins.src.as_slice() else {
        return None;
    };
    let value = immediate(arg)?;
    let delta = match ins.mnemonic.as_str() {
        "v_subrev_nc_u32" => -value,
        "v_add_nc_u32" => value,
        _ => return None,
    };
    if *base != Opnd::Vgpr(7) {
        // Some entry points preserve the incoming SP in a temporary before
        // assigning the adjusted SP. Only accept a copy with an uninterrupted
        // definition on every edge to the adjustment.
        if delta >= 0 || ins.dst != [Opnd::Vgpr(7)] {
            return None;
        }
        let copy = code[..index].iter().rev().find(|op| {
            !matches!(
                op.mnemonic.as_str(),
                "v_writelane_b32" | "buffer_load_format_x"
            ) || op.dst.contains(base)
                || op.dst.contains(&Opnd::Vgpr(7))
        })?;
        if copy.mnemonic != "v_mov_b32"
            || !whole_sdwa_words(copy)
            || copy.dst.as_slice() != [*base]
            || copy.src != [Opnd::Vgpr(7)]
            || code
                .iter()
                .filter_map(target)
                .any(|pc| (copy.offset + 1..=ins.offset).contains(&pc))
        {
            return None;
        }
    }
    Some(delta)
}

fn symbol_offsets(state: &State<'_>) -> Option<(Vec<usize>, usize)> {
    let count = word(state.symbols, 0x38)?;
    let table = state.symbols.get(0x40..)?;
    let records = table.get(..count.checked_mul(24)?)?;
    let n = count.checked_sub(2)?;
    if n == 0 {
        return None;
    }
    let mut starts = Vec::new();
    let mut scheduler = 0;
    for (i, rec) in records.as_chunks::<24>().0.iter().enumerate() {
        let name_offset = usize::try_from(u64::from_le_bytes(*rec.first_chunk()?)).ok()?;
        if name_offset < records.len() {
            return None;
        }
        let name = std::ffi::CStr::from_bytes_until_nul(table.get(name_offset..)?)
            .ok()?
            .to_str()
            .ok()?;
        let start = word(rec, 8)?;
        if !start.is_multiple_of(256) || start >= state.program.len() {
            return None;
        }
        if i < n {
            let expected = if let Some(any) = state.any_hit.as_ref() {
                // Paired material exports each have one leaf. The independent
                // any-hit entry follows the closest-hit entry in machine code.
                if n != 2 {
                    return None;
                }
                format!("{}.ss_0", if i == 0 { any } else { &state.name })
            } else {
                format!("{}.ss_{i}", state.name)
            };
            if name != expected || start == 0 || starts.last().is_some_and(|&prev| start >= prev) {
                return None;
            }
            starts.push(start);
        } else if i == n {
            if name != "XDXR_TraceRay.ss_0" || start != 0 {
                return None;
            }
        } else if name == "main" && start > starts[0] {
            scheduler = start;
        } else {
            return None;
        }
    }
    (*starts.last()? == 256).then_some((starts, scheduler))
}

// Some material leaves append literal tables between s_endpgm and the
// scheduler's s_code_end padding. Keep code, count, alignment and relocation
// checks separate from the table words, which are actual guest shader data.
fn constant_range(main: &[u8], scheduler: usize) -> Option<Range<usize>> {
    // The variant that sets FLAT_SCR_LO/HI has two extra code dwords.
    let header: usize = if fingerprint(main.get(..0x2a0)?, &[]) == 0x9c136eda913b7f18 {
        0x2a0
    } else if fingerprint(main.get(..0x2a8)?, &[]) == 0x0f96235ecaa00100 {
        0x2a8
    } else {
        return None;
    };
    let start = header + 8;
    let count = word(main, header + 4)?;
    let end = start.checked_add(count.checked_mul(4)?)?;
    let relocation = main.len().checked_sub(20)?;
    let padding = main.get(end.checked_add(4)?..relocation)?;
    if count == 0
        || main.len() % 4 != 0
        || word(main, header)? != 0xbf810000
        || word(main, end)? != 0xbf810000
        || !padding
            .chunks_exact(4)
            .all(|w| w == 0xbf9f0000u32.to_le_bytes())
        || word(main, relocation)? != ((scheduler + relocation - 20) << 7) | 1
        || main.get(relocation + 4..)?.iter().any(|&byte| byte != 0)
    {
        return None;
    }
    Some(scheduler + start..scheduler + end)
}

pub(in crate::ray) fn leaf_constants(state: &State<'_>) -> Option<Range<usize>> {
    let (_, scheduler) = symbol_offsets(state)?;
    constant_range(state.program.get(scheduler..)?, scheduler)
}

fn scheduler_matches(state: &State<'_>, scheduler: usize) -> bool {
    let program = state.program;
    let Some(main) = program.get(scheduler..) else {
        return false;
    };
    program
        .get(..256)
        .is_some_and(|stub| fingerprint(stub, &[]) == 0x347fdf3658d5b2da)
        && ((main.len() == 872
            && word(main, 0x354) == Some(((scheduler + 0x340) << 7) | 1)
            && match fingerprint(main, &[0x354..0x358]) {
                0x9dd40147fa5f4696 => true,
                // The leaf scheduler also has a form that sets FLAT_SCR_LO/HI
                // to 0:0x400. Its calls and resume tokens use the same ABI.
                0xced673c1494fc872 => state.kind == 2,
                _ => false,
            })
            || (state.kind == 2 && constant_range(main, scheduler).is_some()))
}

pub(in crate::ray) fn any_hit_range(state: &State<'_>) -> Option<Range<usize>> {
    state.any_hit.as_ref()?;
    let (starts, scheduler) = symbol_offsets(state)?;
    let [any, _closest] = starts.as_slice() else {
        return None;
    };
    scheduler_matches(state, scheduler).then_some(*any..scheduler)
}

pub(in crate::ray) fn leaf_span(state: &State<'_>) -> Result<Range<usize>, String> {
    let (starts, scheduler) = symbol_offsets(state).ok_or("invalid ray leaf symbol table")?;
    if !scheduler_matches(state, scheduler) {
        return Err("unsupported ray leaf scheduler".into());
    }
    let (start, end) = match (state.any_hit.as_ref(), starts.as_slice()) {
        (None, [start]) => (*start, scheduler),
        (Some(_), [any, closest]) => (*closest, *any),
        _ => return Err("unsupported ray leaf count".into()),
    };
    let mut returned = false;
    let mut last = None;
    for ins in instructions(&state.program[start..end]) {
        if !ins.is_complete()
            || ins.mnemonic == "s_swappc_b64"
            || (ins.mnemonic == "s_getpc_b64"
                && constant_range(&state.program[scheduler..], scheduler).is_none())
        {
            return Err("ray leaf has an unsupported call".into());
        }
        if ins.mnemonic == "s_setpc_b64" {
            if returned || ins.src != [Opnd::Sgpr(30)] {
                return Err("unsupported ray leaf return".into());
            }
            returned = true;
        }
        if !matches!(ins.mnemonic.as_str(), "s_nop" | "s_code_end") {
            last = Some(ins);
        }
    }
    let last = last
        .filter(|i| returned && matches!(i.mnemonic.as_str(), "s_setpc_b64" | "s_branch"))
        .ok_or("missing ray leaf return")?;
    Ok(start..start + last.offset + 4)
}

fn reads_only_incoming_return(code: &[Insn]) -> bool {
    // A standalone shader can finish without allocating a frame or calling
    // traversal. It loads only the caller's return token, at scratch[v7].
    let Some(address) = code.iter().position(|ins| {
        ins.mnemonic == "v_lshl_add_u32"
            && ins.dst == [Opnd::Vgpr(8)]
            && ins.src == [Opnd::Vgpr(7), Opnd::ConstInt(2), Opnd::Vgpr(26)]
    }) else {
        return false;
    };
    let Some((read, tail)) = code[address + 1..].split_first() else {
        return false;
    };
    if read.mnemonic != "buffer_load_dword"
        || read.dst != [Opnd::Vgpr(8)]
        || read.src != [Opnd::Vgpr(8), Opnd::Sgpr(0), Opnd::Sgpr(4)]
        || read.aux.mask != 1
        || read.aux.imm != 0
        || tail.last().is_none_or(|ins| ins.mnemonic != "s_setpc_b64")
    {
        return false;
    }
    code[..address].iter().all(|ins| {
        !(ins.mnemonic == "s_getpc_b64"
            || ins.mnemonic == "s_setpc_b64"
            || ins.src.contains(&Opnd::Vgpr(7))
            || ins.dst.contains(&Opnd::Vgpr(7))
            || ins.dst.contains(&Opnd::Vgpr(26))
            || (ins.enc == Enc::Mubuf && ins.src.get(1..) == Some(&[Opnd::Sgpr(0), Opnd::Sgpr(4)]))
            || target(ins).is_some_and(|pc| pc > code[address].offset))
    }) && tail[..tail.len() - 1].iter().all(|ins| {
        matches!(
            ins.mnemonic.as_str(),
            "s_waitcnt" | "v_readlane_b32" | "s_nop"
        )
    })
}

impl Layout {
    pub fn analyze(state: &State<'_>) -> Result<Self, String> {
        let (starts, scheduler) = symbol_offsets(state).ok_or("unsupported ray symbol table")?;
        if !scheduler_matches(state, scheduler) {
            return Err("unsupported ray scheduler".into());
        }
        Self::substates(state.program, &starts, scheduler)
    }

    fn substates(program: &[u8], starts: &[usize], scheduler: usize) -> Result<Self, String> {
        let mut spans = Vec::new();
        let mut next = scheduler;
        let mut frame = None;
        let mut pops = Vec::new();
        let mut frameless = false;
        let mut forward = true;
        for (index, &start) in starts.iter().enumerate() {
            let code: Vec<_> = instructions(&program[start..next]).collect();
            let last = code
                .iter()
                .rposition(|i| !matches!(i.mnemonic.as_str(), "s_nop" | "s_code_end"))
                .ok_or("empty ray substate")?;
            // The compiler may place a cold block after the common return
            // and branch back into the main body from its last instruction.
            let valid_end = match code[last].mnemonic.as_str() {
                "s_setpc_b64" => true,
                "s_branch" => target(&code[last]).is_some_and(|pc| {
                    code[..last]
                        .binary_search_by_key(&pc, |ins| ins.offset)
                        .is_ok()
                }),
                _ => false,
            };
            let returns = || {
                code[..=last]
                    .iter()
                    .filter(|ins| ins.mnemonic == "s_setpc_b64")
            };
            if !valid_end
                || returns().next().is_none()
                || returns().any(|ins| ins.src != [Opnd::Sgpr(30)])
                || code.iter().any(|i| !i.is_complete())
            {
                return Err("unsupported ray continuation return".into());
            }
            if starts.len() == 1 {
                frameless = reads_only_incoming_return(&code[..=last]);
            }
            for (pos, ins) in code[..=last].iter().enumerate() {
                if let Some(delta) = frame_adjustment(&code, pos) {
                    if delta < 0 && index == 0 {
                        if frame.is_some_and(|n| n != -delta) {
                            return Err("inconsistent ray stack frame".into());
                        }
                        frame = Some(-delta);
                    } else if delta > 0 && ins.dst == [Opnd::Vgpr(7)] {
                        pops.push(delta);
                    }
                }
                if ins.mnemonic != "s_getpc_b64" {
                    continue;
                }
                let [Opnd::Sgpr(lo)] = ins.dst.as_slice() else {
                    return Err("invalid ray return PC register".into());
                };
                let [add, carry] = code
                    .get(pos + 1..pos + 3)
                    .ok_or("truncated ray PC relocation")?
                else {
                    unreachable!()
                };
                let [base, delta] = add.src.as_slice() else {
                    return Err("invalid ray PC relocation".into());
                };
                let delta = immediate(delta).ok_or("dynamic ray PC relocation")?;
                let hi = Opnd::Sgpr(lo.checked_add(1).ok_or("invalid ray PC pair")?);
                if add.mnemonic != "s_add_u32"
                    || base != &ins.dst[0]
                    || add.dst != ins.dst
                    || carry.mnemonic != "s_addc_u32"
                    || carry.dst != [hi.clone()]
                    || carry.src.first() != Some(&hi)
                    || carry.src.get(1).and_then(immediate) != Some(if delta < 0 { -1 } else { 0 })
                {
                    return Err("unsupported ray PC relocation sequence".into());
                }
                let dest = (start + ins.offset + 4)
                    .checked_add_signed(delta as isize)
                    .ok_or("ray PC relocation overflow")?;
                let target = starts
                    .iter()
                    .position(|&pc| pc == dest)
                    .ok_or_else(|| format!("unknown ray continuation {dest:#x}"))?;
                forward &= target > index;
            }
            spans.push(start..start + code[last].offset + 4);
            next = start;
        }
        let frame = match frame {
            None if frameless && pops.is_empty() => 0,
            Some(n)
                if (1..4096).contains(&n) && !pops.is_empty() && pops.iter().all(|p| *p == n) =>
            {
                n as usize
            }
            _ => return Err("unsupported ray stack frame".into()),
        };
        Ok(Self {
            entries: spans,
            frame,
            forward,
        })
    }
}

#[cfg(test)]
#[path = "../../tests/ray/continuation/layout.rs"]
mod tests;
