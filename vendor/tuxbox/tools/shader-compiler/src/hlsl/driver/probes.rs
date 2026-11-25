//! Small programs translated for GPU measurements.
use super::*;

/// Dwords per thread in the I/O buffer of [`translate_valu_probe`]: `v0..v{n-1}`, `vcc_lo`,
/// then `exec_lo` (stored only: every thread starts active).
pub fn valu_probe_stride(nvgprs: u32) -> u32 {
    nvgprs + 2
}

/// Emit one LDS exchange inside a synthetic 32-lane control. This diagnostic
/// is not a translation of the surrounding program or its wave control flow.
pub fn translate_lds_exchange_probe(ex: &ShaderExtract, offset: usize) -> Result<String, String> {
    if ex.stage != Stage::Compute {
        return Err("LDS probe needs a compute shader".into());
    }
    let ins = disassemble(&ex.isa).into_iter().find(|i| i.offset == offset)
        .ok_or("missing LDS probe instruction")?;
    let (addr, data, dst) = match (ins.mnemonic.as_str(), ins.src.as_slice(), ins.dst.as_slice()) {
        ("ds_wrxchg_rtn_b32", [Opnd::Vgpr(a), Opnd::Vgpr(v), _], [Opnd::Vgpr(d)]) => (*a, *v, *d),
        _ => return Err("expected one LDS write-exchange instruction".into()),
    };
    let mut em = Emitter::new(ex, vec![ins.clone()]);
    for op in ins.src.iter().chain(ins.dst.iter()) { em.touch(op); }
    em.emit_insn(0, &ins, false, None)?;
    let mut h = String::from("RWBuffer<uint> tb_io : register(u0);\ngroupshared uint tb_lds[8192];\n[numthreads(32,1,1)] void main(uint3 tid : SV_DispatchThreadID) {\n");
    h.push_str("uint exec_lo=1u, exec_hi=0u, vcc_lo=0u, vcc_hi=0u, scc=0u, m0=0u, nul=0u;\n");
    h.push_str(&em.reg_decls());
    let _ = writeln!(h, "\nuint lane=tid.x; tb_lds[lane]=tb_io[lane*4+2]; GroupMemoryBarrierWithGroupSync();");
    let _ = writeln!(h, "v{addr}=tb_io[lane*4]; v{data}=tb_io[lane*4+1]; exec_lo=tb_io[lane*4+3];");
    h.push_str(&em.body);
    h.push_str("GroupMemoryBarrierWithGroupSync();\n");
    let _ = writeln!(h, "tb_io[lane*4+1]=v{dst}; tb_io[lane*4+2]=tb_lds[lane]; }}");
    Ok(h)
}

/// Conformance probe: straight-line vector-ALU code (VOP1/VOP2/VOP3/VOPC/VOP3P words, ending
/// with or without `s_endpgm`) translated with the same per-instruction emitter as real
/// shaders, wrapped in a compute shader that loads each thread's `v0..v{nvgprs-1}` and
/// `vcc_lo` from a raw UAV buffer, runs the code, and stores them back (layout:
/// [`valu_probe_stride`] dwords per thread). The root signature (`UAV(u0)`) is embedded, so
/// the compiled DXIL can create its own root signature.
///
/// This exists so instruction semantics can be compared against a reference model on a real
/// GPU (tests/valu_conformance.rs). Anything other than vector ALU code is refused.
pub fn translate_valu_probe(words: &[u32], nvgprs: u32) -> Result<String, String> {
    let isa: Vec<u8> = words.iter().flat_map(|w| w.to_le_bytes()).collect();
    let ex = ShaderExtract {
        stage: Stage::Compute,
        shader_model: (6, 0),
        required_features: None,
        bitcode_stripped: true,
        fspx_config: None,
        ps09_pixel_inputs: None,
        fssm: Vec::new(),
        entry: Some("main".into()),
        isa_data: isa.clone(),
        isa,
        isa_offset_in_fssm: 0,
        psv_threads: None,
        flat_inputs: Vec::new(),
        flat_outputs: Vec::new(),
        isa_preamble: [0, 64, 1, 1],
        has_input_sig: false,
        has_output_sig: false,
        has_psv0: false,
        has_root_sig: false,
        input_sig: Vec::new(),
        output_sig: Vec::new(),
        resources: Vec::new(),
        gs_info: None,
        mesh_info: None,
        primitive_sig: vec![],
        root_sig: None,
    };
    let insns = disassemble(&ex.isa);
    let mut em = Emitter::new(&ex, insns.clone());
    for (idx, ins) in insns.iter().enumerate() {
        if ins.mnemonic == "s_endpgm" {
            break;
        }
        if !matches!(ins.enc, Enc::Vop1 | Enc::Vop2 | Enc::Vop3 | Enc::Vopc) {
            return Err(format!("{} ({:?}) is not vector ALU code", ins.mnemonic, ins.enc));
        }
        em.emit_insn(idx, ins, false, None).map_err(|e| format!("{}: {e}", ins.mnemonic))?;
    }
    for n in 0..nvgprs {
        em.regs.insert(format!("v{n}"));
    }
    if let Some(r) = em.regs.iter().find(|r| {
        r.strip_prefix('v').and_then(|n| n.parse::<u32>().ok()).is_some_and(|n| n >= nvgprs)
    }) {
        return Err(format!("probe code uses {r}, beyond the {nvgprs} VGPRs loaded"));
    }
    let stride = valu_probe_stride(nvgprs) * 4;
    let mut h = String::new();
    em.push_helpers(&mut h);
    h.push_str("RWByteAddressBuffer tb_io : register(u0);\n");
    h.push_str("[RootSignature(\"UAV(u0)\")]\n[numthreads(64, 1, 1)]\nvoid main(uint3 tid : SV_DispatchThreadID) {\n");
    h.push_str("    uint exec_lo = 1u, exec_hi = 0u, vcc_lo = 0u, vcc_hi = 0u, scc = 0u, m0 = 0u, nul = 0u;\n");
    h.push_str(&em.reg_decls());
    let _ = writeln!(h, "\n    uint tb_base = tid.x * {stride}u;");
    for n in 0..nvgprs {
        let _ = writeln!(h, "    v{n} = tb_io.Load(tb_base + {}u);", n * 4);
    }
    let _ = writeln!(h, "    vcc_lo = tb_io.Load(tb_base + {}u);", nvgprs * 4);
    h.push_str(&em.body);
    for n in 0..nvgprs {
        let _ = writeln!(h, "    tb_io.Store(tb_base + {}u, v{n});", n * 4);
    }
    let _ = writeln!(h, "    tb_io.Store(tb_base + {}u, vcc_lo);", nvgprs * 4);
    let _ = writeln!(h, "    tb_io.Store(tb_base + {}u, exec_lo);", nvgprs * 4 + 4);
    h.push_str("}\n");
    Ok(h)
}

/// Descriptor-table sizes of a [`translate_mem_probe`] shader: how many SRV, sampler and UAV
/// descriptors its root signature declares (at least one each; unused slots may be null).
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct MemProbeTables {
    pub srv: u32,
    pub samplers: u32,
    pub uav: u32,
}

/// User SGPRs holding the descriptor-table pointers in a [`translate_mem_probe`] shader.
pub const MEM_PROBE_SRV_TABLE: u32 = 4;

pub const MEM_PROBE_SAMPLER_TABLE: u32 = 5;

pub const MEM_PROBE_UAV_TABLE: u32 = 6;

/// Conformance probe for vector-memory code: like [`translate_valu_probe`] (same I/O buffer
/// layout, now at `u0, space1`), but the code may also fetch descriptors with `s_load_dword*`
/// from the root tables in s4 (SRVs: T#/V#), s5 (samplers: S#) and s6 (UAVs), and use them
/// in MIMG / MUBUF instructions. A T#/V# at byte offset `32 * i` of its table is HLSL
/// register `t<i>` / `u<i>`, an S# at `16 * i` is `s<i>` (the console's descriptor strides,
/// see `resolve_resources`). Descriptors are resolved by the same dataflow as game shaders,
/// so what is tested is the translation real shaders get.
///
/// The root signature (three descriptor tables, then the I/O UAV) is embedded; table sizes
/// are returned so the caller can fill every slot.
pub fn translate_mem_probe(words: &[u32], nvgprs: u32) -> Result<(String, MemProbeTables), String> {
    use crate::meta::{RootParam, RootParamKind, RootRange, RootSig};
    let isa: Vec<u8> = words.iter().flat_map(|w| w.to_le_bytes()).collect();
    let table = |range_type: u32| RootParam {
        visibility: 0,
        kind: RootParamKind::Table(vec![RootRange { range_type, num: 1, base_register: 0, space: 0, offset_in_table: 0 }]),
    };
    let ex = ShaderExtract {
        stage: Stage::Compute,
        shader_model: (6, 0),
        required_features: None,
        bitcode_stripped: true,
        fspx_config: None,
        ps09_pixel_inputs: None,
        fssm: Vec::new(),
        entry: Some("main".into()),
        isa_data: isa.clone(),
        isa,
        isa_offset_in_fssm: 0,
        psv_threads: None,
        flat_inputs: Vec::new(),
        flat_outputs: Vec::new(),
        isa_preamble: [0, 64, 1, 1],
        has_input_sig: false,
        has_output_sig: false,
        has_psv0: false,
        has_root_sig: true,
        input_sig: Vec::new(),
        output_sig: Vec::new(),
        resources: Vec::new(),
        gs_info: None,
        mesh_info: None,
        primitive_sig: vec![],
        // Root parameters 0, 1, 2 arrive in s4, s5, s6 (compute user-data base 4).
        root_sig: Some(RootSig { version: 1, flags: 0, params: vec![table(0), table(3), table(1)], num_static_samplers: 0, static_samplers: vec![] }),
    };
    let insns = disassemble(&ex.isa);
    let mut em = Emitter::new(&ex, insns.clone());
    em.resolve_resources()?;
    em.infer_tbuf_types();
    for (idx, ins) in insns.iter().enumerate() {
        if ins.mnemonic == "s_endpgm" {
            break;
        }
        let ok = match ins.enc {
            Enc::Vop1 | Enc::Vop2 | Enc::Vop3 | Enc::Vopc | Enc::Mimg | Enc::Mubuf => true,
            Enc::Smem => ins.op <= 4 || (8..=12).contains(&ins.op), // descriptor and buffer loads
            Enc::Sop1 => ins.mnemonic == "s_mov_b32" || ins.mnemonic == "s_mov_b64", // table-pointer copies
            Enc::Sopp => ins.mnemonic == "s_waitcnt" || ins.mnemonic == "s_nop",
            _ => false,
        };
        if !ok {
            return Err(format!("{} ({:?}) is not supported in a memory probe", ins.mnemonic, ins.enc));
        }
        em.emit_insn(idx, ins, false, None).map_err(|e| format!("{}: {e}", ins.mnemonic))?;
    }
    for n in 0..nvgprs {
        em.regs.insert(format!("v{n}"));
    }
    // The table-pointer registers may be copied (their values are meaningless in HLSL:
    // descriptors are resolved statically).
    for n in [MEM_PROBE_SRV_TABLE, MEM_PROBE_SAMPLER_TABLE, MEM_PROBE_UAV_TABLE] {
        em.regs.insert(format!("s{n}"));
    }
    if let Some(r) = em.regs.iter().find(|r| {
        r.strip_prefix('v').and_then(|n| n.parse::<u32>().ok()).is_some_and(|n| n >= nvgprs)
    }) {
        return Err(format!("probe code uses {r}, beyond the {nvgprs} VGPRs loaded"));
    }
    // Table sizes: highest slot used + 1.
    let top = |it: &mut dyn Iterator<Item = u32>| it.max().map_or(1, |m| m + 1);
    let tables = MemProbeTables {
        srv: top(&mut em.tex.keys().copied().chain(em.sbuf.keys().chain(em.tbuf.iter()).filter(|k| !k.0).map(|k| k.1))),
        samplers: top(&mut em.samp.iter().chain(em.samp_cmp.iter()).copied()),
        uav: top(&mut em.uav.keys().copied().chain(em.sbuf.keys().chain(em.tbuf.iter()).filter(|k| k.0).map(|k| k.1))),
    };
    if !em.cb_size.is_empty() || em.uses_code || em.uses_lds {
        return Err("memory probe code may only use SRV, sampler and UAV tables".into());
    }
    let stride = valu_probe_stride(nvgprs) * 4;
    let mut h = String::new();
    em.push_helpers(&mut h);
    h.push_str(&em.resource_decls()?);
    h.push_str("RWByteAddressBuffer tb_io : register(u0, space1);\n");
    let _ = writeln!(
        h,
        "[RootSignature(\"DescriptorTable(SRV(t0, numDescriptors={})), DescriptorTable(Sampler(s0, numDescriptors={})), \
         DescriptorTable(UAV(u0, numDescriptors={})), UAV(u0, space=1)\")]",
        tables.srv, tables.samplers, tables.uav
    );
    h.push_str("[numthreads(64, 1, 1)]\nvoid main(uint3 tid : SV_DispatchThreadID) {\n");
    h.push_str("    uint exec_lo = 1u, exec_hi = 0u, vcc_lo = 0u, vcc_hi = 0u, scc = 0u, m0 = 0u, nul = 0u;\n");
    h.push_str(&em.reg_decls());
    let _ = writeln!(h, "\n    uint tb_base = tid.x * {stride}u;");
    for n in 0..nvgprs {
        let _ = writeln!(h, "    v{n} = tb_io.Load(tb_base + {}u);", n * 4);
    }
    let _ = writeln!(h, "    vcc_lo = tb_io.Load(tb_base + {}u);", nvgprs * 4);
    h.push_str(&em.body);
    for n in 0..nvgprs {
        let _ = writeln!(h, "    tb_io.Store(tb_base + {}u, v{n});", n * 4);
    }
    let _ = writeln!(h, "    tb_io.Store(tb_base + {}u, vcc_lo);", nvgprs * 4);
    let _ = writeln!(h, "    tb_io.Store(tb_base + {}u, exec_lo);", nvgprs * 4 + 4);
    h.push_str("}\n");
    Ok((h, tables))
}
