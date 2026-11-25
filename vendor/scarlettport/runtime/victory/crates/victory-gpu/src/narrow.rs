//! Narrower declared shader writes (`VICTORY_NARROW_*`, all off by default).
//!
//! A range declared written by a submission sent ahead (`host.rs`, the line
//! of flights) holds back every later submission whose preparing touches it
//! on the CPU (`WHY_READ`: a refresh of another copy, a fill of upload
//! memory, a buffer copy) until that flight has landed. The declarations
//! were far wider than what shaders write:
//!
//! 1. `VICTORY_NARROW_ROOT=1`: a root view has no size, so its refresh and
//!    its declared write covered all that is left of its buffer (a view into
//!    the world's 174 MB heap: the rest of the heap). From the console
//!    program, per root parameter: whether anything stores through it at
//!    all, and where every access through it is at a constant offset (scalar
//!    loads, `s_buffer_load`, buffer instructions without a VGPR offset or
//!    index), the bytes from the view's address that the program can reach.
//!    A root UAV nothing stores through is not declared; a bounded one is
//!    refreshed and declared up to its bound only; root SRVs and constant
//!    buffers are refreshed (filled) up to their bound. Accesses that cannot
//!    be bounded leave the view as it was. Exact: the translated program
//!    reaches a root view only through the accesses the bound is taken from.
//! 2. `VICTORY_NARROW_BVH=1`: the ray-tracing builder's programs reach title
//!    memory by raw address through every numbered copy (`memory.rs`), and
//!    every dispatch declared all of them written. A program that has no
//!    `global_`/`flat_` store or atomic (and no indirect jump, and decodes)
//!    cannot write through those views at all: its copies are no longer
//!    declared. Exact. `VICTORY_NARROW_BVH=2` also declares, for a program
//!    that does store by address, only the copies its root views and root
//!    constants name: **not exact** (a pointer read out of memory may lead
//!    anywhere), an experiment to size what is left.
//! 3. `VICTORY_NARROW_VIEWS=1`: unordered access views in descriptor tables
//!    are declared over the view's own range already (`buffer_range`); this
//!    leaves out the views of a table entry the program never stores
//!    through (a shader that writes one UAV and reads another). Exact for
//!    the entries the program names; views it may reach by a computed index
//!    past them stay declared.
//!
//! Counters (the frame line, with the line of flights): per flight the
//! bytes declared, joined per copy, and the bytes the declaration would have
//! had without the switches; and the bytes each switch left out, per frame.
//!
//! shortcut: a GPU-side dirty mask written by translated shaders would bound
//! what is copied back after a flight, but not what a flight declares before
//! it runs (that is what holds later submissions back): it is not built.

use std::sync::OnceLock;
use std::sync::atomic::{AtomicU64, Ordering};

fn flag(name: &str) -> u8 {
    std::env::var(name)
        .ok()
        .and_then(|v| v.trim().parse::<u8>().ok())
        .unwrap_or(0)
}

/// `VICTORY_NARROW_ROOT=1`: root views bounded by the program's accesses.
pub(crate) fn root() -> bool {
    static ON: OnceLock<bool> = OnceLock::new();
    *ON.get_or_init(|| {
        let on = flag("VICTORY_NARROW_ROOT") >= 1;
        if on {
            tracing::info!("root views are refreshed and declared written only as far as their programs reach (VICTORY_NARROW_ROOT=1)");
        }
        on
    })
}

/// `VICTORY_NARROW_BVH`: 1 the builder's copies are declared only for
/// programs that store by address; 2 also only the copies a dispatch names.
pub(crate) fn bvh() -> u8 {
    static LEVEL: OnceLock<u8> = OnceLock::new();
    *LEVEL.get_or_init(|| {
        let level = flag("VICTORY_NARROW_BVH").min(2);
        match level {
            1 => tracing::info!("title memory by address: copies declared written only for programs that store by address (VICTORY_NARROW_BVH=1)"),
            2 => tracing::warn!(
                "title memory by address: only the copies a dispatch names are declared written (VICTORY_NARROW_BVH=2, NOT exact: stores through pointers read from memory to other copies are lost)"
            ),
            _ => {}
        }
        level
    })
}

/// `VICTORY_NARROW_VIEWS=1`: table UAVs a program never stores through are
/// not declared.
pub(crate) fn views() -> bool {
    static ON: OnceLock<bool> = OnceLock::new();
    *ON.get_or_init(|| {
        let on = flag("VICTORY_NARROW_VIEWS") >= 1;
        if on {
            tracing::info!("table UAVs their programs never store through are not declared written (VICTORY_NARROW_VIEWS=1)");
        }
        on
    })
}

/// Whether any switch needs the programs' reflection.
pub(crate) fn any() -> bool {
    root() || bvh() > 0 || views()
}

/// What a switch left out (the frame line's places in [`CUT`]).
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(crate) enum Cut {
    /// A root UAV's declared write.
    RootWrite = 0,
    /// A root view's refresh or fill.
    RootRead = 1,
    /// The builder's copies.
    Bvh = 2,
    /// A table UAV.
    View = 3,
}

/// Since the last frame line: bytes left out by [`Cut`] (not joined).
static CUT: [AtomicU64; 4] = [const { AtomicU64::new(0) }; 4];

/// Since the last frame line: flights; their declared bytes joined per
/// copy; what they would have declared without the switches.
static FLIGHTS: [AtomicU64; 3] = [const { AtomicU64::new(0) }; 3];

pub(crate) fn cut(kind: Cut, bytes: u64) {
    if bytes > 0 {
        CUT[kind as usize].fetch_add(bytes, Ordering::Relaxed);
    }
}

/// A flight sent ahead declared `declared` (copy, from, to), and the
/// switches left out `left_out` besides.
pub(crate) fn flight(declared: Vec<(usize, usize, usize)>, left_out: Vec<(usize, usize, usize)>) {
    let after = joined_bytes(declared.clone());
    let before = if left_out.is_empty() {
        after
    } else {
        joined_bytes(declared.into_iter().chain(left_out).collect())
    };
    FLIGHTS[0].fetch_add(1, Ordering::Relaxed);
    FLIGHTS[1].fetch_add(after, Ordering::Relaxed);
    FLIGHTS[2].fetch_add(before, Ordering::Relaxed);
}

/// The frame line's part (with the line of flights).
pub(crate) fn line(frames: f64) {
    let f: Vec<u64> = FLIGHTS.iter().map(|n| n.swap(0, Ordering::Relaxed)).collect();
    let c: Vec<f64> = CUT.iter().map(|n| n.swap(0, Ordering::Relaxed) as f64 / frames / MB).collect();
    let flights = f[0].max(1) as f64;
    tracing::info!(
        "declared shader writes per flight (VICTORY_NARROW_ROOT={} BVH={} VIEWS={}): {:.1} MB, {:.1} MB without narrowing, \
         {:.1} flights a frame; left out per frame: root UAV writes {:.1} MB, root view refreshes {:.1} MB, \
         builder copies {:.1} MB, table UAVs {:.1} MB",
        u8::from(root()),
        bvh(),
        u8::from(views()),
        f[1] as f64 / flights / MB,
        f[2] as f64 / flights / MB,
        f[0] as f64 / frames,
        c[0],
        c[1],
        c[2],
        c[3],
    );
}

const MB: f64 = (1u64 << 20) as f64;

/// The bytes of `(owner, from, to)` ranges, each byte of an owner once.
pub(crate) fn joined_bytes(mut ranges: Vec<(usize, usize, usize)>) -> u64 {
    ranges.retain(|r| r.1 < r.2);
    ranges.sort_unstable();
    let mut total = 0u64;
    let mut open: Option<(usize, usize, usize)> = None;
    for (owner, from, to) in ranges {
        match &mut open {
            Some(o) if o.0 == owner && from <= o.2 => o.2 = o.2.max(to),
            _ => {
                if let Some(o) = open {
                    total += (o.2 - o.1) as u64;
                }
                open = Some((owner, from, to));
            }
        }
    }
    total + open.map_or(0, |o| (o.2 - o.1) as u64)
}

/// The bytes from a root view's address to refresh or declare: `left` (all
/// that is left of its copy, or a limit) cut to `bound` where the program's
/// accesses are bounded (rounded up to 16 bytes: the host reads constant
/// buffers by 16-byte rows and writes parts of dwords as dwords).
pub(crate) fn bounded(left: u64, bound: Option<u64>) -> u64 {
    match bound {
        Some(b) => left.min(b.saturating_add(15) & !15),
        None => left,
    }
}

/// What a program does through one root parameter.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub(crate) struct RootUse {
    /// The root parameter's index in the title's root signature.
    pub(crate) slot: u8,
    /// Whether anything stores to it or does an atomic on it.
    pub(crate) written: bool,
    /// Every access (read or write) lies below this many bytes from the
    /// view's address; `None`: not known.
    pub(crate) reach: Option<u64>,
    /// Every store and atomic lies below this many bytes; `None`: not known.
    pub(crate) write_reach: Option<u64>,
}

/// What [`reflect`] finds of a shader's console programs.
#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub(crate) struct Reflect {
    /// Whether a program may store to title memory by raw address
    /// (`global_`/`flat_` stores and atomics, indirect jumps, code that
    /// does not decode).
    pub(crate) by_address: bool,
    /// Per root parameter the program reaches; `None` if its accesses could
    /// not all be traced (every root view as before).
    pub(crate) roots: Option<Vec<RootUse>>,
    /// Per entry of the program's used-resource list: whether it may be
    /// stored through; `None` if not known (all of them).
    pub(crate) table_writes: Option<Vec<bool>>,
}

impl Reflect {
    /// Conservative: everything may be written, nothing is bounded.
    pub(crate) fn unknown() -> Reflect {
        Reflect {
            by_address: true,
            roots: None,
            table_writes: None,
        }
    }

    /// What the program does through root parameter `slot`; `None`: not
    /// known (as before).
    pub(crate) fn root(&self, slot: u8) -> Option<RootUse> {
        let roots = self.roots.as_ref()?;
        Some(roots.iter().find(|r| r.slot == slot).copied().unwrap_or(RootUse {
            slot,
            written: false,
            reach: Some(0),
            write_reach: Some(0),
        }))
    }

    /// Whether the used-resource entry `index` may be stored through.
    pub(crate) fn table_written(&self, index: usize) -> bool {
        self.table_writes.as_ref().is_none_or(|w| w.get(index).copied().unwrap_or(true))
    }
}

/// Whether an instruction stores or does an atomic.
fn is_write(name: &str) -> bool {
    name.contains("store") || name.contains("atomic")
}

/// The bytes a memory instruction reads or writes at its address; `None`
/// for kinds whose size is not plain in the name (formats, image access).
pub(crate) fn access_bytes(name: &str) -> Option<u64> {
    for p in ["buffer_atomic_", "global_atomic_", "s_atomic_", "s_buffer_atomic_"] {
        if let Some(rest) = name.strip_prefix(p) {
            return Some(if rest.ends_with("_x2") { 8 } else { 4 });
        }
    }
    let rest = [
        "buffer_load_",
        "buffer_store_",
        "global_load_",
        "global_store_",
        "s_load_",
        "s_buffer_load_",
        "s_store_",
        "s_buffer_store_",
    ]
    .iter()
    .find_map(|p| name.strip_prefix(p))?;
    Some(match rest {
        "dword" => 4,
        "dwordx2" => 8,
        "dwordx3" => 12,
        "dwordx4" => 16,
        "dwordx8" => 32,
        "dwordx16" => 64,
        "ubyte" | "sbyte" | "byte" | "ubyte_d16" | "sbyte_d16" | "ubyte_d16_hi" | "sbyte_d16_hi" | "byte_d16_hi" => 1,
        "ushort" | "sshort" | "short" | "short_d16" | "short_d16_hi" => 2,
        _ => return None,
    })
}

/// The end (bytes from the view's address) of an access at constant
/// `offset` (modulo 2^32, as addresses are traced) plus `extra`, `bytes`
/// long; `None` for an offset below the address.
pub(crate) fn access_end(offset: u32, extra: u64, bytes: u64) -> Option<u64> {
    if offset as i32 >= 0 {
        Some(u64::from(offset) + extra + bytes)
    } else {
        None
    }
}

/// A constant offset operand's value (`null`, inline integers, literals).
fn constant(op: &victory_shader::gfx10::Operand) -> Option<u64> {
    use victory_shader::gfx10::{NULL, Operand};
    match *op {
        Operand::Scalar { code: NULL, .. } => Some(0),
        Operand::Const(c) if (128..=192).contains(&c) => Some(u64::from(c - 128)),
        Operand::Literal(v) if (v as i32) >= 0 => Some(u64::from(v)),
        _ => None,
    }
}

/// Bytes a root parameter's accesses reach, as they are found.
#[derive(Clone, Copy)]
struct Reach {
    written: bool,
    all: Option<u64>,
    writes: Option<u64>,
}

impl Reach {
    fn note(&mut self, write: bool, end: Option<u64>) {
        let more = |r: &mut Option<u64>| *r = r.zip(end).map(|(a, b)| a.max(b));
        more(&mut self.all);
        if write {
            self.written = true;
            more(&mut self.writes);
        }
    }
}

/// Whether a console program may store to title memory by raw address.
pub(crate) fn stores_by_address(program: &victory_shader::NativeProgram) -> bool {
    use victory_shader::gfx10::{Encoding, decode};
    let Ok(code) = decode(&program.code) else {
        return true;
    };
    code.iter().any(|i| {
        let name = i.name();
        match i.encoding {
            Encoding::Flat | Encoding::Global => name.is_none_or(|n| is_write(n) && !n.contains("scratch")),
            Encoding::Sop1 => name.is_none_or(|n| matches!(n, "s_setpc_b64" | "s_swappc_b64")),
            _ => false,
        }
    })
}

/// What a shader's console programs do through root views, raw addresses
/// and table entries. Conservative wherever a trace fails.
pub(crate) fn reflect(programs: &[victory_shader::NativeProgram]) -> Reflect {
    let by_address = programs.is_empty() || programs.iter().any(stores_by_address);
    // One program: the used-resource list the host binds is its own.
    let [native] = programs else {
        return Reflect { by_address, ..Reflect::unknown() };
    };
    match trace(native) {
        Some((roots, table_writes)) => Reflect {
            by_address,
            roots: Some(roots),
            table_writes: Some(table_writes),
        },
        None => Reflect { by_address, ..Reflect::unknown() },
    }
}

fn trace(native: &victory_shader::NativeProgram) -> Option<(Vec<RootUse>, Vec<bool>)> {
    use victory_shader::RootType;
    use victory_shader::gfx10::Fields;
    use victory_shader::resource::{Role, Source};
    let p = victory_shader::spill::build(native).ok()?;
    let st = victory_shader::structure::structure(&p);
    let res = victory_shader::resource::track(&p, &native.layout, native.stage, st.as_ref().ok()).ok()?;
    let layout = &native.layout;
    let inst = |site: (usize, usize)| p.blocks.get(site.0).and_then(|b| b.insts.get(site.1));
    let root_of = |param: usize| layout.get(param).filter(|rp| rp.root == RootType::Descriptor).map(|rp| rp.slot);
    let mut roots: std::collections::BTreeMap<u8, Reach> = Default::default();
    let mut note = |slot: u8, write: bool, end: Option<u64>| {
        roots
            .entry(slot)
            .or_insert(Reach { written: false, all: Some(0), writes: Some(0) })
            .note(write, end);
    };
    // Scalar and global accesses by address: the address holds the
    // instruction's constant offsets; a computed term is not bounded.
    for &(site, a) in &res.scalar {
        let Some(slot) = root_of(a.param) else { continue };
        let Some(i) = inst(site) else {
            note(slot, true, None);
            continue;
        };
        let name = i.name();
        let end = match a.dynamic {
            None => access_bytes(name).and_then(|n| access_end(a.offset, 0, n)),
            Some(_) => None,
        };
        note(slot, is_write(name), end);
    }
    // Descriptors: built around a root view, or loaded from a table.
    let mut table_writes = vec![false; native.used.len()];
    let mut all_tables = false;
    for d in &res.descriptors {
        if d.role != Role::Resource {
            continue;
        }
        let Some(i) = inst(d.site) else { return None };
        let name = i.name();
        let write = is_write(name);
        for leaf in d.source.leaves() {
            match leaf {
                Source::Built { address, .. } => {
                    let Some(slot) = root_of(address.param) else {
                        if write && address.param < layout.len() {
                            all_tables = true;
                        }
                        continue;
                    };
                    let end = if address.dynamic.is_some() {
                        None
                    } else {
                        match &i.fields {
                            Fields::Buffer {
                                offen: false,
                                idxen: false,
                                lds: false,
                                format: None,
                                soffset,
                                offset,
                                ..
                            } => constant(soffset).and_then(|so| {
                                access_bytes(name).and_then(|n| access_end(address.offset, u64::from(*offset) + so, n))
                            }),
                            Fields::Smem { offset, soffset, .. } if name.starts_with("s_buffer_") && *offset >= 0 => {
                                constant(soffset).and_then(|so| {
                                    access_bytes(name).and_then(|n| access_end(address.offset, *offset as u64 + so, n))
                                })
                            }
                            _ => None,
                        }
                    };
                    note(slot, write, end);
                }
                Source::Loaded { load, first } if write => {
                    // The table entry it falls in, as lowering finds it
                    // (`used_kind`); a computed offset may reach the
                    // table's other entries.
                    let Some(&(_, a)) = res.scalar.iter().find(|s| s.0 == *load) else {
                        all_tables = true;
                        continue;
                    };
                    let Some(rp) = layout.get(a.param).filter(|rp| rp.root == RootType::Table) else {
                        // A view picked from the heap by index: not a
                        // table entry the host declares by.
                        continue;
                    };
                    let on_table = |u: &victory_shader::UsedResource| u.root == RootType::Table && u.user_data == rp.user_data;
                    if a.dynamic.is_some() {
                        for (w, u) in table_writes.iter_mut().zip(&native.used) {
                            *w |= on_table(u);
                        }
                        continue;
                    }
                    let dword = a.offset / 4 + u32::from(*first);
                    let entry = native
                        .used
                        .iter()
                        .enumerate()
                        .filter(|(_, u)| on_table(u) && u.table_offset_dwords <= dword)
                        .max_by_key(|(_, u)| u.table_offset_dwords)
                        .map(|(n, _)| n);
                    match entry {
                        Some(n) => table_writes[n] = true,
                        None => {
                            for (w, u) in table_writes.iter_mut().zip(&native.used) {
                                *w |= on_table(u);
                            }
                        }
                    }
                }
                Source::Loaded { .. } => {}
                _ if write => all_tables = true,
                _ => {}
            }
        }
    }
    if all_tables {
        table_writes.iter_mut().for_each(|w| *w = true);
    }
    let roots = roots
        .into_iter()
        .map(|(slot, r)| RootUse {
            slot,
            written: r.written,
            reach: r.all,
            write_reach: if r.written { r.writes } else { Some(0) },
        })
        .collect();
    Some((roots, table_writes))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn joined_bytes_count_each_byte_of_a_copy_once() {
        // Two copies; overlapping and touching ranges of the first.
        let r = vec![(1, 0, 100), (1, 50, 150), (1, 150, 200), (2, 0, 100), (1, 300, 310), (2, 10, 20)];
        assert_eq!(joined_bytes(r), 200 + 100 + 10);
        assert_eq!(joined_bytes(vec![]), 0);
        // Empty and inverted ranges count nothing.
        assert_eq!(joined_bytes(vec![(1, 5, 5), (1, 9, 3)]), 0);
    }

    #[test]
    fn before_counts_what_was_left_out_beside_what_was_declared() {
        let declared = vec![(1, 0, 64)];
        let left_out = vec![(1, 0, 1 << 20), (2, 0, 4096)];
        let after = joined_bytes(declared.clone());
        let before = joined_bytes(declared.into_iter().chain(left_out).collect());
        assert_eq!((after, before), (64, (1 << 20) + 4096));
    }

    #[test]
    fn a_bound_only_ever_cuts_and_rounds_up_to_16_bytes() {
        assert_eq!(bounded(1 << 20, None), 1 << 20);
        assert_eq!(bounded(1 << 20, Some(4)), 16);
        assert_eq!(bounded(1 << 20, Some(32)), 32);
        assert_eq!(bounded(1 << 20, Some(33)), 48);
        // Never past what is left of the copy.
        assert_eq!(bounded(20, Some(100)), 20);
        assert_eq!(bounded(1 << 20, Some(0)), 0);
        assert_eq!(bounded(u64::MAX, Some(u64::MAX)), u64::MAX & !15);
    }

    #[test]
    fn access_sizes_come_from_the_name() {
        assert_eq!(access_bytes("buffer_load_dword"), Some(4));
        assert_eq!(access_bytes("buffer_store_dwordx3"), Some(12));
        assert_eq!(access_bytes("s_buffer_load_dwordx16"), Some(64));
        assert_eq!(access_bytes("s_load_dwordx8"), Some(32));
        assert_eq!(access_bytes("global_load_ushort"), Some(2));
        assert_eq!(access_bytes("buffer_store_byte"), Some(1));
        assert_eq!(access_bytes("buffer_load_short_d16_hi"), Some(2));
        assert_eq!(access_bytes("buffer_atomic_add"), Some(4));
        assert_eq!(access_bytes("buffer_atomic_cmpswap_x2"), Some(8));
        assert_eq!(access_bytes("global_atomic_or"), Some(4));
        // Formats and other kinds: not known, never bounded.
        assert_eq!(access_bytes("buffer_load_format_xyzw"), None);
        assert_eq!(access_bytes("tbuffer_store_format_x"), None);
        assert_eq!(access_bytes("image_store"), None);
        assert_eq!(access_bytes("buffer_load_dwordx5"), None);
    }

    #[test]
    fn writes_are_stores_and_atomics() {
        assert!(is_write("buffer_store_dword"));
        assert!(is_write("global_atomic_add"));
        assert!(is_write("s_buffer_atomic_or"));
        assert!(!is_write("buffer_load_dword"));
        assert!(!is_write("s_buffer_load_dwordx4"));
    }

    #[test]
    fn access_ends_count_from_the_view_and_refuse_offsets_below_it() {
        assert_eq!(access_end(0x10, 0, 4), Some(0x14));
        assert_eq!(access_end(0x10, 0x20 + 4, 16), Some(0x44));
        // An offset traced modulo 2^32 that is below the address.
        assert_eq!(access_end(0xFFFF_FFF0, 0, 4), None);
    }

    #[test]
    fn constant_offsets_are_null_inline_integers_and_literals() {
        use victory_shader::gfx10::{NULL, Operand};
        assert_eq!(constant(&Operand::Scalar { code: NULL, dwords: 1 }), Some(0));
        assert_eq!(constant(&Operand::Const(128)), Some(0));
        assert_eq!(constant(&Operand::Const(192)), Some(64));
        assert_eq!(constant(&Operand::Const(193)), None, "-1 is below the address");
        assert_eq!(constant(&Operand::Const(240)), None, "a float");
        assert_eq!(constant(&Operand::Literal(0x100)), Some(0x100));
        assert_eq!(constant(&Operand::Literal(0x8000_0000)), None);
        assert_eq!(constant(&Operand::Scalar { code: 4, dwords: 1 }), None, "an SGPR");
    }

    #[test]
    fn reach_takes_the_furthest_end_and_an_unknown_one_wins() {
        let mut r = Reach { written: false, all: Some(0), writes: Some(0) };
        r.note(false, Some(64));
        r.note(true, Some(16));
        assert_eq!((r.written, r.all, r.writes), (true, Some(64), Some(16)));
        r.note(false, None);
        assert_eq!((r.all, r.writes), (None, Some(16)));
        r.note(true, Some(128));
        assert_eq!((r.all, r.writes), (None, Some(128)));
        r.note(true, None);
        assert_eq!(r.writes, None);
    }

    #[test]
    fn a_root_the_program_does_not_name_is_neither_read_nor_written() {
        let r = Reflect {
            by_address: false,
            roots: Some(vec![RootUse { slot: 3, written: true, reach: None, write_reach: Some(8) }]),
            table_writes: Some(vec![false, true]),
        };
        assert_eq!(r.root(3).map(|u| (u.written, u.reach, u.write_reach)), Some((true, None, Some(8))));
        assert_eq!(r.root(5).map(|u| (u.written, u.reach)), Some((false, Some(0))));
        assert!(!r.table_written(0));
        assert!(r.table_written(1));
        // Past the list, or not known: written.
        assert!(r.table_written(7));
        let u = Reflect::unknown();
        assert!(u.root(3).is_none());
        assert!(u.table_written(0));
        assert!(u.by_address);
    }

    /// A vertex program reading its root view at s[10:11] (root parameter
    /// slot 3) through a descriptor it builds in s[0:3], then `body`.
    fn root_program(body: &[u32]) -> victory_shader::NativeProgram {
        use victory_shader::{HwStage, NativeProgram, RootParam, RootType};
        let mut words = vec![
            0x8801_FF0B,
            0x0010_0000, // s_or_b32 s1, s11, 0x100000
            0xBE80_030A, // s_mov_b32 s0, s10
            0xBE82_03C1, // s_mov_b32 s2, -1
            0xBE83_03FF,
            0x1101_4FAC, // s_mov_b32 s3, 0x11014fac
        ];
        words.extend_from_slice(body);
        words.push(0xBF81_0000); // s_endpgm
        NativeProgram {
            stage: HwStage::Vs,
            wave64: false,
            vgprs: 256,
            layout: vec![RootParam {
                root: RootType::Descriptor,
                table_offset_dwords: 0,
                sgprs: 2,
                base_register: 0,
                user_data: 10,
                slot: 3,
            }],
            used: Vec::new(),
            io: Default::default(),
            code: words.iter().flat_map(|w| w.to_le_bytes()).collect(),
        }
    }

    #[test]
    fn a_constant_buffer_read_at_constant_offsets_is_bounded() {
        // s_buffer_load_dwordx16 s[0:15], s[0:3], null: bytes 0..64.
        let r = reflect(&[root_program(&[0xF430_0000, 0xFA00_0000])]);
        assert!(!r.by_address);
        let u = r.root(3).expect("traced");
        assert_eq!((u.written, u.reach, u.write_reach), (false, Some(64), Some(0)));
        // A root parameter it does not touch: nothing.
        assert_eq!(r.root(4).map(|u| u.reach), Some(Some(0)));
    }

    #[test]
    fn a_store_at_a_constant_offset_bounds_the_declared_write() {
        // buffer_store_dword v0, off, s[0:3], null offset:0x20
        let r = reflect(&[root_program(&[0xE070_0020, 0x7D00_0000])]);
        let u = r.root(3).expect("traced");
        assert!(u.written);
        assert_eq!((u.reach, u.write_reach), (Some(0x24), Some(0x24)));
        assert!(!r.by_address, "a buffer store is no store by raw address");
    }

    #[test]
    fn a_store_at_a_vgpr_offset_is_not_bounded() {
        // buffer_store_dword v0, v1, s[0:3], null offen offset:0x20
        let r = reflect(&[root_program(&[0xE070_1020, 0x7D00_0001])]);
        let u = r.root(3).expect("traced");
        assert!(u.written);
        assert_eq!((u.reach, u.write_reach), (None, None));
    }

    #[test]
    fn a_load_at_a_vgpr_offset_leaves_writes_out_and_reads_unbounded() {
        // buffer_load_dword v0, v1, s[0:3], null offen
        let r = reflect(&[root_program(&[0xE030_1000, 0x7D00_0001])]);
        let u = r.root(3).expect("traced");
        assert!(!u.written);
        assert_eq!((u.reach, u.write_reach), (None, Some(0)));
    }

    #[test]
    fn only_global_stores_and_atomics_store_by_address() {
        let with = |body: &[u32]| {
            let mut p = root_program(&[]);
            p.code = body.iter().chain(&[0xBF81_0000]).flat_map(|w| w.to_le_bytes()).collect();
            stores_by_address(&p)
        };
        // global_load_dword v1, v[0:1], off
        assert!(!with(&[0xDC30_8000, 0x017D_0000]));
        // global_store_dword v[0:1], v2, off
        assert!(with(&[0xDC70_8000, 0x007D_0200]));
        // buffer_store_dword v0, off, s[0:3], null: no raw address
        assert!(!with(&[0xE070_0020, 0x7D00_0000]));
        // Code that does not decode: anything.
        assert!(with(&[0xFFFF_FFFF]));
    }

    #[test]
    fn two_programs_are_not_reflected() {
        let p = root_program(&[0xF430_0000, 0xFA00_0000]);
        let r = reflect(&[p.clone(), p]);
        assert!(r.roots.is_none() && r.table_writes.is_none());
    }

    #[test]
    fn programs_that_cannot_be_reflected_are_taken_as_writing_anything() {
        let r = reflect(&[]);
        assert!(r.by_address && r.roots.is_none() && r.table_writes.is_none());
    }
}
