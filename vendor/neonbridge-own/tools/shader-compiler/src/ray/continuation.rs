//! Lift the lane-local command stream. Xbox ray traversal calls a native
//! continuation and returns through a stack of 256-byte-aligned program PCs.
//! A host TraceRay preserves the same dwords across each suspension.
mod flow;
mod lanes;
mod layout;
mod leaf;
mod waterfall;
mod wave;
pub(super) use layout::{any_hit_range, leaf_constants, leaf_span};
pub(super) use leaf::{leaf_body, writes};
#[cfg(test)]
mod tests;
use super::{alu, operand, resources};
use crate::rdna2::{Enc, Insn, Opnd, instructions};
use std::collections::{BTreeMap, BTreeSet};
use std::fmt::Write;
use tuxbox_dxbc::ray::State;

// Guest continuations store GPU code addresses in 256-byte units. Keep their
// virtual base distinct from the scheduler's built-in traversal and exit tokens.
pub(in crate::ray) const CODE_BASE: u32 = 0x0100_0000;

fn pair(op: &Opnd) -> Result<[String; 2], String> {
    match op {
        Opnd::Sgpr(i) if *i < 127 => Ok([operand(op)?, operand(&Opnd::Sgpr(i + 1))?]),
        _ => Err("unsupported scalar register pair in ray shader".into()),
    }
}

fn scalar(ins: &Insn) -> Result<String, String> {
    let get = |ops: &[Opnd], i| {
        ops.get(i)
            .ok_or_else(|| format!("missing ray operand at {:#x}", ins.offset))
            .and_then(operand)
    };
    let dst = || get(&ins.dst, 0);
    let a = || get(&ins.src, 0);
    let b = || get(&ins.src, 1);
    let op = ins.mnemonic.as_str();
    Ok(match op {
        "s_mov_b32" if ins.dst == [Opnd::Null] => String::new(),
        "s_mov_b32" => format!("{} = {};", dst()?, a()?),
        "s_ff1_i32_b32" if ins.src == [Opnd::ExecLo] => {
            format!("{} = firstbitlow(tb_ray_ballot(exec != 0u));", dst()?)
        }
        "s_brev_b32" => format!("{} = reversebits({});", dst()?, a()?),
        "s_ashr_i32" => format!(
            "{} = asuint(asint({}) >> ({} & 31u)); scc = ({} != 0u);",
            dst()?,
            a()?,
            b()?,
            dst()?
        ),
        // MODE[3:2] selects FP16/FP64 rounding. Zero selects the native
        // HLSL round-to-nearest-even behavior. Other mode writes need emulation.
        "s_setreg_imm32_b32" if ins.aux.imm == 0x881 && ins.raw.get(1) == Some(&0) => String::new(),
        "s_cmp_eq_u32" | "s_cmp_lg_u32" => {
            let test = if op == "s_cmp_eq_u32" { "==" } else { "!=" };
            format!("scc = ({} {test} {});", a()?, b()?)
        }
        "s_cmp_ge_u32" => format!("scc = ({} >= {});", a()?, b()?),
        "s_mul_i32" => format!("{} = {} * {};", dst()?, a()?, b()?),
        "s_mul_hi_u32" => format!(
            "{} = uint((uint64_t({}) * uint64_t({})) >> 32u);",
            dst()?,
            a()?,
            b()?
        ),
        "s_add_i32" | "s_sub_i32" => {
            let (op, overflow) = if op == "s_add_i32" {
                ("+", "(x ^ z) & (y ^ z)")
            } else {
                ("-", "(x ^ y) & (x ^ z)")
            };
            format!(
                "{{ uint x={}, y={}, z=x {op} y; scc = (({overflow}) >> 31u); {}=z; }}",
                a()?,
                b()?,
                dst()?
            )
        }
        "s_lshl_b32" | "s_lshr_b32" => {
            let op = if op == "s_lshl_b32" { "<<" } else { ">>" };
            format!(
                "{} = {} {op} ({} & 31u); scc = ({} != 0u);",
                dst()?,
                a()?,
                b()?,
                dst()?
            )
        }
        "s_cselect_b32" => format!("{} = scc ? {} : {};", dst()?, a()?, b()?),
        "s_getpc_b64" => {
            let [lo, hi] = pair(&ins.dst[0])?;
            format!(
                "{lo} = {}u; {hi} = 0u;",
                CODE_BASE as usize + ins.offset + ins.dwords * 4
            )
        }
        "s_add_u32" | "s_addc_u32" => {
            let carry = if op == "s_addc_u32" { "scc" } else { "0u" };
            format!(
                "{{ uint a = {}, b = {}, c = {carry}, sum = a + b; scc = (sum < a || (sum + c) < sum); {} = sum + c; }}",
                a()?,
                b()?,
                dst()?
            )
        }
        "s_lshr_b64" | "s_lshl_b64" => {
            let shift = if op == "s_lshl_b64" {
                "tb_ray_shift_left"
            } else {
                "tb_ray_shift_right"
            };
            let [lo, hi] = pair(&ins.src[0])?;
            let [dlo, dhi] = pair(&ins.dst[0])?;
            format!(
                "{{ uint2 p = {shift}(uint2({lo}, {hi}), {}); {dlo} = p.x; {dhi} = p.y; scc = any(p != 0u); }}",
                b()?
            )
        }
        "s_bfe_u32" => format!(
            "{} = tb_ray_bfe({}, {}, (({} >> 16) & 127u)); scc = ({} != 0u);",
            dst()?,
            a()?,
            b()?,
            b()?,
            dst()?
        ),
        "s_and_saveexec_b32" => format!(
            "{{ uint saved = exec; exec &= {}; {} = saved; scc = (exec != 0u); }}",
            a()?,
            dst()?
        ),
        "s_and_b32" | "s_andn2_b32" | "s_or_b32" | "s_xor_b32" => {
            let operator = match op {
                "s_and_b32" => "&",
                "s_xor_b32" => "^",
                "s_andn2_b32" => "& ~",
                _ => "|",
            };
            format!(
                "{} = {} {operator} {}; scc = ({} != 0u);",
                dst()?,
                a()?,
                b()?,
                dst()?
            )
        }
        _ => {
            return Err(format!(
                "unsupported scalar ray instruction {op} at {:#x}",
                ins.offset
            ));
        }
    })
}

fn count_mask(block: &[Insn], index: usize) -> Result<String, String> {
    let ins = &block[index];
    let from_mask = match ins.src.as_slice() {
        [Opnd::VccLo | Opnd::ExecLo] => true,
        [Opnd::Sgpr(_)] => index.checked_sub(1).is_some_and(|prior| {
            let p = &block[prior];
            p.dst == ins.src
                && matches!(p.mnemonic.as_str(), "s_and_b32" | "s_xor_b32")
                && matches!(
                    p.src.as_slice(),
                    [Opnd::VccLo, Opnd::ExecLo] | [Opnd::ExecLo, Opnd::VccLo]
                )
        }),
        _ => false,
    };
    // We represent a guest EXEC/VCC bit per invocation. The native shader
    // counts a wave32 mask, then uses only SCC and overwrites the count.
    // Each ballot dword describes one guest wave32 on a wider host wave.
    if !from_mask
        || !block.get(index + 1).is_some_and(|next| {
            next.mnemonic == "s_cselect_b32"
                && next.dst == ins.dst
                && !next.src.iter().any(|operand| ins.dst.contains(operand))
        })
    {
        return Err(format!("unsupported ray wave count at {:#x}", ins.offset));
    }
    let source = operand(&ins.src[0])?;
    Ok(format!(
        "{{ uint4 votes = WaveActiveBallot({source} != 0u); scc = (votes[WaveGetLaneIndex() >> 5u] != 0u); }}"
    ))
}

pub(super) fn scratch(ins: &Insn) -> bool {
    ins.enc == Enc::Mubuf
        && matches!(
            (ins.aux.mask, ins.src.as_slice()),
            (1, [Opnd::Vgpr(_), Opnd::Sgpr(0), Opnd::Sgpr(4)])
                | (0, [Opnd::Vgpr(0), Opnd::Sgpr(0), Opnd::Sgpr(5)])
        )
}

fn stack(ins: &Insn, payload: Option<&leaf::Stack>) -> Result<String, String> {
    let byte = format!("({} + {}u)", operand(&ins.src[0])?, ins.aux.imm);
    let addr = if ins.aux.mask == 0 {
        if !matches!(
            ins.mnemonic.as_str(),
            "buffer_store_dword" | "buffer_load_dword"
        ) || !(0..4096).contains(&ins.aux.imm)
            || ins.aux.imm % 4 != 0
        {
            return Err(format!(
                "unsupported private ray scratch offset at {:#x}",
                ins.offset
            ));
        }
        format!("privateWord{}", ins.aux.imm / 4)
    } else if let Some(payload) = payload {
        let word = if ins.offset == payload.return_read {
            payload.words
        } else {
            ins.aux.imm as usize / 4
        };
        format!("spill[{word}]")
    } else {
        format!("spill[{byte} >> 2]")
    };
    let data = operand(&ins.dst[0])?;
    match ins.mnemonic.as_str() {
        "buffer_load_dword" => Ok(format!("{data} = {addr};")),
        "buffer_store_dword" => Ok(format!("{addr} = {data};")),
        "buffer_store_short" => Ok(format!(
            "{{ uint at = {byte}, i = at >> 2, shift = (at & 3u) * 8u, halfword = {data} & 0xffffu; \
             spill[i] = (spill[i] & ~(0xffffu << shift)) | (halfword << shift); \
             if (shift == 24u) spill[i + 1u] = (spill[i + 1u] & ~255u) | (halfword >> 8u); }}"
        )),
        _ => Err(format!(
            "unsupported ray scratch operation {}",
            ins.mnemonic
        )),
    }
}

pub(in crate::ray) fn target(ins: &Insn) -> Option<usize> {
    match ins.mnemonic.as_str() {
        "s_branch" | "s_cbranch_execz" | "s_cbranch_execnz" | "s_cbranch_scc0"
        | "s_cbranch_scc1" | "s_cbranch_vccz" | "s_cbranch_vccnz" => {
            (ins.offset + 4).checked_add_signed((ins.aux.imm * 4) as isize)
        }
        _ => None,
    }
}

// The native linker can put short branch islands after the export's final
// return. Rejoining their real target directly avoids an artificial back edge
// across the entire shader. Only inline islands with known, lane-local stores.
fn outlined_edges(code: &[Insn]) -> BTreeMap<usize, (usize, &[Insn])> {
    let mut islands = BTreeMap::new();
    let Some(ret) = code.iter().rposition(|i| i.mnemonic == "s_setpc_b64") else {
        return islands;
    };
    for entry in code[..ret]
        .iter()
        .filter_map(target)
        .filter(|&pc| pc > code[ret].offset)
    {
        if islands.contains_key(&entry) {
            continue;
        }
        let Ok(start) = code.binary_search_by_key(&entry, |i| i.offset) else {
            continue;
        };
        let tail = &code[start..];
        let len = tail
            .iter()
            .take_while(|i| {
                i.mnemonic == "s_nop"
                    || (i.mnemonic == "buffer_store_dword" && scratch(i) && i.aux.mask == 0)
            })
            .count();
        let Some(branch) = tail.get(len).filter(|i| i.mnemonic == "s_branch") else {
            continue;
        };
        if let Some(to) = target(branch).filter(|&pc| pc <= code[ret].offset) {
            islands.insert(entry, (to, &tail[..len]));
        }
    }
    islands
}

pub(super) fn candidate_body(
    state: &State<'_>,
    span: &std::ops::Range<usize>,
) -> Result<(String, String), String> {
    let bindings = resources::Bindings::candidate(state, span)?;
    let mut body = String::new();
    // The validated candidate body lies between the native register saves
    // and restores. Appending its normal return lets DXR own that call frame.
    let mut code = decode_range(state.program, span.start, span.end)?;
    // Some exports put a wait and unconditional branch after their return.
    // Only fold an edge when the entire island rejoins the validated body.
    if code.iter().filter_map(target).any(|dest| dest > span.end) {
        let leaf = any_hit_range(state).ok_or("missing candidate range")?;
        for ins in &mut code {
            let Some(dest) = target(ins).filter(|&dest| dest > span.end) else {
                continue;
            };
            let target = state
                .program
                .get(dest..leaf.end)
                .and_then(|tail| {
                    instructions(tail).find(|op| {
                        !matches!(
                            op.mnemonic.as_str(),
                            "s_nop" | "s_waitcnt" | "s_waitcnt_vscnt"
                        )
                    })
                })
                .filter(|op| op.mnemonic == "s_branch")
                .and_then(|mut op| {
                    op.offset += dest;
                    target(&op)
                })
                .filter(|entry| span.contains(entry))
                .ok_or("unsupported candidate branch island")?;
            ins.aux.imm = (target as i64 - ins.offset as i64 - 4) / 4;
        }
    }
    let mut ret = instructions(&0xbe80_201eu32.to_le_bytes()).next().unwrap();
    ret.offset = span.end;
    code.push(ret);
    waterfall::specialize(&mut code);
    continuation_code(
        code,
        span.start,
        span.end + 4,
        &mut body,
        &bindings,
        None,
        false,
    )?;
    Ok((bindings.declarations, body))
}

fn decode_range(isa: &[u8], start: usize, end: usize) -> Result<Vec<Insn>, String> {
    if start >= end {
        return Err("empty ray continuation".into());
    }
    instructions(isa.get(start..end).ok_or("invalid continuation range")?)
        .map(|mut ins| {
            ins.offset += start;
            ins.is_complete()
                .then_some(ins)
                .ok_or_else(|| "truncated ray instruction".to_owned())
        })
        .collect()
}

fn continuation(
    isa: &[u8],
    start: usize,
    end: usize,
    out: &mut String,
    bindings: &resources::Bindings,
    payload: Option<&leaf::Stack>,
    fold_blocks: bool,
) -> Result<(), String> {
    let code = decode_range(isa, start, end)?;
    continuation_code(code, start, end, out, bindings, payload, fold_blocks)
}

fn continuation_code(
    mut code: Vec<Insn>,
    start: usize,
    end: usize,
    out: &mut String,
    bindings: &resources::Bindings,
    payload: Option<&leaf::Stack>,
    fold_blocks: bool,
) -> Result<(), String> {
    if payload.is_some() {
        waterfall::specialize(&mut code);
    }
    let mut starts = BTreeSet::from([start, end]);
    for ins in &code {
        if let Some(dest) = target(ins) {
            if !(start..end).contains(&dest) {
                return Err(format!("branch outside ray substate at {:#x}", ins.offset));
            }
            starts.insert(dest);
            starts.insert(ins.offset + ins.dwords * 4);
        }
        if ins.mnemonic == "s_setpc_b64" {
            if ins.src != [Opnd::Sgpr(30)] {
                return Err("unsupported ray return target".into());
            }
            starts.insert(ins.offset + ins.dwords * 4);
        }
    }
    let pcs: Vec<_> = starts.into_iter().collect();
    let mut indices = Vec::with_capacity(pcs.len());
    for &pc in &pcs[..pcs.len() - 1] {
        indices.push(
            code.binary_search_by_key(&pc, |ins| ins.offset)
                .map_err(|_| format!("branch into ray instruction operand at {pc:#x}"))?,
        );
    }
    indices.push(code.len());
    let islands = outlined_edges(&code);
    let branch_target = |pc| islands.get(&pc).map_or(pc, |&(dest, _)| dest);
    let mut reachable = BTreeSet::new();
    let mut todo = vec![0];
    let mut loops = BTreeSet::new();
    while let Some(block) = todo.pop() {
        if !reachable.insert(block) {
            continue;
        }
        let last = &code[indices[block + 1] - 1];
        if let Some(dest) = target(last) {
            let next = pcs.binary_search(&branch_target(dest)).unwrap();
            if next <= block {
                loops.insert((next, block));
            }
            todo.push(next);
        }
        if !matches!(last.mnemonic.as_str(), "s_branch" | "s_setpc_b64") {
            if pcs[block + 1] == end {
                return Err("ray substate falls through return".into());
            }
            todo.push(block + 1);
        }
    }
    // A back edge whose interval contains another back edge forms nested
    // loops. A preheader may branch into the body for its first iteration.
    // Preserve that exact entry PC. Overlapping, non-nested intervals still
    // need the general block dispatcher.
    let dispatch = loops
        .iter()
        .any(|&(a, b)| loops.iter().any(|&(c, d)| a < c && c <= b && b < d));
    writeln!(out, "{{ uint nextBlock = {start}u;").unwrap();
    lanes::declarations(&code, out)?;
    let saved_masks = wave::saved_exec_counts(&code, out)?;
    let mut private_words = BTreeSet::new();
    let mut private_base = false;
    let mut branched = false;
    for ins in &code {
        if ins.dst == [Opnd::Sgpr(5)] {
            private_base = !branched && ins.mnemonic == "s_mov_b32" && ins.src == [Opnd::Sgpr(32)];
        }
        branched |= target(ins).is_some();
        if scratch(ins) && ins.aux.mask == 0 {
            if !private_base {
                return Err(format!(
                    "unknown private ray scratch base at {:#x}",
                    ins.offset
                ));
            }
            private_words.insert(ins.aux.imm / 4);
        }
    }
    for word in private_words {
        writeln!(out, "uint privateWord{word} = 0u;").unwrap();
    }
    let mut graph = BTreeMap::new();
    if dispatch && !fold_blocks {
        writeln!(
            out,
            "[loop] while (nextBlock != {end}u) {{ switch (nextBlock) {{"
        )
        .unwrap();
    }
    for block in reachable {
        let pc = pcs[block];
        let following = pcs[block + 1];
        let mut block_source = String::new();
        let out = if fold_blocks {
            &mut block_source
        } else {
            &mut *out
        };
        let mut edge = flow::Edge::Jump(following);
        if !fold_blocks && dispatch {
            writeln!(out, "case {pc}u: {{ nextBlock = {following}u;").unwrap();
        } else if !fold_blocks {
            for &(lo, hi) in loops.iter().rev() {
                if lo == block {
                    writeln!(out, "if (nextBlock <= {}u) {{ [loop] do {{", pcs[hi]).unwrap();
                }
            }
            writeln!(out, "if (nextBlock <= {pc}u) {{ nextBlock = {following}u;").unwrap();
        }
        let mut guarded = false;
        let body = &code[indices[block]..indices[block + 1]];
        for (index, ins) in body.iter().enumerate() {
            let op = ins.mnemonic.as_str();
            if matches!(op, "v_nop" | "s_nop" | "s_waitcnt" | "s_waitcnt_vscnt") {
                continue;
            }
            let (statement, active) = if op == "s_setpc_b64" {
                edge = flow::Edge::Jump(end);
                (
                    if fold_blocks {
                        String::new()
                    } else {
                        format!("nextBlock = {end}u;")
                    },
                    false,
                )
            } else if let Some(dest) = target(ins) {
                let cond = match op {
                    "s_cbranch_execz" => Some("exec == 0u"),
                    "s_cbranch_execnz" => Some("exec != 0u"),
                    "s_cbranch_vccz" => Some("vcc == 0u"),
                    "s_cbranch_vccnz" => Some("vcc != 0u"),
                    "s_cbranch_scc1" => Some("scc != 0u"),
                    "s_cbranch_scc0" => Some("scc == 0u"),
                    "s_branch" => None,
                    _ => return Err("unknown ray branch condition".into()),
                };
                let to = branch_target(dest);
                let mut stores = String::new();
                if let Some((_, island)) = islands.get(&dest) {
                    for store in island.iter().filter(|i| i.mnemonic != "s_nop") {
                        writeln!(stores, "{}", stack(store, payload)?).unwrap();
                    }
                }
                edge = match cond {
                    Some(condition) => flow::Edge::Branch {
                        condition,
                        taken: to,
                        fall: following,
                    },
                    None => flow::Edge::Jump(to),
                };
                let cond = cond.map_or_else(String::new, |c| format!("if ({c}) "));
                if fold_blocks {
                    let text = if stores.is_empty() {
                        String::new()
                    } else {
                        format!("{cond}{{ if (exec != 0u) {{ {stores} }} }}")
                    };
                    (text, false)
                } else if stores.is_empty() {
                    (format!("{cond}nextBlock = {to}u;"), false)
                } else {
                    (
                        format!("{cond}{{ if (exec != 0u) {{ {stores} }} nextBlock = {to}u; }}"),
                        false,
                    )
                }
            } else if let Some(statement) = saved_masks.get(&ins.offset) {
                (statement.clone(), false)
            } else if let Some(spill) = lanes::emit(ins)? {
                (spill, false)
            } else if op == "s_bcnt1_i32_b32" {
                (count_mask(body, index)?, false)
            } else {
                match ins.enc {
                    Enc::Smem => (bindings.memory(ins)?, false),
                    Enc::Mimg | Enc::Flat => (bindings.memory(ins)?, true),
                    Enc::Mubuf if scratch(ins) => (stack(ins, payload)?, true),
                    Enc::Mubuf => (bindings.memory(ins)?, true),
                    Enc::Vop1 | Enc::Vop2 | Enc::Vop3 | Enc::Vopc => {
                        let writes_mask = op.starts_with("v_cmp")
                            || matches!(op, "v_add_co_u32" | "v_add_co_ci_u32");
                        (alu::vector(ins)?, !writes_mask)
                    }
                    _ => (scalar(ins)?, false),
                }
            };
            // Consecutive per-lane operations see the same EXEC mask. One
            // condition per run avoids thousands of redundant DXIL branches.
            if guarded != active {
                out.push_str(if active { "if (exec != 0u) {\n" } else { "}\n" });
                guarded = active;
            }
            writeln!(out, "{statement}").unwrap();
            if guarded && ins.dst.contains(&Opnd::ExecLo) {
                out.push_str("}\n");
                guarded = false;
            }
        }
        if guarded {
            out.push_str("}\n");
        }
        if fold_blocks {
            graph.insert(
                pc,
                flow::Block {
                    body: block_source,
                    exit: edge,
                },
            );
            continue;
        }
        if dispatch {
            out.push_str("break;\n");
        }
        out.push_str("}\n");
        if !dispatch {
            for &(lo, hi) in loops.iter().rev() {
                if hi == block {
                    writeln!(
                        out,
                        "}} while (nextBlock >= {}u && nextBlock <= {pc}u); }}",
                        pcs[lo]
                    )
                    .unwrap();
                }
            }
        }
    }
    if fold_blocks {
        flow::emit(graph, start, end, out);
    } else if dispatch {
        out.push_str("default: return; } }\n");
    }
    out.push_str("}\n");
    Ok(())
}

// Tracing transfers v6, v17..v25 and a stack-relative payload to the traversal
// program. Its return stub pops the continuation token stored at stack[v7].
const TRACE: &str = r#"
if (v[8] == 42u) {
    RayDesc ray = { asfloat(uint3(v[19], v[20], v[21])), asfloat(v[22]),
                    asfloat(uint3(v[23], v[24], v[25])), asfloat(v[6]) };
    uint copyWords = tb_payload_words(v[9] - v[7]);
    NativePayload payload = (NativePayload)0;
    [unroll] for (uint i=0; i<payloadWords; ++i) if (i < copyWords) payload.words[i] = spill[v[9] + i];
    TraceRay(scene, v[17], v[18] & 255u, (v[18] >> 8) & 15u, (v[18] >> 12) & 15u,
             v[18] >> 16, ray, payload);
    [unroll] for (uint i=0; i<payloadWords; ++i) if (i < copyWords) spill[v[9] + i] = payload.words[i];
    v[8] = spill[v[7]];
    v[26] = 0u;
    exec = ~0u;
}
"#;

pub(super) enum Payload {
    Fixed(usize),
    Reflection,
}

pub(super) fn ray_generation(state: &State<'_>, payload: Payload) -> Result<String, String> {
    let layout = layout::Layout::analyze(state)?;
    let bindings = resources::Bindings::analyze(state, &layout.entries)?;
    let mut out = bindings.declarations.clone();
    out.push_str(alu::HLSL);
    let (payload_words, copy_words) = match payload {
        Payload::Fixed(words) => (words, "payloadWords"),
        // A reflection shader also issues occlusion rays. The 6-word payload
        // starts at word 5 and the 16-word payload starts at word 11.
        Payload::Reflection => (16, "(offset == 5u ? 6u : 16u)"),
    };
    writeln!(
        out,
        "static const uint payloadWords = {payload_words}u;\nuint tb_payload_words(uint offset) {{ return {copy_words}; }}\nstruct NativePayload {{uint words[{payload_words}];}};\n[shader(\"raygeneration\")]\nvoid {}() {{",
        state.name
    )
    .unwrap();
    writeln!(out, "uint spill[{0}] = (uint[{0}])0;", layout.frame + 1).unwrap();
    let vgprs = super::vgpr_count(state);
    write!(
        out,
        "\nuint v[{vgprs}] = (uint[{vgprs}])0, s[128] = (uint[128])0;"
    )
    .unwrap();
    out.push_str(
        r#"
uint exec = ~0u, scc = 0u, vcc = 0u, m0 = 0u;
uint bitsX = uint(firstbithigh(max(DispatchRaysDimensions().x, 2u) - 1u)) + 1u;
uint2 xy = DispatchRaysIndex().xy;
v[1] = xy.x | (xy.y << bitsX);
s[13] = (bitsX << 24) | ((32u - bitsX) << 16);
"#,
    );
    let token = |pc: usize| (CODE_BASE as usize + pc) >> 8;
    writeln!(
        out,
        "v[7] = {}u; v[8] = {}u;",
        layout.frame,
        token(layout.entries[0].start)
    )
    .unwrap();
    if !layout.forward {
        out.push_str("[loop] while (v[8] != 0u) { switch (v[8]) {\n");
    }
    for span in layout.entries {
        if layout.forward {
            writeln!(out, "if (v[8] == {}u) {{", token(span.start)).unwrap();
        } else {
            writeln!(out, "case {}u: {{", token(span.start)).unwrap();
        }
        // The scheduler reselects the lanes with this PC and restores the scratch
        // descriptor and lane-local base on every call, including direct resumes.
        out.push_str("exec = ~0u; scc = 1u; v[26] = 0u;\n");
        continuation(
            state.program,
            span.start,
            span.end,
            &mut out,
            &bindings,
            None,
            true,
        )?;
        if layout.forward && layout.frame != 0 {
            out.push_str(TRACE);
        }
        if !layout.forward {
            out.push_str("break;\n");
        }
        out.push_str("}\n");
    }
    if !layout.forward {
        out.push_str("default: return; }\n");
        if layout.frame != 0 {
            out.push_str(TRACE);
        }
        out.push_str("}\n");
    }
    out.push_str("}\n");
    Ok(out)
}
