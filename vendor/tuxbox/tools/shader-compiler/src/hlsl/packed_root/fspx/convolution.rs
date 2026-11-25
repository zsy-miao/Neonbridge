//! The audited convolution passes have one tensor UAV and one read-only weight
//! buffer. Weight vectors use immediate offsets, sometimes added to a scalar byte index.
use super::*;
use crate::meta::RootRange;

/// The wave-size bit and workgroup size are independent (the 128-thread pass
/// exchanges four separate wave32 results through LDS).
pub(in crate::hlsl) fn layout(ex: &ShaderExtract) -> Option<RootAbi> {
    let [0x4005cf00, 0x6c090102, 0, group, mode, shader, storage] = ex.fspx_config? else {
        return None;
    };
    let [threads, 1, 1] = ex.psv_threads? else {
        return None;
    };
    if group != (0x10000 | threads) {
        return None;
    }
    let (abi, programs): (_, &[(usize, u64)]) = match (threads, mode, shader, storage) {
        (32, 0x100001, 0x402c0345, 0x19c) => (
            RootAbi::RegisterWave32,
            &[(3852, 0xd0be69c77bcf1c68), (0x42b4, 0x1f4df0702d59619d)],
        ),
        (64, 0x100001, 0x402c0347, 0x19c) => (
            RootAbi::RegisterWave32,
            &[(8636, 0xf017f2f3910a91d4), (8692, 0x5e6613904ba3e659)],
        ),
        (128, 0x100001, 0x402c0347, 0x2019c) => {
            (RootAbi::RegisterWave32, &[(0x27d0, 0xd20b5cd0ccbda176)])
        }
        (32, 0x100001, 0x402c0347, 0x39c) => {
            (RootAbi::RegisterWave32, &[(0x415c, 0x6be7a2ed20a6c196)])
        }
        (32, 0x100001, 0x402c0347, 0x19c) => {
            (RootAbi::RegisterWave32, &[(0x40c8, 0x9f726559794150b2)])
        }
        (128, 0x100001, 0x402c0347, 0x4009c) => {
            (RootAbi::RegisterWave32, &[(0x5498, 0x432ae0bd65948c8a)])
        }
        (64, 0x100001, 0x402c034d, 0x9c) => {
            (RootAbi::RegisterWave32, &[(0x35c0, 0x23abdeb445ccd988)])
        }
        (64, 1, 0x402c034f, 0x9c) => (
            RootAbi::RegisterWave64,
            &[
                (0x63c8, 0x9f5ac4687d6f5ac1),
                (0x6598, 0xa2b66c345e3c8b84),
                (0xa178, 0x6bf485c97f5d4e50),
            ],
        ),
        _ => return None,
    };
    programs
        .iter()
        .any(|&(len, hash)| matches_program(&ex.isa, len, hash))
        .then_some(abi)
}

pub(super) fn bindings(
    ex: &ShaderExtract,
    insns: &[Insn],
    uav: &RootRange,
    srv: &RootRange,
    abi: RootAbi,
) -> Result<PackedRoot, String> {
    let resources = [(2, 0), (4, 18), (7, 11)];
    let has_dimensions = insns
        .iter()
        .any(|ins| ins.mnemonic == "s_buffer_load_dwordx2");
    let resources = if has_dimensions {
        &resources[..]
    } else {
        &resources[1..]
    };
    if uav.base_register != 0
        || uav.num != 13
        || srv.base_register != 0
        || srv.num != 21
        || !matches!(srv.offset_in_table, 0 | u32::MAX)
        || ex.resources.len() != resources.len()
        || !ex.resources.iter().zip(resources).all(|(r, &(ty, reg))| {
            r.res_type == ty && r.space == 0 && r.lower == reg && r.upper == reg
        })
    {
        return Err("unsupported FSPX convolution resources".into());
    }
    let mut plan = PackedRoot {
        abi,
        ..PackedRoot::default()
    };
    let weights = Binding {
        slot: 18,
        uav: false,
    };
    let features = Binding {
        slot: 11,
        uav: true,
    };
    let wait_flags = matches_program(&ex.isa, 0x415c, 0x6be7a2ed20a6c196);
    // The 128-thread LDS pass broadcasts one wave-local row index. The 32-thread
    // poll tests all lanes' flag reads before any lane can start the convolution.
    if wait_flags || matches_program(&ex.isa, 0x27d0, 0xd20b5cd0ccbda176) {
        plan.register_wave_size = Some(32);
    }
    for (idx, ins) in insns.iter().enumerate() {
        if wait_flags {
            match (ins.offset, ins.mnemonic.as_str()) {
                (0x130, "s_and_b32") => {
                    plan.ballot_nonempty.insert(idx, 9);
                }
                // The inner loop counts 65536 polls, the outer advances 32
                // flag indices. A shared translator counter can truncate both.
                (0x14c | 0x15c, "s_cbranch_scc0") => {
                    plan.bounded_backedges.insert(idx);
                }
                _ => {}
            }
        }
        match (ins.enc, ins.mnemonic.as_str(), ins.aux.imm) {
            (Enc::Smem, "s_buffer_load_dword" | "s_buffer_load_dwordx2", offset) => {
                // Both the dispatch dimensions and the extra wave64 parameter
                // use the b0 descriptor built from s[6:7] in s[8:11].
                if ins.src != [Opnd::Sgpr(8)] || ins.raw[1] >> 25 != 125 {
                    return Err("unexpected convolution root CBV operand".into());
                }
                let word = (offset / 4) as u32;
                let count = if ins.mnemonic.ends_with("x2") { 2 } else { 1 };
                plan.loads
                    .insert(idx, (0..count).map(|k| Some((0, word + k))).collect());
            }
            (Enc::Smem, "s_load_dwordx4", 0x160 | 0x240) => {
                plan.loads.insert(idx, vec![None; 4]);
            }
            (
                Enc::Smem,
                "s_buffer_load_dwordx4" | "s_buffer_load_dwordx8" | "s_buffer_load_dwordx16",
                _,
            ) => {
                let offset = u32::try_from(ins.aux.imm).map_err(|_| "invalid weight offset")?;
                let n = 1 << (ins.op & 7);
                let soffset = (ins.raw[1] >> 25) as u8;
                if soffset != 125 {
                    if soffset > 107 {
                        return Err("unexpected convolution weight offset register".into());
                    }
                    plan.raw_offsets.insert(idx, soffset);
                }
                plan.raw_scalar.insert(idx, (weights, offset, n as u8));
            }
            (
                Enc::Mubuf,
                "buffer_load_dword"
                | "buffer_load_dwordx3"
                | "buffer_load_dwordx4"
                | "buffer_store_dword"
                | "buffer_store_dwordx4",
                _,
            ) => {
                if ins.aux.mask == 5 && ins.mnemonic.starts_with("buffer_load") {
                    plan.coherent_loads.insert(idx);
                } else if ins.aux.mask != 1 {
                    return Err("unexpected convolution tensor address".into());
                }
                plan.buffers.insert(idx, (features, 0));
            }
            (Enc::Mubuf, "buffer_atomic_add", 0)
                if ins.aux.mask == 0 && ins.src[2] == Opnd::Sgpr(14) =>
            {
                // Non-returning completion ticket, addressed solely by the byte offset.
                plan.buffers.insert(idx, (features, 0));
            }
            (Enc::Smem | Enc::Mubuf | Enc::Mimg, _, _) => {
                return Err("unexpected FSPX convolution access".into())
            }
            _ => {}
        }
    }
    Ok(plan)
}
