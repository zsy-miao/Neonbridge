//! Per-program reflection of directly indexed resource heap accesses, for the
//! runtime's shadow write-back marking (agreed with the d3d12 side): which heap
//! indices a program reads and writes, named by where each index comes from.
//! Written beside the HLSL as `<hlsl-out>.heap`, one line per distinct
//! (access, source):
//!   `w root <param> <dword> <add>`  index = root constant dword + add (slots)
//!   `r const <index>`               index known at compile time
//!   `w dynamic`                     computed or loaded per lane / not traced
//!   `heap_access none`              no direct heap descriptor fetch
//! A missing file means unknown. TB_HEAP_REFLECT=0 writes no file.
use std::cell::RefCell;
use std::collections::{BTreeMap, BTreeSet, HashMap};

pub(super) fn on() -> bool {
    std::env::var("TB_HEAP_REFLECT").map_or(true, |v| v.trim() != "0")
}

thread_local! {
    static LAST: RefCell<Option<String>> = const { RefCell::new(None) };
}

/// The reflection of the last program resolved on this thread, if any.
pub fn take() -> Option<String> {
    LAST.with(|l| l.borrow_mut().take())
}

#[derive(Clone, Copy)]
pub(super) enum Src {
    /// Root parameter index and dword within it.
    Root(usize, u32),
    Const(u32),
}

/// Origins at or above this bit are block-renamed classes (heap_origins), not load sites.
const RENAMED: usize = 1usize << (usize::BITS - 1);

#[derive(Default)]
pub(super) struct HeapReflect {
    /// Source of the heap index whose `<< 5` byte offset an SGPR holds.
    pub(super) src: HashMap<u16, Src>,
    sites: BTreeMap<usize, String>,
    writes: BTreeSet<usize>,
    unknown_write: bool,
}

impl HeapReflect {
    /// A descriptor fetched at instruction `idx` from heap byte offset in `soff` + `imm`.
    pub(super) fn site(&mut self, idx: usize, soff: u16, imm: i64) {
        let add = (imm.max(0) / 32) as u32;
        let text = match self.src.get(&soff) {
            Some(Src::Root(p, d)) => format!("root {p} {d} {add}"),
            Some(Src::Const(v)) => format!("const {}", v + add),
            None => "dynamic".into(),
        };
        self.sites.insert(idx, text);
    }

    /// A memory instruction consuming a heap descriptor of load-site `origin`.
    pub(super) fn use_(&mut self, origin: Option<usize>, write: bool) {
        if !write {
            return;
        }
        match origin {
            Some(o) if o < RENAMED => { self.writes.insert(o); }
            _ => self.unknown_write = true,
        }
    }

    /// Publish this program's lines for `take`.
    pub(super) fn publish(&self) {
        let mut lines = BTreeSet::new();
        for (idx, text) in &self.sites {
            if self.unknown_write || self.writes.contains(idx) {
                lines.insert(format!("w {text}"));
            }
            lines.insert(format!("r {text}"));
        }
        let mut out = String::new();
        if lines.is_empty() {
            out.push_str("heap_access none\n");
        }
        for l in lines {
            out.push_str(&l);
            out.push('\n');
        }
        LAST.with(|l| *l.borrow_mut() = Some(out));
    }
}
