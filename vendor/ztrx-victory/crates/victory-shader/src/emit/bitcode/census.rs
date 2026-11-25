//! Counts of a module's instructions by kind (a diagnostic): what a
//! translated program costs before the driver's compiler sees it.
//!
//! `VICTORY_SHADER_CENSUS` (any value) turns the counting on; the counts of
//! the module a thread wrote last are read with [`take`]. The `translate`
//! command writes them as a table (see `apps/victory/src/translate.rs`).

use super::{Cast, Module, Op, Tag, Term, V};
use std::cell::RefCell;

/// The kinds, in the order of [`Census::counts`].
pub const KINDS: [&str; 26] = [
    "total",
    "arith",
    "math_call",
    "bitcast",
    "cast",
    "select",
    "guard_select",
    "phi",
    "extract",
    "buffer_load",
    "cbuffer_load",
    "texture_read",
    "lds_load",
    "buffer_store",
    "lds_store",
    "atomic",
    "handle",
    "annotate",
    "dimensions",
    "barrier",
    "wave_op",
    "io_other",
    "gep",
    "clamp",
    "loop_counter",
    "blocks",
];

/// Instruction counts of one written module, by [`KINDS`].
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct Census {
    pub counts: [u32; KINDS.len()],
}

thread_local! {
    /// The modules this thread wrote since the last [`take`]: the length and
    /// a hash of each one's bitcode, with its counts (a program may be
    /// emitted in several forms before one is kept).
    static WRITTEN: RefCell<Vec<(usize, u64, Census)>> = const { RefCell::new(Vec::new()) };
}

fn fnv(bytes: &[u8]) -> u64 {
    bytes.iter().fold(0xcbf2_9ce4_8422_2325, |h, &b| {
        (h ^ u64::from(b)).wrapping_mul(0x0100_0000_01b3)
    })
}

/// Whether modules are counted (`VICTORY_SHADER_CENSUS` set).
pub fn enabled() -> bool {
    static ON: std::sync::OnceLock<bool> = std::sync::OnceLock::new();
    *ON.get_or_init(|| std::env::var_os("VICTORY_SHADER_CENSUS").is_some())
}

/// The counts of the module whose bitcode the container `dxil` holds, if
/// this thread wrote it since the last call (which forgets the others).
pub fn take(dxil: &[u8]) -> Option<Census> {
    let written = WRITTEN.with(|l| std::mem::take(&mut *l.borrow_mut()));
    let at = dxil.windows(4).position(|w| w == b"BC\xC0\xDE")?;
    written
        .into_iter()
        .rev()
        .find(|(len, hash, _)| dxil.get(at..at + len).is_some_and(|b| fnv(b) == *hash))
        .map(|w| w.2)
}

/// Counts `m`, whose written form is `bitcode`, and keeps the result for
/// [`take`].
pub(crate) fn note(m: &Module, bitcode: &[u8]) {
    if !enabled() {
        return;
    }
    let c = m.census();
    WRITTEN.with(|l| l.borrow_mut().push((bitcode.len(), fnv(bitcode), c)));
}

fn kind(name: &str) -> usize {
    let n = name.strip_prefix("dx.op.").unwrap_or(name);
    let n = n.split('.').next().unwrap_or(n);
    let k = |s: &str| KINDS.iter().position(|k| *k == s).expect("kind");
    match n {
        "unary" | "binary" | "tertiary" | "unaryBits" | "legacyF16ToF32" | "legacyF32ToF16"
        | "isSpecialFloat" | "dot2" | "dot3" | "dot4" | "quaternary" => k("math_call"),
        "bufferLoad" | "rawBufferLoad" => k("buffer_load"),
        "cbufferLoadLegacy" | "cbufferLoad" => k("cbuffer_load"),
        "bufferStore" | "rawBufferStore" | "textureStore" => k("buffer_store"),
        "atomicBinOp" | "atomicCompareExchange" => k("atomic"),
        "createHandle" | "createHandleFromBinding" | "createHandleFromHeap" => k("handle"),
        "annotateHandle" => k("annotate"),
        "getDimensions" => k("dimensions"),
        "barrier" => k("barrier"),
        _ if n.starts_with("wave") || n.starts_with("quad") => k("wave_op"),
        _ if n.starts_with("textureLoad")
            || n.starts_with("sample")
            || n.starts_with("textureGather")
            || n.starts_with("calculateLOD") =>
        {
            k("texture_read")
        }
        _ => k("io_other"),
    }
}

impl Module {
    /// The instructions [`Module::write`] writes, by kind.
    pub(crate) fn census(&self) -> Census {
        let k = |s: &str| KINDS.iter().position(|k| *k == s).expect("kind");
        let call_kind: Vec<usize> = self.funcs.iter().map(|f| kind(&f.name)).collect();
        let live = self.live();
        // The blocks `write` lays out: those reached from the entry.
        let mut seen = vec![false; self.blocks.len()];
        let mut stack = vec![0usize];
        if !self.blocks.is_empty() {
            seen[0] = true;
        }
        while let Some(b) = stack.pop() {
            for &s in super::successors(self.blocks[b].term).iter() {
                if !seen[s] {
                    seen[s] = true;
                    stack.push(s);
                }
            }
        }
        let mut c = Census::default();
        for (b, blk) in self.blocks.iter().enumerate() {
            if !seen[b] {
                continue;
            }
            c.counts[k("blocks")] += 1;
            let _: Option<Term> = blk.term;
            for &i in blk
                .phis
                .iter()
                .filter(|&&i| live[i as usize])
                .chain(&blk.body)
            {
                c.counts[0] += 1;
                let at = match &self.insts[i as usize].op {
                    Op::Bin(..) | Op::Cmp(..) => k("arith"),
                    Op::Cast(Cast::Bitcast, _) => k("bitcast"),
                    Op::Cast(..) => k("cast"),
                    Op::Select(..) => k("select"),
                    Op::Extract(..) => k("extract"),
                    Op::Phi(_) => k("phi"),
                    Op::Gep(..) => k("gep"),
                    Op::Load(_) => k("lds_load"),
                    Op::Store(..) => k("lds_store"),
                    Op::AtomicRmw(..) | Op::CmpXchg(..) => k("atomic"),
                    Op::Call(f, _) => call_kind[*f as usize],
                };
                c.counts[at] += 1;
                match self.tags.get(&i) {
                    Some(Tag::Guard) => c.counts[k("guard_select")] += 1,
                    Some(Tag::Clamp) => c.counts[k("clamp")] += 1,
                    Some(Tag::Counter) => c.counts[k("loop_counter")] += 1,
                    None => {}
                }
                let _ = V::Inst(i);
            }
        }
        c
    }
}
