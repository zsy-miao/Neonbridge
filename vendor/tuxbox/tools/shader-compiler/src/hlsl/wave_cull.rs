//! Reconstruct guest wave64 votes, lane reads and reductions on wave32 hosts.
//! In a single-wave group, the guest PC and scalar branches are group-uniform.
use super::{compute_group, packed_root::matches_program, r, Emitter, Insn, Opnd, ShaderExtract, Stage};

const SCALAR_FIELD_INTERPRETER: (usize, u64) = (0x7600, 0xe9bc16e878389630);

pub(super) fn needs_branchless_ballots(ex: &ShaderExtract) -> bool {
    let (size, hash) = SCALAR_FIELD_INTERPRETER;
    matches_program(&ex.isa, size, hash)
}

pub(super) fn recognize(ex: &ShaderExtract) -> bool {
    if ex.stage != Stage::Compute { return false; }
    let signatures: &[(usize, u64)] = match (ex.entry.as_deref(), compute_group(ex)) {
        (Some("CullPerPageDrawCommandsCs"), [64, 1, 1]) => &[
            (8052, 0x5d796603fd1dea38), (10308, 0xce78579f2ce98f18),
            (8428, 0x8cd856ebca820348), (7256, 0x3a8e6c2104cd9816),
            (9100, 0x4e7df4739f9901e8), (9272, 0x8967fb838ed514f7),
            (7552, 0x1b8e0550f5ad2b73), (6392, 0xdf6e5025d7df4d45),
        ],
        (Some("VirtualSmInvalidateInstancePagesCS"), [64, 1, 1]) => &[
            (4604, 0x3a8e759a33ecc608), (5868, 0xcf8067fde3c91638),
            (5156, 0x1578d47cb07c2498), (8576, 0x73df372fcc20b792),
            (7856, 0xd091795480830606),
        ],
        (Some("MainCS"), [16, 16, 1]) =>
            &[(1928, 0xf08580324c59c199), (2464, 0xed9544d539da1bfe), (4564, 0x7793b99bec855677)],
        (Some("MainCS"), [8, 8, 1]) => &[
            (2356, 0xe4995b0e2291e61e), (2884, 0x1c9821104747da89),
            (0xb60, 0x12a42452f6c1a572), (0xb14, 0xfa0fb2a25f9a6daa),
            (0x46d4, 0x66e5a41fe82edbb5), (0x5e08, 0xe7a79d07a42bb0dd),
            (0x5ecc, 0x8bc61c0c39d60c8b), SCALAR_FIELD_INTERPRETER,
        ],
        (Some("MainCS"), [4, 4, 4]) =>
            &[(0x4e98, 0xdce6ffe25dfb267a), (0x6510, 0xad6ae60bb84e14ba)],
        (Some("MarkValid_PrepareCS"), [32, 1, 1]) =>
            &[(2156, 0x1ece7ffcb2a479e1)],
        (Some("GenerateReflectionRaysCS"), [1024, 1, 1]) =>
            &[(3340, 0x5bc1cc7ce6066550)],
        (Some("ScreenProbeConvertToIrradianceCS"), [4, 4, 1]) =>
            &[(3412, 0x866405adb7fafa7c), (3624, 0xeb00c121a26580f4)],
        (Some("ScreenProbeConvertToIrradianceCS"), [8, 8, 1]) =>
            &[(3624, 0x63d66671fc0a1366), (3412, 0x12fe7f4e4ffe1884)],
        (Some("ScreenProbeConvertToIrradianceCS"), [16, 16, 1]) =>
            &[(3412, 0x193b9f3175b7f9ec), (3624, 0xf4da31a196be9727)],
        (Some("ReduceCS"), [8, 8, 1]) => &[
            (0x8c0, 0x5f94576096b63dd7), (0x938, 0x43d8c1d206699c1a),
            (0x9fc, 0x7fb3a86f58ef1edf), (0x7c0, 0x54aa6243086c92fa),
            (0x694, 0x3685b18a31dcaef), (0x7fc, 0x87b11799ebe8ab0a),
            (0xb40, 0x46a65b5e5b7c6364), (0x898, 0x385455d50f930be8),
            (0x7fc, 0xd67bd59c80613095), (0x938, 0xa630f54cca8e4781),
            (0xaf0, 0x3637edf9d6234435), (0x7c0, 0xad1dc2fa32090887),
            (0x6bc, 0xe788cec91f881ca), (0x9c8, 0x6fac1617fd83ec99),
            (0x694, 0xc699332963136bc1), (0x8f0, 0x73f689cdfad1b41d),
            (0x6bc, 0x3704d7b46717e471), (0x8f0, 0x94166f67dd42fe16),
        ],
        (Some("SetupIndirectCS"), [8, 8, 1]) => &[
            (0x894, 0x278f52380e3e767), (0x638, 0x8162094e83094731),
            (0x52c, 0xbebe1b8516937ed3), (0x7fc, 0x1e2aa5e8f0e0492b),
            (0x4d0, 0x6ce35a59da1e463f), (0x6d4, 0xd3d2aa8d15a1d6d2),
        ],
        (Some("AllocateUsedProbesCS"), [4, 4, 4]) =>
            &[(0x390, 0x81592c871bda88ec), (0x438, 0x1f50df488adab82b)],
        (Some("AllocateProbeTracesCS"), [4, 4, 4]) => &[(0x524, 0x263dbbe09d8cb9e9)],
        (Some("UpdateCacheForUsedProbesCS"), [4, 4, 4]) => &[(0x310, 0x8cf39cf63a6deb53)],
        (Some("ComputeCulledTilesStartOffsetCS"), [8, 8, 1]) => &[(0x198, 0xfc9b56774be15eff)],
        (Some("WaterTileCatergorisationCS"), [8, 8, 1]) => &[(0x2f4, 0xa988bee2d9bfe99a)],
        (Some("FLumenVisualizeCreateRaysCS"), [64, 1, 1]) => &[(0x25c, 0xcbbb6dcf2d5d0444)],
        (Some("SimulateMainComputeCS"), [64, 1, 1]) => &[(0x1b64, 0x8275264b84babc75)],
        (Some("CullObjectsForShadowCS"), [64, 1, 1]) =>
            &[(0x618, 0x8df216bcfc666c6a), (0x87c, 0x301604b6ad761ec9)],
        (Some("BuildSymbolBufferCS"), [64, 1, 1]) =>
            &[(0x21e0, 0xc097f65a33169935)],
        (Some("MainDrawDebugShadowCS"), [8, 8, 1]) =>
            &[(0x404, 0x456eb138997ddf5c)],
        (Some("SetupVoxelTracesCS"), [4, 4, 4]) =>
            &[(0x2d4, 0x9654e55254d67657)],
        (Some("DepthExport"), [8, 8, 1]) =>
            &[(0xb74, 0xb8f638844c43ba0b), (0x14c4, 0x4b87869861958c9b)],
        (Some("GenerateProbeTraceTilesCS"), [8, 8, 1]) =>
            &[(0x35c, 0xb403ec1c63de7a13), (0x9b0, 0x46b23175b16b9945)],
        (Some("ReflectionTileClassificationBuildListsCS"), [8, 8, 1]) =>
            &[(0x6e4, 0x9d6b53fa2d02366c), (0x5e8, 0x4086c3fedd3beafc)],
        (Some("CullMeshSDFObjectsForViewCS"), [64, 1, 1]) =>
            &[(0x320, 0x5d67548baa66e61a)],
        (Some("TileMainCS"), [8, 8, 1]) =>
            &[(0x57c, 0xa3bc6beaa81928b1)],
        (Some("CompactVisBufferCS"), [4, 4, 4]) =>
            &[(0x3bc, 0xbd6ccef7ec3c4a0)],
        (Some("CocFlattenMainCS"), [8, 8, 1]) =>
            &[(700, 0x4e4b5231a51d4ccd), (868, 0xdee3f7409b7d2aab)],
        _ => &[],
    };
    if signatures.iter().any(|&(size, hash)| matches_program(&ex.isa, size, hash)) { return true; }
    compute_group(ex) == [64, 1, 1]
        && (ex.entry.as_deref() == Some("AllocateCommandInstanceOutputSpaceCs")
            && matches_program(&ex.isa, 468, 0xf082cd3768ec31f0)
        || ex.entry.as_deref() == Some("CompactFeedbackHashTableCS")
            && matches_program(&ex.isa, 348, 0x5976decd660b00a3)
        || ex.entry.as_deref() == Some("CullToVoxelClipmapCS")
            && matches_program(&ex.isa, 404, 0xcc708f80a9096e5d)
        || ex.entry.as_deref() == Some("BuildCardsUpdateListCS") && [
            (2440, 0x4d1c4843bac1c845),
            (2640, 0x72b5ee7a38d3445d),
        ].into_iter().any(|(size, hash)| matches_program(&ex.isa, size, hash))
        || ex.entry.as_deref() == Some("InstanceCullVSM") && [
            (7740, 0x2cb56c8263df11b9), (5644, 0x2a06c03631aa33d4),
            (8068, 0x0d2da7ed428e363a), (5636, 0xdcf5938525abc31a),
            (8092, 0x1ef46547771249a3), (5380, 0x0815f4188e78d1ad),
            (7740, 0x138ce6201f23fcc9), (5396, 0x83f25286a0b05e8d),
        ].into_iter().any(|(size, hash)| matches_program(&ex.isa, size, hash)))
}

/// Local storage addressed by the audited single-wave LDS sequences. The rest
/// of the host shared-memory budget remains available for lane exchange.
pub(super) fn lds_words(em: &Emitter<'_>) -> Option<u32> {
    if em.hair_multisample { return Some(2); } // group allocation base and total
    if !em.wave_cull { return None; }
    Some(match em.ex.entry.as_deref()? {
        "GenerateReflectionRaysCS" => 1040, // 1024 bucket counts followed by 16 wave totals
        "CullPerPageDrawCommandsCs" | "VirtualSmInvalidateInstancePagesCS" => 320, // five rows of 64 lanes
        "ComputeCulledTilesStartOffsetCS" => 2, // total and allocation base
        "SetupIndirectCS" => 1, // tile flags
        "ReduceCS" => 4, // near/far masks and their allocation bases
        "WaterTileCatergorisationCS" => 64, // one word per tile lane
        "GenerateProbeTraceTilesCS" if em.ex.isa.len() == 0x9b0 => 1027, // two queues of 256 uint2 tiles, two counts, one base
        "MainCS" if em.ex.isa.len() == 0xb14 => 390, // six 64-lane planes and their allocation bases
        "MainCS" if matches!(em.ex.isa.len(), 0x6510 | 0x5ecc | 0x7600) => 2048, // field interpreter working storage
        "GenerateProbeTraceTilesCS" => 1, // allocation base
        "ReflectionTileClassificationBuildListsCS" => 130, // two tiles and their counters
        "TileMainCS" => 256, // two pairs of dwords per lane
        "CompactVisBufferCS" => 386, // 384 visibility records between count and base
        "FLumenVisualizeCreateRaysCS" | "CullMeshSDFObjectsForViewCS" => 66, // counter, 64 indices and base
        _ => return None,
    })
}

/// Scalar EXEC/VCC branches test all lanes, including on a loop backedge.
pub(super) fn branch_condition(em: &Emitter<'_>, i: &Insn, fallback: &str) -> String {
    if em.lockstep.is_some() {
        return super::lockstep::branch_condition(i.op).unwrap_or_else(|| fallback.into());
    }
    if em.resolve_wave32 {
        return match i.op {
            6 => "vcc_lo == 0u", 7 => "vcc_lo != 0u",
            8 => "exec_lo == 0u", 9 => "exec_lo != 0u",
            _ => fallback,
        }.into();
    }
    if !em.wave_cull || em.hoist_wave_exchanges { return fallback.into(); }
    let reg = match i.op {
        6 | 7 => "vcc_lo",
        8 | 9 => "exec_lo",
        _ => return fallback.into(),
    };
    let invert = if i.op & 1 == 0 { "!" } else { "" };
    format!("{invert}any(tb_page_ballot(tb_wave_tid, ({reg} & 1u) != 0u))")
}

// The PC and all scalar branch decisions are identical for the 64 invocations,
// so the group can rendezvous inside the branches and loops actually taken.
pub(super) fn prepare_insn(em: &mut Emitter<'_>, idx: usize, i: &Insn) -> Result<(), String> {
    if !em.hoist_wave_exchanges && i.mnemonic != "s_cbranch_execz" {
        if let Some(code) = super::page_waves::rendezvous(idx, i)? {
            em.line(&code);
        }
    }
    if matches!(i.mnemonic.as_str(), "s_cbranch_scc0" | "s_cbranch_scc1") {
        if let Some(prev) = idx.checked_sub(1).and_then(|p| em.insns.get(p)).filter(|p| p.mnemonic == "s_andn2_b64") {
            em.line(&format!("scc = any(tb_page_ballot(tb_wave_tid, ({} & 1u) != 0u));", r(&prev.dst[0])?));
        }
    }
    Ok(())
}

pub(super) fn emit(em: &mut Emitter<'_>, idx: usize, i: &Insn, target: Option<usize>) -> Result<bool,String> {
    let op = i.mnemonic.as_str();
    if em.own_lane_reads.contains(&idx) {
        em.ws(&i.dst[0], &r(&i.src[0])?)?;
        return Ok(true);
    }
    // A lane that a loop's mask branches leave behind only has its own EXEC bit to follow.
    if em.ex.stage == Stage::Mesh && em.in_loop(idx) {
        let test = match op {
            "s_cbranch_execz" => "(exec_lo & 1u) == 0u",
            "s_cbranch_vccz" => "(vcc_lo & 1u) == 0u",
            "s_cbranch_vccnz" => "(vcc_lo & 1u) != 0u",
            _ => "",
        };
        if !test.is_empty() {
            if let Some(t) = target {
                em.line(&format!("if ({test}) j{t} = true;"));
            }
            return Ok(true);
        }
    }
    if !em.hoist_wave_exchanges && i.enc == super::Enc::Sopp && (6..=9).contains(&i.op) {
        if let Some(t) = target {
            em.line(&format!("if ({}) j{t} = true;", branch_condition(em, i, "")));
        }
        // A backwards branch uses the enclosing loop's condition.
        return Ok(true);
    }
    match op {
        "s_cmp_lg_u32" if i.src == [Opnd::ExecHi, Opnd::ConstInt(0)] => {
            em.line("scc = (tb_page_ballot(tb_wave_tid, (exec_lo & 1u) != 0u).y != 0u);");
        }
        "s_cselect_b32" if i.src == [Opnd::ExecHi, Opnd::ExecLo] => {
            em.line(&format!("uint2 exec_words{idx} = tb_page_ballot(tb_wave_tid, (exec_lo & 1u) != 0u);"));
            em.ws(&i.dst[0], &format!("(scc != 0u ? exec_words{idx}.y : exec_words{idx}.x)"))?;
        }
        "s_cbranch_vccnz" | "s_cbranch_vccz" if op == "s_cbranch_vccnz" || em.ex.stage == Stage::Mesh => {
            let target = target.ok_or("backward VCC branch in instance cull")?;
            if em.hoist_wave_exchanges {
                let invert = if op == "s_cbranch_vccz" { "!" } else { "" };
                em.line(&format!("if ({invert}any(page_vcc{idx})) j{target} = true;"));
            } else {
                em.line(&format!("if (any(tb_page_ballot(tb_wave_tid, (vcc_lo & 1u) != 0u))) j{target} = true;"));
            }
        }
        "v_readfirstlane_b32" => {
            if !em.hoist_wave_exchanges { em.line(&first_read(idx, i)?); }
            em.ws(&i.dst[0], &format!("first_value{idx}"))?;
        }
        "s_mov_b32" if matches!(i.dst.first(), Some(Opnd::ExecLo | Opnd::ExecHi)) => {
            let side = if i.dst[0] == Opnd::ExecLo { "<" } else { ">=" };
            em.line(&format!("if ((tb_wave_tid & 63u) {side} 32u) exec_lo = ({} >> (tb_wave_tid & 31u)) & 1u;", r(&i.src[0])?));
        }
        "v_permlanex16_b32" => {
            if !em.hoist_wave_exchanges {
                em.line(&cross_row(idx, i)?);
            }
            em.wv(&i.dst[0], &format!("other_row{idx}"))?;
        }
        "v_add_nc_u32" | "v_add_f32" | "v_min_f32" | "v_max_f32" | "v_max_u32" if matches!(i.src.first(), Some(Opnd::Raw(233 | 250))) => {
            let (read, valid) = dpp_read(idx, i)?;
            if !em.hoist_wave_exchanges { em.line(&read); }
            let other = r(&i.src[1])?;
            let result = match op {
                "v_max_u32" => format!("max(shifted{idx}, {other})"),
                "v_add_f32" => format!("asuint(asfloat(shifted{idx}) + asfloat({other}))"),
                "v_min_f32" => format!("asuint(min(asfloat(shifted{idx}), asfloat({other})))"),
                "v_max_f32" => format!("asuint(max(asfloat(shifted{idx}), asfloat({other})))"),
                _ => format!("(shifted{idx} + {other})"),
            };
            em.wv(&i.dst[0], &format!("({valid} ? {result} : {other})"))?;
        }
        _ => return super::page_waves::emit(em, idx, i, target),
    }
    Ok(true)
}

// Workgroup barriers must be reached by all four waves. Exchange register
// values before applying each guest wave's forward branch to its writeback.
pub(super) fn exchange(idx: usize, i: &Insn, mesh: bool) -> Result<Option<String>, String> {
    match i.mnemonic.as_str() {
        "s_cbranch_vccnz" | "s_cbranch_vccz" if i.op == 7 || mesh =>
            Ok(Some(format!("uint2 page_vcc{idx} = tb_page_ballot(tb_wave_tid, (vcc_lo & 1u) != 0u);"))),
        "v_permlanex16_b32" => Ok(Some(cross_row(idx, i)?)),
        "v_readfirstlane_b32" => Ok(Some(first_read(idx, i)?)),
        "v_add_nc_u32" | "v_max_u32" | "v_add_f32" if matches!(i.src.first(), Some(Opnd::Raw(233 | 250))) => Ok(Some(dpp_read(idx, i)?.0)),
        _ => super::page_waves::rendezvous(idx, i),
    }
}

fn first_read(idx: usize, i: &Insn) -> Result<String, String> {
    let value = r(&i.src[0])?;
    // RDNA2 selects lane zero when EXEC is empty, even for a scalar write in compute.
    Ok(format!("uint2 first_mask{idx} = tb_page_ballot(tb_wave_tid, (exec_lo & 1u) != 0u);
uint first_lane{idx} = first_mask{idx}.x != 0u ? uint(firstbitlow(first_mask{idx}.x)) : first_mask{idx}.y != 0u ? 32u + uint(firstbitlow(first_mask{idx}.y)) : 0u;
uint first_value{idx} = tb_page_read(tb_wave_tid, {value}, first_lane{idx});"))
}

fn cross_row(idx: usize, i: &Insn) -> Result<String, String> {
    let selector = match (&i.src[1..], i.raw[0] & 0xff00) {
        ([Opnd::ConstInt(0), Opnd::ConstInt(0)], 0) => 0,
        ([Opnd::ConstInt(-1), Opnd::ConstInt(-1)], 0x4800) => 15,
        _ => return Err("unrecognized cross-row cull permutation".into()),
    };
    let src = r(&i.src[0])?;
    Ok(format!("uint other_row{idx} = tb_page_read(tb_wave_tid, {src}, ((tb_wave_tid & ~15u) ^ 16u) + {selector}u);"))
}

fn dpp_read(idx: usize, i: &Insn) -> Result<(String, String), String> {
    let code = i.raw[1] >> 8;
    let src = format!("v{}", i.raw[1] & 255);
    let mut valid = "true".to_owned();
    let lane = match (i.src[0], code) {
        (Opnd::Raw(233), 0xde54c1 | 0x936012 | 0x000924) =>
            format!("(tb_wave_tid & ~7u) | (({code:#x}u >> (3u * (tb_wave_tid & 7u))) & 7u)"),
        (Opnd::Raw(250), 0xff0940) => "(tb_wave_tid ^ 15u)".into(),
        (Opnd::Raw(250), 0xff0911 | 0xff0112 | 0xff0114 | 0xff0118) => {
            let k = code & 15;
            valid = format!("((tb_wave_tid & 15u) >= {k}u)");
            format!("tb_wave_tid - {k}u")
        }
        _ => return Err("unrecognized cull DPP control".into()),
    };
    Ok((format!("uint shifted{idx} = tb_page_read(tb_wave_tid, {src}, {lane});"), valid))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn readfirstlane_selects_lane_zero_when_both_exec_words_are_empty() {
        // v_readfirstlane_b32 s28, v9
        let ops = crate::rdna2::disassemble(&0x7e38_0509u32.to_le_bytes());
        for mesh in [false, true] {
            let code = exchange(0, &ops[0], mesh).unwrap().unwrap();
            assert!(code.contains("first_mask0.y != 0u ? 32u + uint(firstbitlow(first_mask0.y)) : 0u"), "{code}");
        }
    }
}
