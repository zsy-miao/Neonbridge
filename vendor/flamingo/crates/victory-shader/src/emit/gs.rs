//! A merged ES+GS stage as two host shaders (step 4 of geometry shader
//! support, `merged`, `handoff`): a vertex shader and a geometry shader.
//!
//! The console runs the stage as one wave: the ES half writes each vertex's
//! record to LDS, the GS half reads the records of its triangle's vertices
//! from there, writes its output vertices to LDS too and sends `GS_EMIT` /
//! `GS_CUT`. The host has no LDS in those stages, so the translation splits
//! the stage where the data crosses:
//!
//! * The ES half becomes a vertex shader. Each record dword it writes is
//!   output `k` of its vertex: row `k / 4`, channel `k % 4` of a signature
//!   made for the purpose (`TEXCOORD<row>`, 32-bit integers, the dwords
//!   untouched). The rewrite is of the machine code: a `ds_write_b32` ..
//!   `b128` becomes the `exp param<row>` of the same size, `s_barrier` a
//!   `s_nop` and the `s_setpc_b64` handover an `s_endpgm`.
//! * The GS half becomes a geometry shader (triangles in, triangle strip out,
//!   stream 0). Its reads of an input vertex's record are `loadInput` of that
//!   signature at that vertex. Its writes to its own LDS are writes to an
//!   array private to the invocation, at the offset from the first record
//!   written (the base, the runtime table's value and the lane, only
//!   separate invocations that have one array each here). Its k-th
//!   `GS_EMIT` copies the k-th record of that array to the output signature
//!   and emits the vertex; `GS_CUT` ends the strip.
//!
//! What this assumes, none of it read from the stage data or checked on a
//! title program (see the pull request):
//!
//! * An output record has 4 dwords per signature register: the dword at
//!   `4 * row + column` is that channel of the output with that register, in
//!   the geometry container's output signature, and the records follow each
//!   other without a gap. The written range has to be exactly that many
//!   records, one per emit, or the program is refused.
//! * Each emit instruction in the code is one vertex: emits are numbered in
//!   code order, so alternative arms that emit are refused.
//! * The topology is a triangle strip and the stage has one instance.

use std::collections::HashMap;

use super::container::SigElement;
use crate::ShaderError::{self, Unsupported};
use crate::handoff::{Handoff, Access, VertexRead, own_accesses};
use crate::ir::Program;
use crate::merged::{GsMessage, MergedGs, limits, message_of};
use crate::native::{NativeProgram, StageIo, Usage, VsOutput};

type Site = (usize, usize);

fn unsupported(what: &'static str) -> ShaderError {
    Unsupported { offset: 0, what }
}

/// What the lowering of a GS program does with its LDS accesses and
/// messages.
#[derive(Debug, Clone, Default)]
pub(crate) struct GsPlan {
    /// Reads of an input vertex's record.
    pub reads: HashMap<Site, VertexRead>,
    /// Writes to the program's own array: (first dword in it, dwords, first
    /// VGPR).
    pub stores: HashMap<Site, (u32, u32, u8)>,
    /// Reads back from it.
    pub loads: HashMap<Site, (u32, u32, u8)>,
    /// `GS_EMIT` and friends: the emit's number and the message.
    pub messages: HashMap<Site, (u32, GsMessage)>,
    /// Dwords of an output record.
    pub record_dwords: u32,
    /// Dwords of the array (the records of every emit).
    pub array_dwords: u32,
    pub max_vertices: u32,
}

/// The signature of the records between the halves: one `TEXCOORD` register
/// per four dwords, 32-bit integers.
pub(crate) fn record_signature(dwords: u32) -> Vec<SigElement> {
    (0..dwords.div_ceil(4))
        .map(|row| {
            let cols = (dwords - 4 * row).min(4) as u8;
            SigElement {
                name: "TEXCOORD".into(),
                index: row,
                kind: 0,
                comp: 5,
                interp: 1,
                row: Some(row),
                col: 0,
                cols,
                usage: (1 << cols) - 1,
                half: false,
            }
        })
        .collect()
}

/// The vertex id input of the ES half as a vertex shader.
pub(crate) fn vertex_id_input() -> Vec<SigElement> {
    vec![SigElement {
        name: "SV_VertexID".into(),
        index: 0,
        kind: 1,
        comp: 5,
        interp: 0,
        row: Some(0),
        col: 0,
        cols: 1,
        usage: 1,
        half: false,
    }]
}

/// The ES half as a vertex shader: `code` with the record writes as exports
/// (see the module notes) and the I/O of a vertex shader whose outputs are the
/// record's rows.
pub(crate) fn es_as_vertex_shader(
    merged: &MergedGs,
    es: &Program,
    handoff: &Handoff,
) -> Result<NativeProgram, ShaderError> {
    let mut code = merged.es.code.clone();
    let mut put = |at: usize, words: &[u32]| -> Result<(), ShaderError> {
        if code.get(at..at + 4 * words.len()).is_none() {
            return Err(unsupported("an ES instruction outside the code"));
        }
        for (k, w) in words.iter().enumerate() {
            code[at + 4 * k..at + 4 * k + 4].copy_from_slice(&w.to_le_bytes());
        }
        Ok(())
    };
    for s in &handoff.es.stores {
        let inst = &es.blocks[s.site.0].insts[s.site.1];
        let (row, ch) = (s.dword / 4, s.dword % 4);
        if ch + s.count > 4 {
            return Err(unsupported("a record write across two registers"));
        }
        // exp param<row>, the written VGPRs in their channels
        let en = ((1u32 << s.count) - 1) << ch;
        let word0 = 0xF800_0000 | (32 + row) << 4 | en;
        let mut regs = [0u32; 4];
        for k in 0..s.count {
            regs[(ch + k) as usize] = u32::from(s.reg) + k;
        }
        let word1 = regs[0] | regs[1] << 8 | regs[2] << 16 | regs[3] << 24;
        put(inst.machine.offset, &[word0, word1])?;
    }
    for b in &es.blocks {
        for inst in &b.insts {
            match inst.name() {
                "s_barrier" => put(inst.machine.offset, &[0xBF80_0000])?,
                "s_setpc_b64" => put(inst.machine.offset, &[0xBF81_0000])?,
                _ => {}
            }
        }
    }
    let outputs = (0..handoff.es.dwords.div_ceil(4))
        .map(|row| VsOutput {
            usage: Usage::Generic,
            row: row as u8,
            param: row as u8,
            fp16: false,
            pack2: false,
        })
        .collect();
    Ok(NativeProgram {
        layout: merged.layout.clone(),
        io: StageIo::Vs {
            inputs: Vec::new(),
            outputs,
        },
        code,
        ..merged.es.clone()
    })
}

/// How the lowering of the GS program treats its LDS accesses and messages.
///
/// `outputs` is the geometry container's output signature, which the output
/// records are matched with (module notes).
pub(crate) fn plan(gs: &Program, handoff: &Handoff, outputs: &[SigElement]) -> Result<GsPlan, ShaderError> {
    let lim = limits(gs)?;
    if lim.streams != 1 {
        return Err(unsupported("a geometry stream other than stream 0"));
    }
    let mut messages = HashMap::new();
    let mut emits = 0;
    for (b, block) in gs.blocks.iter().enumerate() {
        for (i, inst) in block.insts.iter().enumerate() {
            let Some(msg) = message_of(inst) else { continue };
            if msg == GsMessage::Done {
                continue;
            }
            messages.insert((b, i), (emits, msg));
            emits += msg.vertices();
        }
    }
    if emits != lim.max_vertices {
        return Err(unsupported("emits on alternative paths of a geometry program"));
    }
    if outputs.iter().any(|e| e.row.is_none()) {
        return Err(unsupported("a geometry output without a register"));
    }
    let record_dwords = outputs.iter().filter_map(|e| e.row).max().map_or(0, |r| 4 * (r + 1));
    let accesses = own_accesses(gs)?;
    let first = accesses.first().ok_or(unsupported("a geometry program that writes no output record"))?;
    if accesses.iter().any(|a: &Access| (a.base, a.mul) != (first.base, first.mul)) {
        return Err(unsupported("geometry output accesses at more than one base"));
    }
    let writes = accesses.iter().filter(|a| a.write);
    let lo = writes.clone().map(|a| a.byte / 4).min().ok_or(unsupported("a geometry program that writes no output record"))?;
    let hi = writes.map(|a| a.byte / 4 + a.count).max().unwrap_or(lo);
    let array_dwords = emits * record_dwords;
    if hi - lo != array_dwords || record_dwords == 0 {
        return Err(unsupported("the output records do not fit the output signature"));
    }
    let mut p = GsPlan {
        record_dwords,
        array_dwords,
        max_vertices: lim.max_vertices,
        messages,
        ..GsPlan::default()
    };
    for a in &accesses {
        let at = a.byte / 4;
        if at < lo || at + a.count > hi {
            return Err(unsupported("a geometry output access outside the records"));
        }
        let entry = (at - lo, a.count, a.reg);
        if a.write {
            p.stores.insert(a.site, entry);
        } else {
            p.loads.insert(a.site, entry);
        }
    }
    p.reads = handoff.reads.iter().map(|r| (r.site, *r)).collect();
    Ok(p)
}

#[cfg(any(test, feature = "synthetic"))]
pub(crate) mod tests {
    #![cfg_attr(not(test), allow(dead_code, unused))]
    use super::*;
    use crate::dxbc::FourCC;
    use crate::handoff::tests::*;
    use crate::native::HwStage;
    use crate::native::tests::container_parts;

    fn bytes(words: &[u32]) -> Vec<u8> {
        words.iter().flat_map(|w| w.to_le_bytes()).collect()
    }

    /// An `OSG1` part: (name, index, system value, component type, register,
    /// mask).
    fn osg1(rows: &[(&str, u32, u32, u32, u32, u8)]) -> Vec<u8> {
        let names_at = 8 + 32 * rows.len();
        let mut out = Vec::new();
        out.extend_from_slice(&(rows.len() as u32).to_le_bytes());
        out.extend_from_slice(&8u32.to_le_bytes());
        let mut names = Vec::new();
        for &(name, index, sv, comp, reg, mask) in rows {
            let at = (names_at + names.len()) as u32;
            names.extend_from_slice(name.as_bytes());
            names.push(0);
            for v in [0, at, index, sv, comp, reg] {
                out.extend_from_slice(&v.to_le_bytes());
            }
            out.extend_from_slice(&[mask, mask, 0, 0]);
            out.extend_from_slice(&0u32.to_le_bytes());
        }
        out.extend_from_slice(&names);
        out
    }

    /// A position and four float registers.
    fn outputs() -> Vec<u8> {
        osg1(&[
            ("SV_Position", 0, 1, 3, 0, 0xF),
            ("TEXCOORD", 0, 0, 3, 1, 0xF),
            ("TEXCOORD", 1, 0, 3, 2, 0xF),
            ("TEXCOORD", 2, 0, 3, 3, 0xF),
            ("TEXCOORD", 3, 0, 3, 4, 0xF),
        ])
    }

    /// The ES half of the pair: `es_words`, a record of five dwords.
    pub fn es_container() -> Vec<u8> {
        container_parts(
            &[(HwStage::Es, &bytes(&es_words())), (HwStage::Vs, &bytes(&[S_ENDPGM]))],
            &[],
        )
    }

    /// The GS half: reads vertices 0 and 1 (dwords 0-4 of each), loads the
    /// runtime table, writes `emits` output records of 20 dwords at the
    /// address that load gives (plus its offsets), emits that many vertices
    /// and cuts after the last.
    pub fn gs_words(emits: u32) -> Vec<u32> {
        let mut w = crate::handoff::tests::gs_words();
        w.pop();
        // s_load_dword s0, s[8:9], 0xb8; v_mov_b32 v12, s0
        w.extend([0xF400_0004, 0xFA00_00B8, 0x7E18_0200]);
        for n in 0..emits {
            for row in 0..5 {
                w.push(ds(DS_WRITE_B128, n * 80 + row * 16));
                w.push(ds_regs(12, if row == 0 { 4 } else { 20 }, 0));
            }
        }
        let emit = 2 | 2 << 4;
        let emit_cut = 2 | 3 << 4;
        for n in 0..emits {
            w.push(0xBF90_0000 | if n + 1 == emits { emit_cut } else { emit });
        }
        w.push(0xBF90_0003); // s_sendmsg GS_DONE
        w.push(S_ENDPGM);
        w
    }

    pub fn gs_container(emits: u32) -> Vec<u8> {
        container_parts(&[(HwStage::Gs, &bytes(&gs_words(emits)))], &[(FourCC::OSG1, &outputs())])
    }

    fn native_of(container: &[u8]) -> crate::native::NativeProgram {
        let c = crate::dxbc::Container::parse(container).unwrap();
        crate::native::native_programs(c.part(FourCC::DXIL).unwrap().data).unwrap().remove(0)
    }

    fn signature_of(container: &[u8], fourcc: FourCC) -> Vec<SigElement> {
        let c = crate::dxbc::Container::parse(container).unwrap();
        super::super::signature(c.part(fourcc).unwrap().data).unwrap()
    }

    #[test]
    fn the_record_signature_is_a_register_per_four_dwords() {
        let s = record_signature(6);
        assert_eq!(s.len(), 2);
        assert_eq!((s[0].row, s[0].cols, s[1].row, s[1].cols), (Some(0), 4, Some(1), 2));
        assert!(s.iter().all(|e| e.comp == 5 && e.interp == 1 && e.name == "TEXCOORD"));
    }

    #[test]
    fn the_es_half_becomes_a_vertex_shader_exporting_its_record() {
        let (merged, h) = super::super::merged_halves(&es_container(), &gs_container(3)).unwrap();
        let ir = crate::spill::build(&merged.es).unwrap();
        let vs = es_as_vertex_shader(&merged, &ir, &h).unwrap();
        let ir = crate::spill::build(&vs).unwrap();
        let names: Vec<_> = ir.blocks.iter().flat_map(|b| b.insts.iter().map(|i| i.name())).collect();
        // the two writes are exports of the same size, the barrier a nop and
        // the handover an end
        assert_eq!(names, ["v_lshlrev_b32", "exp", "exp", "s_endpgm"]);
        assert_eq!(vs.code.len(), merged.es.code.len());
        let StageIo::Vs { outputs, .. } = &vs.io else { panic!("not a vertex program") };
        assert_eq!(outputs.iter().map(|o| (o.row, o.param)).collect::<Vec<_>>(), [(0, 0), (1, 1)]);
    }

    #[test]
    fn the_gs_plan_numbers_the_emits_and_places_the_records() {
        let (_, h) = super::super::merged_halves(&es_container(), &gs_container(3)).unwrap();
        let gs = crate::spill::build(&native_of(&gs_container(3))).unwrap();
        let outs = signature_of(&gs_container(3), FourCC::OSG1);
        let p = plan(&gs, &h, &outs).unwrap();
        assert_eq!((p.record_dwords, p.array_dwords, p.max_vertices), (20, 60, 3));
        let mut m: Vec<_> = p.messages.values().map(|(n, msg)| (*n, *msg)).collect();
        m.sort_by_key(|x| x.0);
        assert_eq!(m[2], (2, GsMessage::EmitCut { stream: 0 }));
        assert_eq!(p.stores.len(), 15);
        assert!(p.stores.values().all(|s| s.0 % 4 == 0 && s.1 == 4));
        assert_eq!(p.reads.len(), 2);
        // Records that do not fill the signature's size are refused.
        let (_, h2) = super::super::merged_halves(&es_container(), &gs_container(2)).unwrap();
        let gs2 = crate::spill::build(&native_of(&gs_container(2))).unwrap();
        assert!(plan(&gs2, &h2, &outs[..4]).is_err());
    }

    /// A merged stage of synthetic programs comes out as a vertex shader and
    /// a geometry shader of the declared shapes, which the validator, where
    /// there is one, accepts.
    #[test]
    fn a_pair_is_a_vertex_shader_and_a_geometry_shader() {
        let out = super::super::translate_merged_gs(&es_container(), &gs_container(3)).unwrap();
        assert_eq!(out.record_dwords, 5);
        // Program kinds: vertex (1) and geometry (2), shader model 6.6.
        let kind = |dxil: &[u8]| {
            let c = crate::dxbc::Container::parse(dxil).unwrap();
            c.part(FourCC::DXIL).unwrap().data[..4].to_vec()
        };
        assert_eq!(kind(&out.vertex.dxil), [0x66, 0, 1, 0]);
        assert_eq!(kind(&out.geometry.dxil), [0x66, 0, 2, 0]);
        // The geometry shader takes what the vertex shader gives, register for
        // register, and gives the container's own outputs.
        let vs_out = signature_of(&out.vertex.dxil, FourCC::OSG1);
        let gs_in = signature_of(&out.geometry.dxil, FourCC::ISG1);
        assert_eq!(vs_out.len(), 2);
        for (o, i) in vs_out.iter().zip(&gs_in) {
            assert_eq!((&o.name, o.index, o.row, o.cols, o.comp), (&i.name, i.index, i.row, i.cols, i.comp));
        }
        let gs_out = signature_of(&out.geometry.dxil, FourCC::OSG1);
        assert_eq!(
            gs_out.iter().map(|e| e.row).collect::<Vec<_>>(),
            [Some(0), Some(1), Some(2), Some(3), Some(4)]
        );
        #[cfg(windows)]
        if let Ok(mut v) = super::super::validator::Validator::load(
            super::super::validator::Validator::default_path().as_deref(),
        ) {
            v.validate(&mut out.vertex.dxil.clone()).unwrap();
            v.validate(&mut out.geometry.dxil.clone()).unwrap();
        }
    }

    /// The host makes a pipeline of the pair: vertex shader, geometry shader
    /// and a pixel stage, as the pipeline path of the host does.
    #[test]
    #[cfg(windows)]
    fn the_host_makes_a_pipeline_of_the_pair() {
        use super::super::host::{Host, stub_ps};
        let Ok(host) = Host::new(false) else {
            eprintln!("no SM 6.6 device; skipped");
            return;
        };
        let out = super::super::translate_merged_gs(&es_container(), &gs_container(3)).unwrap();
        let ps = stub_ps();
        let root = host.root_signature(&out.vertex, &ps).unwrap();
        host.pipeline_with_gs(&root, &out.vertex.dxil, Some(&out.geometry.dxil), &ps.dxil)
            .unwrap();
    }
}
