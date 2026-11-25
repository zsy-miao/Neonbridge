//! Prove native wave participation through structured, bounded compute loops.
use super::{ray_resolve, wave_sum, Emitter, Enc, Insn, Opnd};
use std::collections::{BTreeSet, VecDeque};
use std::ops::RangeInclusive;

mod flow;
mod scalar;

use flow::Flow;
use scalar::Effect;

fn scalar(operand: Opnd) -> Option<u8> {
    match operand {
        Opnd::Sgpr(reg @ 0..=105) => Some(reg),
        _ => None,
    }
}

fn literal(operand: Opnd) -> Option<u32> {
    match operand {
        Opnd::ConstInt(value) => Some(value as u32),
        Opnd::Literal(value) => Some(value),
        _ => None,
    }
}

fn first_lane(op: &Insn) -> Option<(u8, u8)> {
    if op.mnemonic != "v_readfirstlane_b32" {
        return None;
    }
    let [Opnd::Vgpr(dst @ 0..=105) | Opnd::Sgpr(dst @ 0..=105)] = op.dst[..] else {
        return None;
    };
    let [Opnd::Vgpr(src)] = op.src[..] else {
        return None;
    };
    Some((dst, src))
}

fn writes_exec(op: &Insn) -> bool {
    op.dst
        .iter()
        .any(|o| matches!(o, Opnd::ExecLo | Opnd::ExecHi))
        || op.mnemonic.contains("saveexec")
        || op.mnemonic.contains("wrexec")
        || op.mnemonic.starts_with("v_cmpx")
}

#[derive(Clone, Copy, PartialEq, Eq)]
struct Exec {
    nonempty: bool,
    saved: u128,
}

impl Exec {
    fn transfer(self, op: &Insn, effect: &Effect) -> Self {
        let m = op.mnemonic.as_str();
        let mut out = Self {
            nonempty: self.nonempty,
            saved: self.saved & !effect.scalar_writes,
        };
        let save = m.ends_with("_saveexec_b32") || m == "s_mov_b32" && op.src == [Opnd::ExecLo];
        if self.nonempty && save {
            if let [Opnd::Sgpr(reg @ 0..=105)] = op.dst[..] {
                out.saved |= 1 << reg;
            }
        }
        if writes_exec(op) {
            let src = op.src.first().copied();
            let nonzero = src.and_then(literal).is_some_and(|v| v != 0);
            out.nonempty = m == "s_or_saveexec_b32" && (self.nonempty || nonzero)
                || m == "s_mov_b32"
                    && op.dst == [Opnd::ExecLo]
                    && match src {
                        Some(Opnd::Sgpr(reg @ 0..=105)) => self.saved & (1 << reg) != 0,
                        Some(Opnd::ExecLo) => self.nonempty,
                        _ => nonzero,
                    };
        }
        out
    }
}

fn nonempty_exec(code: &[Insn], flow: &Flow, effects: &[Effect], end: usize) -> Vec<Option<Exec>> {
    let mut incoming = vec![None::<Exec>; end + 1];
    incoming[0] = Some(Exec {
        nonempty: true,
        saved: 0,
    });
    let mut pending = VecDeque::from([0]);
    while let Some(i) = pending.pop_front() {
        let op = &code[i];
        let out = incoming[i].unwrap().transfer(op, &effects[i]);
        for (edge, to) in flow.edges[i].into_iter().enumerate() {
            let Some(to) = to.filter(|&to| to <= end) else {
                continue;
            };
            let mut next = out;
            if op.enc == Enc::Sopp && matches!((op.op, edge), (8, 1) | (9, 0)) {
                next.nonempty = true;
            }
            if let Some(prev) = incoming[to] {
                next.nonempty &= prev.nonempty;
                next.saved &= prev.saved;
            }
            if incoming[to] != Some(next) {
                incoming[to] = Some(next);
                pending.push_back(to);
            }
        }
    }
    incoming
}

fn bounded_counter(
    code: &[Insn],
    flow: &Flow,
    effects: &[Effect],
    head: usize,
    latch: usize,
) -> bool {
    let Some(bump) = latch.checked_sub(2).filter(|&bump| bump >= head) else {
        return false;
    };
    let (inc, cmp) = (&code[bump], &code[bump + 1]);
    let [counter @ Opnd::Sgpr(reg @ 0..=105), limit] = cmp.src[..] else {
        return false;
    };
    if code[latch].op != 4
        || cmp.mnemonic != "s_cmp_eq_u32"
        || !literal(limit).is_some_and(|limit| (1..=32).contains(&limit))
        || inc.mnemonic != "s_add_i32"
        || inc.dst != [counter]
        || inc.src != [counter, Opnd::ConstInt(1)]
        || flow.incoming[latch] != [latch - 1]
        || flow.incoming[latch - 1] != [bump]
        || (head..latch).any(|i| i != bump && effects[i].writes(reg))
    {
        return false;
    }
    let inits = flow.definitions(head, 0, Some((latch, head)), |i| effects[i].writes(reg));
    inits.is_some_and(|inits| {
        inits.into_iter().all(|i| {
            code[i].mnemonic == "s_mov_b32"
                && code[i].dst == [counter]
                && code[i].src.first().copied().and_then(literal) == Some(0)
        })
    })
}

fn writes_bitmap(op: &Insn, bitmap: u8) -> bool {
    // The single-word forms commonly used between iterations have exact
    // destinations. Treat other vector writes as possible overlapping writes.
    if op.mnemonic.contains("movrel")
        || op.mnemonic == "v_swap_b32" && op.src.contains(&Opnd::Vgpr(bitmap))
    {
        return true;
    }
    if first_lane(op).is_some() {
        return false;
    }
    let single = matches!(
        op.mnemonic.as_str(),
        "v_mov_b32"
            | "v_add_nc_u32"
            | "v_sub_nc_u32"
            | "v_and_b32"
            | "v_or_b32"
            | "v_xor_b32"
            | "v_lshlrev_b32"
            | "v_lshrrev_b32"
            | "v_mul_f32"
            | "v_mul_legacy_f32"
            | "v_fma_f32"
            | "v_fma_mix_f32"
            | "v_max_f32"
            | "v_cvt_f32_ubyte0"
            | "v_cvt_f32_f16"
    );
    op.dst
        .iter()
        .any(|dst| matches!(*dst, Opnd::Vgpr(reg) if reg == bitmap || !single && reg < bitmap))
}

fn bounded_bitmap(
    code: &[Insn],
    flow: &Flow,
    effects: &[Effect],
    head: usize,
    latch: usize,
) -> Option<()> {
    let (read, bitmap) = first_lane(&code[head])?;
    if code[head..latch].iter().any(writes_exec)
        || flow.loops.iter().any(|&(h, l)| head < h && l < latch)
    {
        return None;
    }
    let definition = |at, reg| flow.definition(at, head, |i| effects[i].writes(reg));
    let compare = flow.definition(latch, head, |i| effects[i].writes_scc)?;
    let cmp = &code[compare];
    if !matches!(
        (cmp.mnemonic.as_str(), code[latch].op),
        ("s_cmp_eq_u32", 4) | ("s_cmp_lg_u32", 5)
    ) {
        return None;
    }
    let [left, right] = cmp.src[..] else {
        return None;
    };
    let (value, bit) = if scalar(left) == Some(read) {
        (left, right)
    } else {
        (right, left)
    };
    if scalar(value) != Some(read) || definition(compare, read)? != head {
        return None;
    }
    let shift = definition(compare, scalar(bit)?)?;
    let shl = &code[shift];
    let [Opnd::ConstInt(1), count] = shl.src[..] else {
        return None;
    };
    if shl.mnemonic != "s_lshl_b32" || shl.dst != [bit] {
        return None;
    }
    let mask = definition(shift, scalar(count)?)?;
    let [mut index, mut mask_value] = code[mask].src[..] else {
        return None;
    };
    if literal(index) == Some(31) {
        std::mem::swap(&mut index, &mut mask_value);
    }
    let find = definition(mask, scalar(index)?)?;
    if code[mask].mnemonic != "s_and_b32"
        || literal(mask_value) != Some(31)
        || code[find].mnemonic != "s_ff1_i32_b32"
        || code[find].src != [value]
        || definition(find, read)? != head
    {
        return None;
    }
    let copy = flow.definition(latch, head, |i| writes_bitmap(&code[i], bitmap))?;
    let mov = &code[copy];
    let [updated] = mov.src[..] else {
        return None;
    };
    let xor = definition(copy, scalar(updated)?)?;
    // The next iteration must read all 32 bits of the XOR result.
    if mov.enc != Enc::Vop1
        || mov.mnemonic != "v_mov_b32"
        || mov.aux.sdwa.is_some()
        || mov.dst != [Opnd::Vgpr(bitmap)]
        || code[xor].mnemonic != "s_xor_b32"
        || (code[xor].src != [bit, value] && code[xor].src != [value, bit])
        || definition(xor, read)? != head
        || definition(xor, scalar(bit)?)? != shift
    {
        return None;
    }
    // The same guest lane is elected on every trip. A nonzero bitmap loses its
    // lowest set bit until the singleton comparison exits. Zero first becomes
    // a bit-31 singleton, so even that case exits on the second trip.
    Some(())
}

fn region_is_intact(flow: &Flow, region: &RangeInclusive<usize>) -> bool {
    (*region.start() + 1..=*region.end()).all(|i| flow.incoming[i] == [i - 1])
        && (*region.start()..*region.end()).all(|i| flow.edges[i] == [None, Some(i + 1)])
}

/// Whether the emitter keeps whole lane masks and nothing outside the scalar
/// instructions can change them.
fn full_mask_program(em: &Emitter<'_>) -> bool {
    em.resolve_wave32
        && em.packed.is_none()
        && !em.wave_cull
        && !em.page_waves
        && !em.compute_swizzle
        && !em.indexed_lane
        && em.vote_uses.is_empty()
        && !em.insns.iter().any(|op| {
            (op.mnemonic.starts_with("tb_") && op.mnemonic != ray_resolve::READ_LANE)
                || op.enc == Enc::Ds
                || matches!(
                    op.mnemonic.as_str(),
                    "s_barrier" | "s_sendmsg" | "s_sendmsghalt"
                )
        })
}

/// Interleaved loops go through the block dispatcher, where no loop needs a
/// bound of its own: it is enough that every scalar write keeps the entry
/// scalars wave-uniform, so each branch is a decision of the whole wave and the
/// lanes follow the same blocks.
pub(super) fn uniform_dispatch(em: &Emitter<'_>) -> bool {
    let code = &em.insns;
    if !full_mask_program(em) {
        return false;
    }
    let Some(flow) = Flow::graph(code) else { return false };
    if flow.structured() || wave_sum::reduction_regions(code, 32).any(|(_, region)| !region_is_intact(&flow, &region)) {
        return false;
    }
    let effects: Vec<_> = code.iter().map(Effect::new).collect();
    (0..code.len()).all(|i| !flow.reachable[i] || scalar::scalar_step(em, &flow, &effects, i))
}

/// In this emitter mode the entry scalars already have wave-uniform values.
/// Every reachable scalar write must preserve that invariant. Scalar branch
/// conditions then preserve host participation, including in earlier loops
/// that retain the emitter's watchdog.
pub(super) fn bounded_latches(em: &Emitter<'_>) -> Option<BTreeSet<usize>> {
    let code = &em.insns;
    if !full_mask_program(em) {
        return None;
    }
    let flow = Flow::new(code)?;
    let regions: Vec<_> = wave_sum::reduction_regions(code, 32)
        .map(|(_, region)| region)
        .collect();
    let mut end = *regions.last()?.end();
    if regions.iter().any(|r| !region_is_intact(&flow, r)) {
        return None;
    }
    if let Some(last) = code.iter().rposition(|op| {
        matches!(
            op.mnemonic.as_str(),
            "v_readfirstlane_b32" | ray_resolve::READ_LANE | "v_permlanex16_b32"
        ) || op.mnemonic.starts_with("v_cmp")
            || op.mnemonic.contains("_co_")
    }) {
        end = end.max(last);
    }
    for &(head, latch) in &flow.loops {
        if head <= end {
            end = end.max(latch);
        }
    }
    let effects: Vec<_> = code.iter().map(Effect::new).collect();
    if (0..=end).any(|i| flow.reachable[i] && !scalar::scalar_step(em, &flow, &effects, i)) {
        return None;
    }
    let exec = nonempty_exec(code, &flow, &effects, end);
    let mut bounded = BTreeSet::new();
    for &(head, latch) in flow.loops.iter().filter(|&&(head, _)| head <= end) {
        let reduction = regions.iter().any(|r| (head..=latch).contains(r.start()));
        let bitmap = matches!(code[latch].op, 4 | 5) && first_lane(&code[head]).is_some();
        if !reduction && !bitmap {
            continue;
        }
        if (head + 1..=latch).any(|i| {
            flow.incoming[i]
                .iter()
                .any(|&from| !(head..=latch).contains(&from))
        }) || !(bounded_counter(code, &flow, &effects, head, latch)
            || exec[head].is_some_and(|mask| mask.nonempty)
                && bounded_bitmap(code, &flow, &effects, head, latch).is_some())
        {
            return None;
        }
        bounded.insert(latch);
    }
    (!bounded.is_empty()).then_some(bounded)
}

#[cfg(test)]
#[path = "../tests/hlsl/wave_loops.rs"]
mod tests;
