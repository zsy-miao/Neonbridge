//! Virtual shadow page shaders maintain independent 64-lane masks inside each
//! 256-thread group. Their elected atomics target several different buffers and
//! can subtract as well as add. Keep these memory operations in native order.
use super::{compute_group, packed_root::matches_program, r, Emitter, Insn, Opnd, ShaderExtract, Stage};

pub(super) fn recognize(ex: &ShaderExtract) -> bool {
    ex.stage == Stage::Compute && compute_group(ex) == [256, 1, 1]
        && [
            (488, 0x37edf39a5cfc0ea0), (568, 0x7c25b388d8bc92d2),
            (384, 0xc647b3791d6798f7), (428, 0x03e1a9c249a48aea),
            (1024, 0x3015c824d44da81d), (308, 0xd2f65235710d5348),
            (500, 0x2b0fe09ed7b1f4c2), (624, 0x88edbe82eaf6c826),
        ].into_iter().any(|(n, hash)| matches_program(&ex.isa, n, hash))
}

// The generic register representation stores each lane's bit of EXEC or an
// SGPR mask in the low bit of the pair's first register.
fn ballot_input(ins: &Insn) -> Option<Opnd> {
    match ins.mnemonic.as_str() {
        "s_cbranch_execz" => Some(Opnd::ExecLo),
        "s_ff1_i32_b64" | "s_bcnt1_i32_b64" => Some(ins.src[0]),
        "v_mbcnt_lo_u32_b32" if !matches!(ins.src[0], Opnd::ConstInt(-1)) => Some(ins.src[0]),
        "v_mbcnt_hi_u32_b32" => match ins.src[0] {
            Opnd::ExecHi => Some(Opnd::ExecLo),
            Opnd::Sgpr(n) if n != 0 => Some(Opnd::Sgpr(n - 1)),
            _ => None,
        },
        _ => None,
    }
}

// Every scalar branch in these straight-line programs is wave-uniform, and
// every branch target is forward. Emit group rendezvous outside ALL guest
// branch guards. A skipped guest wave only writes its own scratch partition.
pub(super) fn rendezvous(idx: usize, ins: &Insn) -> Result<Option<String>, String> {
    if let Some(src) = ballot_input(ins) {
        return Ok(Some(format!("uint2 page_mask{idx} = tb_page_ballot(tb_wave_tid, ({} & 1u) != 0u);", r(&src)?)));
    }
    if ins.mnemonic == "v_readlane_b32" {
        return Ok(Some(format!("uint page_value{idx} = tb_page_read(tb_wave_tid, {}, {});", r(&ins.src[0])?, r(&ins.src[1])?)));
    }
    Ok(None)
}

pub(super) fn emit(em: &mut Emitter<'_>, i: usize, ins: &Insn, target: Option<usize>) -> Result<bool, String> {
    let mask = format!("page_mask{i}");
    match ins.mnemonic.as_str() {
        "s_cbranch_execz" => {
            let target = target.ok_or("backward page wave branch")?;
            em.line(&format!("if (({mask}.x | {mask}.y) == 0u) j{target} = true;"));
        }
        "s_ff1_i32_b64" => {
            em.ws(&ins.dst[0], &format!("({mask}.x != 0u ? uint(firstbitlow({mask}.x)) : {mask}.y != 0u ? 32u + uint(firstbitlow({mask}.y)) : 0xffffffffu)"))?;
        }
        "s_bcnt1_i32_b64" => {
            em.ws(&ins.dst[0], &format!("(countbits({mask}.x) + countbits({mask}.y))"))?;
            em.line(&format!("scc = ({} != 0u);", r(&ins.dst[0])?));
        }
        "v_readlane_b32" => {
            let Opnd::Vgpr(s) = ins.dst[0] else { return Err("invalid scalar destination in page lane read".into()); };
            em.ws(&Opnd::Sgpr(s), &format!("page_value{i}"))?;
        }
        "v_mbcnt_lo_u32_b32" | "v_mbcnt_hi_u32_b32" => {
            let high = ins.mnemonic == "v_mbcnt_hi_u32_b32";
            let bits = if high { "max(32u, tb_wave_tid & 63u) - 32u" } else { "min(32u, tb_wave_tid & 63u)" };
            let prefix = if ins.src[0] == Opnd::ConstInt(-1) {
                bits.to_string()
            } else {
                let word = if high { "y" } else { "x" };
                format!("countbits({mask}.{word} & {})", super::low_mask(bits))
            };
            em.wv(&ins.dst[0], &format!("({} + ({prefix}))", r(&ins.src[1])?))?;
        }
        _ => return Ok(false),
    }
    Ok(true)
}

pub(super) fn declarations(group: [u32; 3], branchless: bool) -> String {
    let (words, lanes, clear, vote) = if branchless {
        // Each of the 64 lanes clears a distinct word. Zero votes contribute
        // zero to the OR. Omitting both branches per vote keeps the large
        // field interpreter within the pipeline compiler's 1 MiB guest stack.
        (64, 256, "page_bits[tid] = 0u;",
            "InterlockedOr(page_bits[tid >> 5], uint(vote) << (tid & 31u));")
    } else {
        let vote = "if (vote) InterlockedOr(page_bits[tid >> 5], 1u << (tid & 31u));";
        if group == [1024, 1, 1] {
            (32, 1024, "if (tid < 32u) page_bits[tid] = 0u;", vote)
        } else {
            (8, 256, "if (tid < 8u) page_bits[tid] = 0u;", vote)
        }
    };
    format!(r#"
groupshared uint page_bits[{words}];
groupshared uint page_lane_values[{lanes}];

uint2 tb_page_ballot(uint tid, bool vote) {{
    {clear}
    GroupMemoryBarrierWithGroupSync();
    {vote}
    GroupMemoryBarrierWithGroupSync();
    uint halfWave = (tid >> 6) * 2u;
    uint2 bits = uint2(page_bits[halfWave], page_bits[halfWave + 1u]);
    GroupMemoryBarrierWithGroupSync();
    return bits;
}}

uint tb_page_read(uint tid, uint value, uint lane) {{
    page_lane_values[tid] = value;
    GroupMemoryBarrierWithGroupSync();
    uint result = page_lane_values[(tid & ~63u) | (lane & 63u)];
    GroupMemoryBarrierWithGroupSync();
    return result;
}}
"#)
}
