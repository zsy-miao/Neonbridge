//! The merged ES+GS stage (a geometry shader), step 1 of its support.
//!
//! The console links two programs into one hardware stage when it creates a
//! pipeline with a geometry shader: the ES program, which sits in the vertex
//! container next to the ordinary VS, and the GS program, which is all the
//! geometry container holds. Both run on the same wave, so they share one
//! SGPR file: eight system SGPRs first, then the user data of the root
//! signature ([`HwStage::user_sgpr`]).
//!
//! What this step knows, from the title's programs as the issue on this
//! (fork issue 138) writes it down (static analysis, not a run):
//!
//! * The GS half loads from a pointer in user data 0 and 1 (s8 and s9) that
//!   is no root parameter: the console runtime's own table. [`RUNTIME_TABLE`]
//!   names it; resource tracking takes the pair as that table when the merged
//!   root layout (both halves', [`pair_es_gs`]) puts nothing there
//!   ([`crate::resource::track_merged`]; a geometry program tracked alone cannot
//!   know the ES half leaves them free), and [`crate::io::Input::RuntimeTable`]
//!   names the two dwords.
//! * A root parameter is the same in both halves: [`pair_es_gs`] merges the
//!   two layouts and refuses programs that disagree about a user-data dword.
//!
//! What it does not do yet: translate the stage. The ES half hands its
//! records over through LDS and the GS half reads them from there
//! ([`crate::handoff`] finds both); the GS half writes its output vertices
//! to LDS too, with `s_sendmsg` GS_EMIT, GS_CUT and GS_DONE ([`limits`]
//! counts them); the output is not modeled. Emission refuses a geometry
//! program with [`crate::emit::GS_EMIT`].

use crate::ShaderError::{self, Malformed};
use crate::gfx10::Fields;
use crate::ir::{Exit, Program};
use crate::native::{HwStage, NativeProgram, RootParam, RootType, StageIo};

/// The first user-data dword of the runtime table pointer: it takes dwords 0
/// (low half) and 1 (high half) of the merged stage's user data.
pub const RUNTIME_TABLE: u8 = 0;

/// An ES program and the GS program it is linked with.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct MergedGs {
    pub es: NativeProgram,
    pub gs: NativeProgram,
    /// The root parameters of the merged stage: those of either half, by user
    /// data (a parameter both halves list is in it once).
    pub layout: Vec<RootParam>,
}

impl MergedGs {
    /// LDS bytes the merged stage allocates: what the two records say (they
    /// describe one allocation; the larger covers a record that says less).
    pub fn lds_bytes(&self) -> u32 {
        let of = |p: &NativeProgram| match &p.io {
            StageIo::Es(io) | StageIo::Gs(io) => io.lds_bytes,
            _ => 0,
        };
        of(&self.es).max(of(&self.gs))
    }

    /// The GS program as the merged stage runs it: the code of the GS half
    /// with the root layout of both halves, which is what its user SGPRs
    /// hold.
    pub fn program(&self) -> NativeProgram {
        NativeProgram {
            layout: self.layout.clone(),
            ..self.gs.clone()
        }
    }
}

/// Pairs an ES program with the GS program of the same pipeline.
///
/// Fails when the programs are not an ES and a GS, run at different wave
/// sizes, or disagree about what a user-data dword holds (the two halves
/// share the SGPRs, so a dword has one meaning).
pub fn pair_es_gs(es: NativeProgram, gs: NativeProgram) -> Result<MergedGs, ShaderError> {
    if es.stage != HwStage::Es {
        return Err(Malformed("the vertex half of a merged stage is not an ES program"));
    }
    if gs.stage != HwStage::Gs {
        return Err(Malformed("the geometry half of a merged stage is not a GS program"));
    }
    if es.wave64 != gs.wave64 {
        return Err(Malformed("ES and GS programs run at different wave sizes"));
    }
    let mut layout = es.layout.clone();
    for g in &gs.layout {
        let overlaps = |e: &RootParam| {
            // (the count is read from the container: no overflow)
            let end = |p: &RootParam| u32::from(p.user_data).saturating_add(p.sgprs);
            u32::from(e.user_data) < end(g) && u32::from(g.user_data) < end(e)
        };
        // The same table pointer listed by both halves with other ranges (the
        // offset and the base register of a range differ) is one root
        // parameter read through two ranges: both are kept.
        let same_pointer = |e: &RootParam| {
            g.root == RootType::Table
                && (e.root, e.user_data, e.sgprs, e.slot) == (g.root, g.user_data, g.sgprs, g.slot)
        };
        let over: Vec<RootParam> = layout.iter().filter(|e| overlaps(e)).copied().collect();
        if over.contains(g) && over.iter().all(|e| e == g || same_pointer(e)) {
            continue;
        }
        if !over.iter().all(same_pointer) {
            return Err(Malformed("ES and GS programs disagree about a user-data dword"));
        }
        layout.push(*g);
    }
    layout.sort_by_key(|p| (p.user_data, p.slot));
    Ok(MergedGs { es, gs, layout })
}

/// A geometry message of `s_sendmsg` (`simm16`: message in bits 0-3,
/// operation in 4-6, stream in 8-9), the ones the GS half sends instead of
/// exporting.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum GsMessage {
    /// `GS_OP_EMIT`: the next output vertex of the stream.
    Emit { stream: u8 },
    /// `GS_OP_CUT`: end the stream's current strip.
    Cut { stream: u8 },
    /// `GS_OP_EMIT_CUT`: the next output vertex, then end the strip.
    EmitCut { stream: u8 },
    /// `MSG_GS_DONE`: the wave is finished with its geometry.
    Done,
}

const MSG_GS: u16 = 2;
const MSG_GS_DONE: u16 = 3;

impl GsMessage {
    /// The message `s_sendmsg` sends with this `simm16`; `None` for any
    /// other (`GS_ALLOC_REQ`, `GS_OP_NOP`, ...).
    pub fn decode(simm16: u16) -> Option<GsMessage> {
        let (msg, op, stream) = (simm16 & 0xF, simm16 >> 4 & 7, (simm16 >> 8 & 3) as u8);
        match (msg, op) {
            (MSG_GS_DONE, 0) => Some(GsMessage::Done),
            (MSG_GS, 1) => Some(GsMessage::Cut { stream }),
            (MSG_GS, 2) => Some(GsMessage::Emit { stream }),
            (MSG_GS, 3) => Some(GsMessage::EmitCut { stream }),
            _ => None,
        }
    }

    /// The vertices this message emits (0 or 1).
    pub fn vertices(self) -> u32 {
        u32::from(matches!(self, GsMessage::Emit { .. } | GsMessage::EmitCut { .. }))
    }
}

/// The geometry message of an instruction, if it is an `s_sendmsg` of one.
pub fn message_of(inst: &crate::ir::Inst) -> Option<GsMessage> {
    match inst.fields {
        Fields::Sopp { imm } if matches!(inst.name(), "s_sendmsg" | "s_sendmsghalt") => {
            GsMessage::decode(imm)
        }
        _ => None,
    }
}

/// What a GS program does with its output, from its code. The record does
/// not give these in a layout this crate reads, so the code is the source: a
/// geometry shader declares its maximum before it emits.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct Limits {
    /// Vertices emitted along the longest path through the program.
    pub max_vertices: u32,
    /// Streams that emit or cut (bit n: stream n).
    pub streams: u8,
    /// A `GS_CUT` or `GS_EMIT_CUT` is sent: strips end before the program does.
    pub cuts: bool,
}

/// The output limits of a GS program.
///
/// Fails when a message is sent inside a loop (no bound), or when the
/// program emits nothing at all.
pub fn limits(p: &Program) -> Result<Limits, ShaderError> {
    let unsupported = |what| ShaderError::Unsupported { offset: 0, what };
    let counts: Vec<u32> = p
        .blocks
        .iter()
        .map(|b| b.insts.iter().filter_map(message_of).map(GsMessage::vertices).sum())
        .collect();
    // A block that can reach itself and sends a message has no bound.
    let reaches = |from: usize, to: usize| {
        let mut seen = vec![false; p.blocks.len()];
        let mut stack = p.blocks[from].exit.successors();
        while let Some(b) = stack.pop() {
            if b == to {
                return true;
            }
            if !std::mem::replace(&mut seen[b], true) {
                stack.extend(p.blocks[b].exit.successors());
            }
        }
        false
    };
    // A jump to an address in registers could reach blocks that emit more.
    if p.blocks.iter().any(|b| matches!(b.exit, Exit::Indirect)) {
        return Err(unsupported("an unresolved jump in a geometry program"));
    }
    let mut streams = 0u8;
    let mut cuts = false;
    for (b, block) in p.blocks.iter().enumerate() {
        let sends: Vec<_> = block.insts.iter().filter_map(message_of).collect();
        if sends.iter().any(|m| !matches!(m, GsMessage::Done)) && reaches(b, b) {
            return Err(unsupported("a geometry message inside a loop"));
        }
        for m in sends {
            match m {
                GsMessage::Emit { stream } => streams |= 1 << stream,
                GsMessage::Cut { stream } | GsMessage::EmitCut { stream } => {
                    streams |= 1 << stream;
                    cuts = true;
                }
                GsMessage::Done => {}
            }
        }
    }
    // The longest path of emits; a loop sends nothing, so its back edge is
    // not followed.
    fn longest(p: &Program, counts: &[u32], b: usize, on: &mut Vec<usize>, memo: &mut [Option<u32>]) -> u32 {
        if let Some(n) = memo[b] {
            return n;
        }
        on.push(b);
        let after = match p.blocks[b].exit {
            Exit::End => 0,
            // (refused below: the jump's target is unknown, so is its bound)
            Exit::Indirect => 0,
            ref e => {
                let next: Vec<usize> = e.successors().into_iter().filter(|s| !on.contains(s)).collect();
                next.into_iter()
                    .map(|s| longest(p, counts, s, on, memo))
                    .max()
                    .unwrap_or(0)
            }
        };
        on.pop();
        let n = counts[b] + after;
        memo[b] = Some(n);
        n
    }
    let max_vertices = longest(p, &counts, 0, &mut Vec::new(), &mut vec![None; p.blocks.len()]);
    if max_vertices == 0 {
        return Err(unsupported("a geometry program that emits no vertex"));
    }
    Ok(Limits {
        max_vertices,
        streams,
        cuts,
    })
}

#[cfg(test)]
mod tests {
    use super::*;
    /// `s_sendmsg` with `simm16`.
    fn send(simm16: u16) -> u32 {
        0xBF90_0000 | u32::from(simm16)
    }

    const EMIT: u16 = 2 | 2 << 4;
    const CUT: u16 = 2 | 1 << 4;
    const EMIT_CUT: u16 = 2 | 3 << 4;
    const DONE: u16 = 3;
    const S_ENDPGM: u32 = 0xBF81_0000;

    fn limits_of(words: &[u32]) -> Result<Limits, ShaderError> {
        let code: Vec<u8> = words.iter().flat_map(|w| w.to_le_bytes()).collect();
        limits(&crate::ir::build(&code, true, 256).unwrap())
    }

    #[test]
    fn messages_decode_by_message_operation_and_stream() {
        assert_eq!(GsMessage::decode(EMIT), Some(GsMessage::Emit { stream: 0 }));
        assert_eq!(GsMessage::decode(CUT | 1 << 8), Some(GsMessage::Cut { stream: 1 }));
        assert_eq!(GsMessage::decode(EMIT_CUT | 3 << 8), Some(GsMessage::EmitCut { stream: 3 }));
        assert_eq!(GsMessage::decode(DONE), Some(GsMessage::Done));
        // GS_ALLOC_REQ (9), GS_OP_NOP and a message of no kind here.
        for other in [9, 2, 7, 3 | 1 << 4] {
            assert_eq!(GsMessage::decode(other), None, "{other:#x}");
        }
        assert_eq!(GsMessage::Emit { stream: 0 }.vertices(), 1);
        assert_eq!(GsMessage::EmitCut { stream: 0 }.vertices(), 1);
        assert_eq!(GsMessage::Cut { stream: 0 }.vertices(), 0);
        assert_eq!(GsMessage::Done.vertices(), 0);
    }

    #[test]
    fn straight_line_emits_are_counted_and_a_cut_is_noted() {
        let l = limits_of(&[send(EMIT), send(EMIT), send(EMIT_CUT), send(DONE), S_ENDPGM]).unwrap();
        assert_eq!(l, Limits { max_vertices: 3, streams: 1, cuts: true });
        let l = limits_of(&[send(EMIT | 1 << 8), send(DONE), S_ENDPGM]).unwrap();
        assert_eq!(l, Limits { max_vertices: 1, streams: 2, cuts: false });
    }

    #[test]
    fn the_longest_path_sets_the_maximum() {
        // s_cmp_eq_u32 s0, 0; s_cbranch_scc1 over the two emits: the path
        // that skips them emits none, the other two.
        let l = limits_of(&[
            0xBF06_8000, // s_cmp_eq_u32 s0, 0
            0xBF85_0002, // s_cbranch_scc1 -> the end
            send(EMIT),
            send(EMIT),
            S_ENDPGM,
        ]);
        assert_eq!(l.unwrap().max_vertices, 2);
        // Either of two arms: the larger counts, not the sum.
        let l = limits_of(&[
            0xBF06_8000, // s_cmp_eq_u32 s0, 0
            0xBF85_0003, // s_cbranch_scc1 -> the second arm
            send(EMIT),
            send(EMIT),
            0xBF82_0001, // s_branch -> the end
            send(EMIT),
            S_ENDPGM,
        ]);
        assert_eq!(l.unwrap().max_vertices, 2);
    }

    #[test]
    fn a_message_in_a_loop_or_no_emit_is_refused() {
        // An emit, then s_add_u32 s0, s0, -1 and s_cbranch_scc1 back to it
        // (a branch cannot go to the entry, so an s_nop comes first).
        let looped = limits_of(&[
            0xBF80_0000, // s_nop 0
            send(EMIT),
            0x8000_C100, // s_add_u32 s0, s0, -1
            0xBF85_FFFD, // s_cbranch_scc1 -> the emit
            S_ENDPGM,
        ]);
        assert!(
            matches!(looped, Err(ShaderError::Unsupported { what, .. }) if what == "a geometry message inside a loop"),
            "{looped:?}"
        );
        let none = limits_of(&[send(DONE), S_ENDPGM]);
        assert!(matches!(none, Err(ShaderError::Unsupported { .. })));
    }

    #[test]
    fn the_merged_stage_allocates_what_its_larger_record_says() {
        let io = |lds_bytes| crate::native::MergedIo { lds_bytes };
        let mut es = program(HwStage::Es, vec![]);
        let mut gs = program(HwStage::Gs, vec![]);
        assert_eq!(pair_es_gs(es.clone(), gs.clone()).unwrap().lds_bytes(), 0);
        es.io = StageIo::Es(io(0x2000));
        gs.io = StageIo::Gs(io(0x3C00));
        assert_eq!(pair_es_gs(es, gs).unwrap().lds_bytes(), 0x3C00);
    }

    fn program(stage: HwStage, layout: Vec<RootParam>) -> NativeProgram {
        NativeProgram {
            stage,
            wave64: true,
            vgprs: 8,
            layout,
            used: vec![],
            io: match stage {
                HwStage::Gs => StageIo::Gs(Default::default()),
                HwStage::Es => StageIo::Es(Default::default()),
                _ => StageIo::None,
            },
            code: vec![],
        }
    }

    fn param(root: RootType, user_data: u8, sgprs: u32, slot: u8) -> RootParam {
        RootParam {
            root,
            table_offset_dwords: 0,
            sgprs,
            base_register: 0,
            user_data,
            slot,
        }
    }

    #[test]
    fn a_parameter_both_halves_list_is_in_the_layout_once() {
        let shared = param(RootType::Constants, 2, 4, 0);
        let es = program(HwStage::Es, vec![shared, param(RootType::Table, 6, 1, 1)]);
        let gs = program(HwStage::Gs, vec![shared, param(RootType::Descriptor, 8, 2, 2)]);
        let merged = pair_es_gs(es, gs).unwrap();
        let at: Vec<_> = merged.layout.iter().map(|p| p.user_data).collect();
        assert_eq!(at, [2, 6, 8]);
        // The merged program is the GS code over the merged layout.
        let p = merged.program();
        assert_eq!((p.stage, p.layout.len()), (HwStage::Gs, 3));
    }

    #[test]
    fn the_same_table_pointer_with_other_ranges_in_each_half_is_one_parameter() {
        // One descriptor table (user data 4, root slot 2): the ES half reads
        // a range of it, the GS half another.
        let range = |offset, base| RootParam {
            table_offset_dwords: offset,
            base_register: base,
            ..param(RootType::Table, 4, 1, 2)
        };
        let es = program(HwStage::Es, vec![range(0, 0)]);
        let gs = program(HwStage::Gs, vec![range(8, 3)]);
        let merged = pair_es_gs(es, gs).unwrap();
        assert_eq!(merged.layout, [range(0, 0), range(8, 3)]);
        // A range both list is in it once.
        let es = program(HwStage::Es, vec![range(0, 0)]);
        let gs = program(HwStage::Gs, vec![range(0, 0), range(8, 3)]);
        assert_eq!(pair_es_gs(es, gs).unwrap().layout, [range(0, 0), range(8, 3)]);
        // Another root parameter on that dword is still a conflict.
        let es = program(HwStage::Es, vec![range(0, 0)]);
        let gs = program(HwStage::Gs, vec![RootParam { slot: 3, ..range(8, 3) }]);
        assert!(pair_es_gs(es, gs).is_err());
        // So is a table where the other half has constants.
        let es = program(HwStage::Es, vec![range(0, 0)]);
        let gs = program(HwStage::Gs, vec![param(RootType::Constants, 4, 1, 2)]);
        assert!(pair_es_gs(es, gs).is_err());
    }

    #[test]
    fn a_huge_sgpr_count_does_not_overflow_the_overlap_test() {
        let es = program(HwStage::Es, vec![param(RootType::Constants, 200, u32::MAX, 0)]);
        let gs = program(HwStage::Gs, vec![param(RootType::Constants, 250, 1, 1)]);
        // They overlap: refused, not wrapped around into "no overlap".
        assert!(pair_es_gs(es, gs).is_err());
    }

    #[test]
    fn a_jump_to_a_register_address_has_no_bound() {
        // An emit, then s_setpc_b64 s[0:1]: the target could emit more.
        let r = limits_of(&[send(EMIT), 0xBE80_2000]);
        assert!(matches!(r, Err(ShaderError::Unsupported { what, .. })
            if what == "an unresolved jump in a geometry program"), "{r:?}");
    }

    #[test]
    fn halves_that_disagree_about_a_dword_are_refused() {
        // The same dword, a table in one half and constants in the other.
        let es = program(HwStage::Es, vec![param(RootType::Table, 3, 1, 1)]);
        let gs = program(HwStage::Gs, vec![param(RootType::Constants, 2, 4, 0)]);
        assert!(pair_es_gs(es, gs).is_err());
        // Equal start but not the same parameter.
        let es = program(HwStage::Es, vec![param(RootType::Table, 3, 1, 1)]);
        let gs = program(HwStage::Gs, vec![param(RootType::Table, 3, 1, 2)]);
        assert!(pair_es_gs(es, gs).is_err());
    }

    #[test]
    fn only_an_es_and_a_gs_at_the_same_wave_size_pair() {
        let es = || program(HwStage::Es, vec![]);
        let gs = || program(HwStage::Gs, vec![]);
        assert!(pair_es_gs(es(), gs()).is_ok());
        assert!(pair_es_gs(gs(), es()).is_err(), "swapped");
        assert!(pair_es_gs(program(HwStage::Vs, vec![]), gs()).is_err());
        assert!(pair_es_gs(es(), program(HwStage::Vs, vec![])).is_err());
        let mut narrow = gs();
        narrow.wave64 = false;
        assert!(pair_es_gs(es(), narrow).is_err());
    }
}
