//! IR to DXIL, one machine instruction at a time (decision 0003).
//!
//! Every register dword is an `i32` (SCC an `i1`); float operations bitcast
//! their operands. Lane masks exist only as the per-lane booleans of
//! structurisation; branches and guards are built from them. A value the
//! emitter cannot express is `Missing` with a reason, and only fails the
//! program when something with an effect (an export, a branch) needs it.

use std::collections::{BTreeMap, HashMap, HashSet};

use super::UserData;
use super::bitcode::{Attr, Bin, Cast, Rmw, Term, V, pred};
use super::container::SigElement;
use super::shader::{Shader, flags};
use crate::ShaderError;
use crate::gfx10::{Dpp, Fields, NULL, Operand};
use crate::io::{Barycentric, Export, Input, Interp, Io, Target};
use crate::ir::{EXEC, Exit, Inst, Program, Reg, Value};
use crate::native::{HwStage, NativeProgram, ResourceType, RootType, StageIo, Usage, UsedResource};
use crate::resource::{Address, CODE, Resources, Role, Site, Source};
use crate::structure::{Atom, Bool, LaneExit, Structured};

type Why = &'static str;

/// How an interpolation evaluates an input channel.
#[derive(Debug, Clone, Copy)]
enum Eval {
    /// Loaded with this DXIL interpolation mode (1: constant, P0).
    Mode(u8),
    /// From the vertex values of a constant element: P0 when `p0`, plus
    /// i (P1 - P0), plus j (P2 - P0), weights as floats.
    Weights {
        p0: bool,
        i: Option<V>,
        j: Option<V>,
    },
}

/// Updates which have no native DXIL atomic instruction.
#[derive(Clone, Copy)]
enum AtomicUpdate {
    Inc,
    Dec,
    ReverseSub,
    MaskOr,
    Wrap,
    ClampSub,
}

/// An unformatted byte or halfword load. LLVM's DSInstructions.td and
/// BUFInstructions.td model the D16 destinations as tied to their old value.
fn subword_load(rest: &str) -> Option<(u32, bool, Option<u32>)> {
    let (rest, half) = if let Some(n) = rest.strip_suffix("_d16_hi") {
        (n, Some(16))
    } else if let Some(n) = rest.strip_suffix("_d16") {
        (n, Some(0))
    } else {
        (rest, None)
    };
    let (width, signed) = match rest {
        "i8" | "sbyte" => (8, true),
        "u8" | "ubyte" => (8, false),
        "i16" | "sshort" => (16, true),
        "u16" | "ushort" | "short" => (16, false),
        _ => return None,
    };
    Some((width, signed, half))
}

fn ds_integer_atomic(name: &str) -> Option<(&str, bool, u8)> {
    let (op, ty) = name.strip_prefix("ds_")?.rsplit_once('_')?;
    let op = op.strip_suffix("_rtn").unwrap_or(op);
    let wide = match ty {
        "b32" | "u32" | "i32" => false,
        "b64" | "u64" | "i64" => true,
        _ => return None,
    };
    match op {
        "add" | "sub" | "and" | "or" | "xor" | "min" | "max" | "wrxchg" | "wrxchg2"
        | "wrxchg2st64" | "rsub" | "inc" | "dec" | "mskor" | "wrap" | "cmpst" => {
            Some((op, wide, ty.as_bytes()[0]))
        }
        _ => None,
    }
}

#[derive(Debug, Clone, Copy)]
enum Val {
    I32(V),
    Bit(V),
    Missing(Why),
}

pub(crate) struct Lowered {
    pub dxil: Vec<u8>,
    /// Bit n: immediate sampler n is used for comparisons.
    pub cmp_samplers: u64,
    pub root_cbvs: Vec<u8>,
    pub root_srvs: Vec<u8>,
    pub root_uavs: Vec<u8>,
    pub stride_table: bool,
    pub user_data: Vec<UserData>,
    pub samplers: Vec<[u32; 4]>,
    /// Heap handles made with a non-uniform index.
    #[cfg(test)]
    pub nonuniform_handles: u32,
    /// Whether the program uses wave operations.
    #[cfg(test)]
    pub wave_ops: bool,
    /// Test builds: the property words of each handle annotation, in order.
    #[cfg(test)]
    pub annotations: Vec<(u32, u32)>,
}

pub(crate) struct Parts<'a> {
    pub native: &'a NativeProgram,
    pub psv: Option<&'a [u8]>,
    pub p: &'a Program,
    pub st: &'a Structured,
    pub io: &'a Io,
    pub res: &'a Resources,
    pub inputs: Vec<SigElement>,
    pub outputs: Vec<SigElement>,
    pub prims: Vec<SigElement>,
    /// Mesh: what the group id arrives multiplied by.
    pub mesh_stride: u32,
    /// The wave sizes the host runs.
    pub target: super::Target,
    /// Run the program as the console does (see [`CONSOLE_FLOW`]); `st`
    /// is then [`crate::structure::structure_console`]'s.
    pub console_cf: bool,
    /// Never ask for the console flow: the per-lane form or nothing (what
    /// a program falls back to when the console flow cannot lower it).
    pub per_lane_only: bool,
}

#[cfg(test)]
thread_local! {
    /// The opcode audit: the instruction (by code offset) whose refusal is
    /// reported even when none of its results are used.
    pub(crate) static AUDIT_OFFSET: std::cell::Cell<Option<usize>> = const { std::cell::Cell::new(None) };
}

/// What [`lower`] stops with when a program should run as the console runs
/// it: lower it again from [`crate::structure::structure_console`] with
/// [`Parts::console_cf`]. Every lane of a console wave then runs every
/// block, branches are the console's (taken wave-wide) and EXEC guards the
/// vector instructions. Two kinds of program ask for it:
///
/// * a wave64 program whose two 32-lane halves must exchange values on a
///   host without 64-lane waves (a group of one console wave): the wave-wide
///   tests come from masks both halves exchange through group-shared
///   memory, and both halves reach every exchange, so a group barrier
///   orders it;
/// * on any host, a program that reads or writes a named lane
///   (`v_readlane`, `v_writelane`) and has branches its lanes take
///   differently. Those instructions ignore EXEC on the console, and a wave
///   runs a loop while any lane does; in the per-lane form a lane that left
///   a loop (or skipped a branch) is not there to be read, and never sees
///   the scalars set inside. One host wave is the console wave here, so
///   nothing is exchanged. If the console flow cannot lower such a program
///   it keeps the per-lane form ([`Parts::per_lane_only`]).
pub(crate) const CONSOLE_FLOW: &str = "a wave64 program whose halves exchange lanes (console flow)";

enum Stop {
    /// A phi whose incoming value has no DXIL form was read: redo with it missing.
    Retry(Value, Why),
    Fail(ShaderError),
}

const UNTRACKED_SELECT: Why = "a select on an untracked mask";

pub(crate) fn lower(parts: &Parts) -> Result<Lowered, ShaderError> {
    match lower_program(parts, false) {
        Err(ShaderError::Unsupported {
            what: UNTRACKED_SELECT,
            ..
        }) if parts.native.stage == HwStage::Cs => lower_program(parts, true),
        result => result,
    }
}

fn lower_program(parts: &Parts, numeric_selects: bool) -> Result<Lowered, ShaderError> {
    let mut bad = HashMap::new();
    loop {
        match Lower::new(parts, &bad, numeric_selects).run() {
            Ok(l) => return Ok(l),
            Err(Stop::Retry(v, why)) => {
                if bad.insert(v, why).is_some() {
                    return Err(ShaderError::Unsupported {
                        offset: 0,
                        what: "a phi that does not settle",
                    });
                }
            }
            Err(Stop::Fail(e)) => return Err(e),
        }
    }
}

/// The f16 bits of inline float constant `c` (240..=248: 0.5, -0.5, 1.0,
/// -1.0, 2.0, -2.0, 4.0, -4.0, 1/(2 pi)).
fn inline_f16(c: u8) -> u32 {
    [
        0x3800, 0xB800, 0x3C00, 0xBC00, 0x4000, 0xC000, 0x4400, 0xC400, 0x3118,
    ][usize::from(c - 240)]
}

/// Private memory per lane when the code does not show how far its
/// addresses reach (an index loaded or carried around a loop).
// shortcut: the console's size is set at pipeline creation (the program
// metadata read so far does not hold it); accesses past this read 0 and
// drop writes, as accesses past the console's size do.
pub(super) const SCRATCH_CAP: u32 = 4 << 10;
/// The most private memory per lane, the D3D11 limit of indexable
/// temporaries (4096 four-dword registers).
// shortcut: accesses past it read 0 and drop writes.
const SCRATCH_MAX: u32 = 64 << 10;

/// Bytes a `scratch_*` opcode moves (per lane), from the name after
/// `scratch_load_` or `scratch_store_`.
fn scratch_width(rest: &str) -> Option<u32> {
    Some(match rest.split('_').next()? {
        "dword" => 4,
        "dwordx2" => 8,
        "dwordx3" => 12,
        "dwordx4" => 16,
        "byte" | "ubyte" | "sbyte" => 1,
        "short" | "ushort" | "sshort" => 2,
        _ => return None,
    })
}

/// Private memory per lane the program needs: past the highest byte its
/// scratch accesses can reach where the code computing their addresses
/// bounds them, else [`SCRATCH_CAP`]; at most [`SCRATCH_MAX`].
fn scratch_bytes(p: &Program) -> u32 {
    let mut defs = HashMap::new();
    for i in p.blocks.iter().flat_map(|b| &b.insts) {
        for &(_, v) in &i.writes {
            defs.insert(v, i);
        }
    }
    let mut memo = HashMap::new();
    let mut end = 0u64;
    for i in p.blocks.iter().flat_map(|b| &b.insts) {
        let name = i.name();
        let Fields::Flat {
            addr,
            saddr,
            offset,
            ..
        } = &i.fields
        else {
            continue;
        };
        let rest = name
            .strip_prefix("scratch_load_")
            .or_else(|| name.strip_prefix("scratch_store_"));
        let Some(width) = rest.and_then(scratch_width) else {
            continue;
        };
        let base = match (addr, saddr) {
            (Some(o), _) | (None, Some(o)) => operand_bound(&defs, &mut memo, i, o, 0),
            (None, None) => Some(0),
        };
        // A negative offset only lowers the address (or wraps it out of
        // range, checked when the program runs).
        let off = u64::from((*offset).max(0).unsigned_abs());
        end = end.max(base.map_or(u64::from(SCRATCH_CAP), |b| b + off + u64::from(width)));
    }
    end.min(u64::from(SCRATCH_MAX)) as u32
}

/// An upper bound of operand `o` of `inst` as an unsigned number.
fn operand_bound(
    defs: &HashMap<Value, &Inst>,
    memo: &mut HashMap<Value, Option<u64>>,
    inst: &Inst,
    o: &Operand,
    depth: u8,
) -> Option<u64> {
    match *o {
        Operand::Scalar { code: NULL, .. } => Some(0),
        Operand::Scalar { code, .. } => value_bound(defs, memo, inst.read(Reg::S(code))?, depth),
        Operand::Vector { reg, .. } => value_bound(defs, memo, inst.read(Reg::V(reg))?, depth),
        Operand::Const(c) => crate::resource::inline(c).map(u64::from),
        Operand::Literal(x) => Some(u64::from(x)),
        Operand::Attr { .. } => None,
    }
}

/// An upper bound of 32-bit value `v` as an unsigned number, where the
/// instructions computing it show one (constants combined by masks, shifts,
/// sums, products, selects); `None` where it depends on anything else
/// (inputs, memory, a phi) or may wrap.
fn value_bound(
    defs: &HashMap<Value, &Inst>,
    memo: &mut HashMap<Value, Option<u64>>,
    v: Value,
    depth: u8,
) -> Option<u64> {
    if let Some(&b) = memo.get(&v) {
        return b;
    }
    let inst = *defs.get(&v)?;
    if depth > 32 {
        return None;
    }
    let (dst, src) = match &inst.fields {
        Fields::Sop {
            dst: Some(Operand::Scalar { code, .. }),
            src,
            imm: None,
        } => (Reg::S(*code), src),
        Fields::Valu(x)
            if x.sdwa.is_none()
                && x.dpp.is_none()
                && x.neg | x.abs | x.opsel == 0
                && !x.clamp
                && x.omod == 0 =>
        {
            match x.dst {
                Some(Operand::Vector { reg, .. }) => (Reg::V(reg), &x.src),
                _ => return None,
            }
        }
        _ => return None,
    };
    if inst.write(dst) != Some(v) {
        return None;
    }
    let mut arg =
        |k: usize| -> Option<u64> { operand_bound(defs, memo, inst, src.get(k)?, depth + 1) };
    // The smallest all-ones number at least `x` (bounds `|` and `^`).
    let ones = |x: u64| {
        if x == 0 {
            0
        } else {
            u64::MAX >> x.leading_zeros()
        }
    };
    let u24 = |x: Option<u64>| x.map(|x| x.min(0xFF_FFFF));
    let shl = |x: Option<u64>, n: Option<u64>| -> Option<u64> {
        x?.checked_shl(u32::try_from(n.filter(|&n| n < 32)?).ok()?)
    };
    let b = match inst.name() {
        "v_mov_b32" | "s_mov_b32" => arg(0),
        "v_and_b32" | "s_and_b32" => match (arg(0), arg(1)) {
            (Some(x), Some(y)) => Some(x.min(y)),
            (x, y) => x.or(y),
        },
        "v_or_b32" | "s_or_b32" | "v_xor_b32" | "s_xor_b32" => Some(ones(arg(0)?.max(arg(1)?))),
        "v_lshlrev_b32" => {
            let n = arg(0);
            shl(arg(1), n)
        }
        "s_lshl_b32" => {
            let x = arg(0);
            shl(x, arg(1))
        }
        "v_lshrrev_b32" => arg(1),
        "s_lshr_b32" => arg(0),
        "v_add_nc_u32" | "v_add_u32" | "v_add_co_u32" | "s_add_u32" | "s_add_i32" => {
            Some(arg(0)? + arg(1)?)
        }
        "v_add3_u32" => Some(arg(0)? + arg(1)? + arg(2)?),
        "v_mul_u32_u24" => Some(u24(arg(0))? * u24(arg(1))?),
        "v_mul_lo_u32" | "s_mul_i32" => arg(0)?.checked_mul(arg(1)?),
        "v_mad_u32_u24" => Some(u24(arg(0))? * u24(arg(1))? + arg(2)?),
        "v_lshl_add_u32" | "v_lshl_or_b32" => {
            let x = arg(0);
            let n = arg(1);
            Some(shl(x, n)? + arg(2)?)
        }
        "v_add_lshl_u32" => {
            let x = Some(arg(0)? + arg(1)?);
            shl(x, arg(2))
        }
        "v_and_or_b32" => {
            let x = match (arg(0), arg(1)) {
                (Some(x), Some(y)) => x.min(y),
                (x, y) => x.or(y)?,
            };
            Some(x + arg(2)?)
        }
        "v_bfe_u32" => match arg(2) {
            Some(w) if w < 32 => Some((1 << w) - 1),
            _ => arg(0),
        },
        "v_cndmask_b32" | "v_max_u32" | "s_max_u32" | "s_cselect_b32" => Some(arg(0)?.max(arg(1)?)),
        "v_min_u32" | "s_min_u32" => match (arg(0), arg(1)) {
            (Some(x), Some(y)) => Some(x.min(y)),
            (x, y) => x.or(y),
        },
        _ => None,
    }
    .filter(|&b| b <= u64::from(u32::MAX));
    memo.insert(v, b);
    b
}

const TWO_PI: f32 = std::f32::consts::TAU;
/// `s_sendmsg` id of a mesh or NGG stage's output allocation request.
const GS_ALLOC_REQ: u16 = 9;

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum Numeric {
    Unorm,
    Snorm,
    Uscaled,
    Sscaled,
    Uint,
    Sint,
    Float,
}

/// A GFX10 buffer format (the unified format of typed buffer loads): the
/// channel widths from the lowest bits up, and how the channels convert.
/// Formats are numbered in families (`8`, `16`, `8_8`, ...: AMD names
/// list channels from the highest bits), each with its numeric formats in
/// the order unorm, snorm, uscaled, sscaled, uint, sint, float.
fn buffer_format(format: u8) -> Option<(&'static [u32], Numeric)> {
    use Numeric::*;
    const NORM: &[Numeric] = &[Unorm, Snorm, Uscaled, Sscaled, Uint, Sint];
    const ALL: &[Numeric] = &[Unorm, Snorm, Uscaled, Sscaled, Uint, Sint, Float];
    const WIDE: &[Numeric] = &[Uint, Sint, Float];
    // Numbers and order from LLVM's `UfmtGFX10` (`SIDefines.h`); the GFX10
    // buffer formats have no 5_9_9_9. Channel order from Mesa
    // (`ac_formats.c`): R11G11B10_FLOAT is 10_11_11, and 10_10_10_2 has
    // its 2-bit channel in the lowest bits.
    const FAMILIES: [(&[u32], &[Numeric]); 14] = [
        (&[8], NORM),
        (&[16], ALL),
        (&[8, 8], NORM),
        (&[32], WIDE),
        (&[16, 16], ALL),
        (&[11, 11, 10], ALL),     // 10_11_11
        (&[10, 11, 11], ALL),     // 11_11_10
        (&[2, 10, 10, 10], NORM), // 10_10_10_2
        (&[10, 10, 10, 2], NORM), // 2_10_10_10
        (&[8, 8, 8, 8], NORM),
        (&[32, 32], WIDE),
        (&[16, 16, 16, 16], ALL),
        (&[32, 32, 32], WIDE),
        (&[32, 32, 32, 32], WIDE),
    ];
    let mut n = usize::from(format.checked_sub(1)?);
    for (bits, numerics) in FAMILIES {
        if n < numerics.len() {
            return Some((bits, numerics[n]));
        }
        n -= numerics.len();
    }
    None
}

/// How far a float channel of `width` bits (16, or the unsigned 11- and
/// 10-bit floats of the packed formats) shifts up to be a half:
/// the small floats are a half's exponent (5 bits, biased 15) and the top
/// of its mantissa, without the sign, so `legacyF16ToF32` widens them
/// exactly, denormals, infinities and NaNs included.
fn half_shift(width: u32) -> u32 {
    debug_assert!(matches!(width, 10 | 11 | 16), "{width}-bit float");
    if width == 16 { 0 } else { 15 - width }
}

struct Lower<'a> {
    s: Shader,
    x: &'a Parts<'a>,
    /// SGPR spills to VGPR lanes and their reloads that nothing reads any
    /// more (see [`crate::sgpr_spill`]): not lowered.
    dead: HashSet<Site>,
    bad: &'a HashMap<Value, Why>,
    lane_fail: HashMap<Value, Why>,
    vals: HashMap<Value, Val>,
    floats: HashMap<Value, V>,
    /// DXIL block per IR block (unreachable: none).
    block: Vec<Option<usize>>,
    cur: usize,
    bools: HashMap<(usize, Bool), V>,
    lane_bits: HashMap<Value, V>,
    /// IR phi location by value.
    phi_at: HashMap<Value, (usize, usize)>,
    mask_phis: HashMap<Value, V>,
    pending_masks: Vec<(V, usize, usize)>,
    data_phis: Vec<(V, usize, usize)>,
    read_phis: HashSet<Value>,
    params: HashMap<Value, (usize, u32)>,
    user_data: Option<V>,
    user_reads: BTreeMap<u32, UserData>,
    samplers: Vec<[u32; 4]>,
    descriptors: HashMap<(Site, bool), &'a Source>,
    scalar: HashMap<Site, Address>,
    exports: HashMap<Site, &'a Export>,
    interps: HashMap<Site, &'a Interp>,
    handles: HashMap<(usize, Site, u8, bool, (u32, u32)), V>,
    /// Blocks splitting the edge from one IR block to another.
    edges: HashMap<(usize, usize), usize>,
    /// Per IR block: a `done` export is behind every lane entering it.
    exported: Vec<bool>,
    /// Reverse-postorder index per IR block.
    rpo: Vec<usize>,
    /// DXIL block each IR block ends in (guarded stores split blocks).
    tail: Vec<Option<usize>>,
    /// The program writes UAVs: barriers fence them too.
    writes_uavs: bool,
    wide_lds: bool,
    /// src0 of the current instruction as DPP fetched it.
    src0: Option<V>,
    /// Compute code that reads other lanes: thread ids follow the host
    /// lanes, so lane n is console lane n.
    console_lanes: bool,
    /// Retry a compute shader with numeric selects only when the normal
    /// translation needs one. Unused selects need no fixed wave size.
    numeric_selects: bool,
    /// A console wave64 compute program run as two 32-lane host waves
    /// (each lane's reads stay in its half; see `Lower::new`).
    split: bool,
    /// Split, and run as the console runs it ([`CONSOLE_FLOW`]).
    console_cf: bool,
    /// Split without [`Self::console_cf`] cannot run it, the console flow can.
    wants_console_flow: bool,
    /// The console thread's flat id in the group (relabelled; see
    /// [`Self::thread_id`]).
    console_flat: Option<V>,
    /// Group-shared slots the two halves exchange a dword through.
    xchg: Option<V>,
    /// Mesh: the thread index in the group (made in the entry block).
    flat: Option<V>,
    /// Mesh: IR blocks whose `s_barrier` is left out.
    no_barrier: HashSet<usize>,
    /// Vertex and mesh outputs holding 16-bit values: row, whether two per
    /// dword, and the row packed after it.
    half_outputs: HashMap<u8, (bool, Option<u8>)>,
    /// Pixel inputs holding 16-bit values: row, whether two per dword, and
    /// the row packed after it.
    half_inputs: HashMap<u8, (bool, Option<u8>)>,
    /// Multi-element 16-bit rows whose trailing element overflows a 4-dword
    /// `pack2` attribute and spills into a free dword of a later `pack2`
    /// attribute: `(attr_row, half_index) -> (sig_row, col)`.
    half_input_spills: HashMap<(u8, u8), (u8, u8)>,
    /// How lane masks become data.
    mask_data: MaskData,
    /// The instruction being lowered may read lane masks as data.
    mask_ok: bool,
    /// Ballots of masks read as data, by low dword: (low, high).
    ballots: HashMap<Value, (V, V)>,
    /// SCC of a scalar mask operation ("some lane is set"), with the low
    /// dword of the mask it tests: read as data, the ballot is not zero.
    mask_scc: HashMap<Value, Value>,
    /// Lane masks read as numbers (see [`mask_numbers`]).
    mask_numbers: HashSet<Value>,
    /// Masks whose ballot is being built: a mask whose lane bit needs its
    /// own value as a number (an opaque mask keyed by a dword it shares
    /// with another) would otherwise recurse until the stack overflows.
    balloting: HashSet<Value>,
    /// Block writing each value.
    def_block: HashMap<Value, usize>,
    /// IR block being lowered.
    cur_ir: usize,
    /// Whole-wave sections by block and start index.
    sections: HashMap<usize, HashMap<usize, super::wave::Section>>,
    /// Values written in a section that code outside it reads.
    section_out: HashSet<Value>,
    /// Values of whole-wave sections holding one lane's, the one read after.
    one_lane: HashSet<Value>,
    /// Memory by address is written too, for other groups to read.
    coherent: Option<bool>,
    /// Threads of different groups take turns through atomics (a tree built
    /// from its leaves up): what they wrote before one must be seen after
    /// it. The console's program waits for its writes there.
    ordered: Option<bool>,
    /// Views of buffers accessed by address, by instruction.
    made: HashMap<Site, V>,
    /// The program's constant table (the code from the first byte read
    /// through `s_getpc_b64` on): global, first byte, dwords.
    code_table: Option<(V, u32, u32)>,
    /// Private (scratch) memory: the lane's array and its dwords.
    scratch: Option<(V, u32)>,
    /// Loop headers' iteration counters (see [`LOOP_LIMIT`]): the phi by
    /// IR block, and its incoming (value, DXIL block) pairs.
    counters: HashMap<usize, (V, Vec<(V, usize)>)>,
    /// Loops that lanes leave through one block, by that block: the header
    /// and the scalar values the loop writes that code after it reads (see
    /// [`Lower::leave_loops`]).
    loop_exits: HashMap<usize, Vec<(usize, Vec<Value>)>>,
    /// Loop bodies (IR blocks) by header.
    loop_bodies: HashMap<usize, HashSet<usize>>,
    /// Scalar values as the wave's last iteration of a divergent loop left
    /// them, with the loop's header: what code after the loop reads.
    finals: HashMap<Value, (Val, usize)>,
    /// The block a loop stopped by [`LOOP_LIMIT`] leaves the program from.
    abort: Option<usize>,
    /// Per-lane descriptor joins being lowered: IR block and the heap index
    /// phi there (what [`Source::Carried`] reads).
    lane_paths: Vec<(usize, V)>,
    /// Heap index phis of per-lane joins with predecessors lowered later.
    pending_paths: Vec<PendingPath>,
}

/// A heap index phi of a [`Source::Paths`], filled once every predecessor is
/// lowered.
struct PendingPath {
    phi: V,
    block: usize,
    sampler: bool,
    /// [`Lower::lane_paths`] where it was made, itself included.
    outer: Vec<(usize, V)>,
    /// Per predecessor: the index when it was lowered already, else its
    /// source.
    incoming: Vec<(usize, Result<V, Source>)>,
}

/// Iterations after which a loop is left (per entry into it), unless
/// [`super::loop_limit`] says otherwise. The console runs a loop as long as
/// its data says; data the host has not mirrored yet (a null view reads
/// zeros) can make that four billion iterations, which hangs the host GPU
/// until the driver resets it. A lane that reaches the limit leaves the
/// program as killed (a pixel discards).
// shortcut: a genuine loop of more iterations than this is cut short too.
pub(super) const LOOP_LIMIT: u32 = 1 << 20;

/// Lane masks read as numbers (`s_bcnt1`, `s_ff1`, `v_mbcnt`, scalar
/// compares, bit tests) are ballots of the lanes' booleans, taken where the
/// mask is written: structurisation's `active` masks hold only lanes that
/// reach that block, so the ballot misses none.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum MaskData {
    /// Compute: host lane n is console lane n (the program declares the
    /// console's wave size), so ballots are the console's masks bit for bit
    /// and any use is exact.
    Console,
    /// Pixel and vertex programs: ballots over the host's wave, whatever its
    /// size. Only uses that mean the same on any wave are allowed: counts,
    /// the first lane, the count below a lane, equality (the HLSL wave
    /// operations the console compiler expands). Lane numbers stay host
    /// lanes throughout (`v_mbcnt`, `v_readlane` at a computed lane).
    Host,
    /// Mesh programs: their lanes and waves (LDS per console wave) are the
    /// console's but no host wave size can be declared.
    None,
}

/// What a value holding a lane mask read as a number stops with.
const LANE_MASK: Why = "a lane mask read as data";

/// Scalar set operations, which keep a lane mask a lane mask: their result
/// as a number is the same operation on their sources' numbers.
fn mask_set_op(name: &str) -> bool {
    let Some(op) = name
        .strip_prefix("s_")
        .and_then(|n| n.strip_suffix("_b32").or_else(|| n.strip_suffix("_b64")))
    else {
        return false;
    };
    matches!(
        op,
        "and" | "or" | "xor" | "andn2" | "orn2" | "nand" | "nor" | "xnor" | "not" | "mov"
    )
}

/// Samples whose level comes from derivatives across each quad of lanes.
fn implicit_derivatives(name: &str) -> bool {
    sample_form(name).is_some_and(|f| !f.gather && !f.c && !f.d && !f.l && !f.lz)
}

/// The lane masks whose value as a number the program reads: those an
/// instruction reads as data ([`reads_mask_as_data`]) or a join that is no
/// mask takes, and the masks the set operations and joins writing them
/// read in turn. Only these get a number where a set operation writes them
/// (the others would take ballots nothing reads).
fn mask_numbers(st: &Structured, p: &crate::ir::Program) -> HashSet<Value> {
    let is_mask = |v: Value| st.masks.contains_key(&v) || st.highs.contains_key(&v);
    let mut out: HashSet<Value> = HashSet::new();
    for b in &p.blocks {
        for phi in b.phis.iter().filter(|phi| !is_mask(phi.value)) {
            out.extend(phi.args.iter().copied().filter(|&a| is_mask(a)));
        }
        for i in b.insts.iter().filter(|i| reads_mask_as_data(st, i)) {
            let src: &[Operand] = match &i.fields {
                Fields::Sop { src, .. } => src,
                Fields::Valu(v) => &v.src,
                _ => &[],
            };
            for op in src {
                if let Operand::Scalar { code, dwords } = *op
                    && code != NULL
                {
                    let read = (0..dwords).filter_map(|k| i.read(Reg::S(code + k)));
                    out.extend(read.filter(|&v| is_mask(v)));
                }
            }
        }
    }
    loop {
        let before = out.len();
        for b in &p.blocks {
            for phi in &b.phis {
                if out.contains(&phi.value) {
                    out.extend(phi.args.iter().copied().filter(|&a| is_mask(a)));
                }
            }
            for i in &b.insts {
                if matches!(i.fields, Fields::Sop { .. })
                    && mask_set_op(i.name())
                    && i.writes.iter().any(|w| out.contains(&w.1))
                {
                    let read = i.reads.iter().filter(|r| matches!(r.0, Reg::S(_)));
                    out.extend(read.map(|r| r.1).filter(|&v| is_mask(v)));
                }
            }
        }
        if out.len() == before {
            return out;
        }
    }
}

/// Instructions that read a lane mask (not EXEC implicitly, not a lane's
/// bit) as a number.
fn reads_mask_as_data(st: &Structured, i: &Inst) -> bool {
    let is_mask = |v: Value| st.masks.contains_key(&v) || st.highs.contains_key(&v);
    let ops: Vec<&Operand> = match &i.fields {
        // A mask operation (its result a mask) is no data use.
        Fields::Sop { .. }
            if i.writes
                .iter()
                .any(|&(r, w)| matches!(r, Reg::S(_)) && is_mask(w)) =>
        {
            return false;
        }
        Fields::Sop { src, .. } => src.iter().collect(),
        Fields::Valu(v)
            if !matches!(i.name(), "v_cndmask_b32") && !i.name().contains("_co_ci_") =>
        {
            v.src.iter().collect()
        }
        _ => return false,
    };
    ops.into_iter().any(|op| match *op {
        Operand::Scalar { code, .. } if code != NULL => i.read(Reg::S(code)).is_some_and(is_mask),
        _ => false,
    })
}

/// SCC of each scalar operation writing a tracked lane mask, with the
/// mask's low dword (EXEC's new value for the `saveexec` forms).
fn mask_sccs(st: &Structured, p: &crate::ir::Program) -> HashMap<Value, Value> {
    let mut out = HashMap::new();
    for inst in p.blocks.iter().flat_map(|b| &b.insts) {
        let Fields::Sop { dst, .. } = &inst.fields else {
            continue;
        };
        let Some(scc) = inst.write(Reg::Scc) else {
            continue;
        };
        let lo = if inst.name().contains("saveexec") {
            inst.write(Reg::S(EXEC))
        } else {
            match *dst {
                Some(Operand::Scalar { code, .. }) => inst.write(Reg::S(code)),
                _ => None,
            }
        };
        if let Some(lo) = lo.filter(|lo| st.masks.contains_key(lo)) {
            out.insert(scc, lo);
        }
    }
    out
}

/// Whether `inst` gives each lane what it would under any EXEC: scalar
/// code and loads that do not touch EXEC, and vector code that reads no
/// other lane (no DPP, permutes or lane reads and writes).
fn lane_local(inst: &Inst) -> bool {
    // EXEC as an operand, or EXECZ.
    let exec = |o: &Operand| {
        matches!(*o, Operand::Scalar { code, .. } if code >= EXEC) || *o == Operand::Const(252)
    };
    let name = inst.name();
    match &inst.fields {
        Fields::Sop { .. } => !inst
            .reads
            .iter()
            .chain(&inst.writes)
            .any(|&(r, _)| r == Reg::S(EXEC) || r == Reg::S(EXEC + 1)),
        Fields::Smem { .. } => name.starts_with("s_load_") || name.starts_with("s_buffer_load_"),
        Fields::Valu(v) => {
            v.dpp.is_none()
                && !name.starts_with("v_interp_")
                && !matches!(
                    name,
                    "v_readlane_b32"
                        | "v_readfirstlane_b32"
                        | "v_writelane_b32"
                        | "v_permlane16_b32"
                        | "v_permlanex16_b32"
                )
                && !v.src.iter().chain(&v.dst).chain(&v.carry).any(exec)
        }
        _ => false,
    }
}

/// Dwords a scalar load reads, by the suffix of its name after
/// `s_load_dword` or `s_buffer_load_dword`.
fn smem_dwords(suffix: &str) -> Option<u32> {
    match suffix {
        "" => Some(1),
        "x2" => Some(2),
        "x4" => Some(4),
        "x8" => Some(8),
        "x16" => Some(16),
        _ => None,
    }
}

/// A select that needs the actual scalar mask words and console lane index.
fn numeric_select(st: &Structured, inst: &Inst) -> bool {
    inst.name() == "v_cndmask_b32"
        && matches!(&inst.fields, Fields::Valu(v)
            if matches!(v.src.get(2), Some(Operand::Scalar { code, .. }) if *code != NULL))
        && inst
            .writes
            .first()
            .is_some_and(|&(_, v)| !st.select_masks.contains_key(&v))
}

/// Where half `h` (dword h / 2, the high half when odd) of a parameter
/// packing 16-bit rows `a` and `b` two per dword comes from: a row and
/// channel of `elements`, none for padding. Channel c of `a` is half c;
/// `b` starts at the dword after `a`'s last channel.
fn packed_half(elements: &[SigElement], a: u8, b: Option<u8>, h: u8) -> Option<(u8, u8)> {
    let extent = |r: u8| {
        elements
            .iter()
            .filter(|e| e.row == Some(u32::from(r)))
            .map(|e| e.col + e.cols)
            .max()
    };
    let na = extent(a)?;
    let a_halves = na.div_ceil(2) * 2;
    if h < a_halves {
        return (h < na).then_some((a, h));
    }
    let b = b?;
    let c = h - a_halves;
    (c < extent(b)?).then_some((b, c))
}

/// Vertex or mesh outputs of 16-bit values (see [`Lower::half_outputs`]):
/// a packed row's partner is the next 16-bit row of the signature that no
/// output entry names, taken in row order.
fn half_outputs(io: &StageIo, outputs: &[SigElement]) -> HashMap<u8, (bool, Option<u8>)> {
    let entries = match io {
        StageIo::Vs { outputs, .. } => outputs,
        StageIo::Ms(ms) => &ms.outputs,
        _ => return HashMap::new(),
    };
    let per_vertex = || entries.iter().filter(|o| o.param & 0x80 == 0);
    let mut free: Vec<u8> = outputs
        .iter()
        .filter(|e| e.half && e.kind == 0)
        .filter_map(|e| u8::try_from(e.row?).ok())
        .filter(|&r| !per_vertex().any(|o| o.row == r))
        .collect();
    free.sort_unstable();
    free.dedup();
    let mut halves: Vec<_> = per_vertex().filter(|o| o.fp16).collect();
    halves.sort_by_key(|o| o.row);
    halves
        .into_iter()
        .map(|o| {
            let b = o
                .pack2
                .then(|| free.iter().position(|&r| r > o.row))
                .flatten();
            (o.row, (o.pack2, b.map(|i| free.remove(i))))
        })
        .collect()
}

/// What a sample or gather opcode adds to the plain form, by its name:
/// comparison (`_c`), explicit derivatives (`_d`, coarse `_cd`, 16-bit
/// `_g16`), a level (`_l`) or level 0 (`_lz`), a bias (`_b`), a level
/// clamp (`_cl`), a texel offset (`_o`).
#[derive(Debug, Clone, Copy, Default)]
struct SampleForm {
    gather: bool,
    c: bool,
    d: bool,
    g16: bool,
    l: bool,
    lz: bool,
    b: bool,
    cl: bool,
    o: bool,
}

fn sample_form(name: &str) -> Option<SampleForm> {
    let (gather, rest) = match name.strip_prefix("image_sample") {
        Some(rest) => (false, rest),
        None => (true, name.strip_prefix("image_gather4")?),
    };
    let mut f = SampleForm {
        gather,
        ..SampleForm::default()
    };
    for token in rest.split('_').skip(1) {
        let flag = match token {
            "c" => &mut f.c,
            "d" | "cd" => &mut f.d,
            "g16" => &mut f.g16,
            "l" => &mut f.l,
            "lz" => &mut f.lz,
            "b" => &mut f.b,
            "cl" => &mut f.cl,
            "o" => &mut f.o,
            _ => return None,
        };
        *flag = true;
    }
    Some(f)
}

/// Pixel inputs of 16-bit values (see [`Lower::half_inputs`]): a packed row's
/// partner is `row2` when distinct from `row`, else the next unlisted 16-bit
/// row of the signature before the next `fp16` input row, taken in row order.
fn half_inputs(io: &StageIo, inputs: &[SigElement]) -> HashMap<u8, (bool, Option<u8>)> {
    let StageIo::Ps {
        inputs: entries, ..
    } = io
    else {
        return HashMap::new();
    };
    // Keep the first fp16 entry per row (matching the previous `.find()`
    // behaviour) so duplicate rows neither overwrite earlier ones nor consume
    // an extra free partner row.
    let mut seen = HashSet::new();
    let mut halves: Vec<_> = entries
        .iter()
        .filter(|i| i.fp16 && seen.insert(i.row))
        .collect();
    halves.sort_by_key(|i| i.row);
    // In the console's PsInput table, `row2` repeats `row` when no distinct
    // partner row is named; exclude both listed rows and explicitly claimed
    // partners (`row2 != row`) from the fallback pool.
    let mut free: Vec<u8> = inputs
        .iter()
        .filter(|e| e.half && e.kind == 0)
        .filter_map(|e| u8::try_from(e.row?).ok())
        .filter(|&r| {
            !entries.iter().any(|i| i.row == r)
                && !halves
                    .iter()
                    .any(|i| i.pack2 && i.row2 != i.row && i.row2 == r)
        })
        .collect();
    free.sort_unstable();
    free.dedup();
    let mut out = HashMap::with_capacity(halves.len());
    for (idx, i) in halves.iter().enumerate() {
        let next_fp16 = halves.get(idx + 1).map_or(u8::MAX, |n| n.row);
        // Only take a fallback partner before the next fp16 input row so an
        // earlier unpaired input cannot steal a later fp16 input's partner.
        let b = if !i.pack2 {
            None
        } else if i.row2 != i.row {
            Some(i.row2)
        } else {
            free.iter()
                .position(|&r| r > i.row && r < next_fp16)
                .map(|p| free.remove(p))
        };
        out.insert(i.row, (i.pack2, b));
    }
    out
}

/// Elements from multi-element 16-bit signature rows that overflow a 4-dword
/// `pack2` attribute (because each element starts on a whole dword) and spill
/// into free dwords of a subsequent `pack2` attribute.
fn half_input_spills(
    half_inputs: &HashMap<u8, (bool, Option<u8>)>,
    inputs: &[SigElement],
) -> HashMap<(u8, u8), (u8, u8)> {
    let row_elems = |r: u8| -> Vec<&SigElement> {
        let mut v: Vec<_> = inputs
            .iter()
            .filter(|e| e.half && e.row == Some(u32::from(r)))
            .collect();
        v.sort_by_key(|e| e.col);
        v
    };
    let extent = |r: u8| -> u8 {
        inputs
            .iter()
            .filter(|e| e.row == Some(u32::from(r)))
            .map(|e| e.col + e.cols)
            .max()
            .unwrap_or(0)
    };
    let mut rows: Vec<(u8, Option<u8>)> = half_inputs
        .iter()
        .filter_map(|(&a, &(pack2, b))| pack2.then_some((a, b)))
        .collect();
    rows.sort_by_key(|&(a, _)| a);
    let mut pending: Vec<(u8, u8, u8)> = Vec::new();
    let mut out = HashMap::new();
    for (a, b) in rows {
        let mut elems_a = row_elems(a);
        let dwords_b: u8 = b
            .map(|r| row_elems(r).iter().map(|e| e.cols.div_ceil(2)).sum())
            .unwrap_or(0);
        let mut dwords_a: u8 = elems_a.iter().map(|e| e.cols.div_ceil(2)).sum();
        while elems_a.len() > 1 && dwords_a + dwords_b > 4 {
            let spill = elems_a.pop().unwrap();
            dwords_a -= spill.cols.div_ceil(2);
            pending.push((a, spill.col, spill.cols));
        }
        let mut used = extent(a).div_ceil(2) + b.map_or(0, |r| extent(r).div_ceil(2));
        if used < 4 && !pending.is_empty() {
            let mut idx = 0;
            while idx < pending.len() && used < 4 {
                let (sr, sc, scols) = pending[idx];
                let need = scols.div_ceil(2);
                if used + need <= 4 {
                    for k in 0..scols {
                        out.insert((a, used * 2 + k), (sr, sc + k));
                    }
                    used += need;
                    pending.remove(idx);
                } else {
                    idx += 1;
                }
            }
        }
    }
    out
}

fn unsupported(offset: usize, what: Why) -> Stop {
    Stop::Fail(ShaderError::Unsupported { offset, what })
}

/// A byte or halfword LDS access: `ds_read_{u,i}{8,16}[_d16[_hi]]` and
/// `ds_write_b{8,16}[_d16_hi]`.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
struct SubDword {
    write: bool,
    bits: u32,
    /// Reads: sign-extended.
    signed: bool,
    /// Reads: the result is 16 bits wide and goes into one half of the VGPR,
    /// the other half kept (`Some(true)`: the high half). Writes: the data
    /// comes from the high half (`Some(true)`).
    d16: Option<bool>,
}

impl SubDword {
    fn of(name: &str) -> Option<Self> {
        let (write, rest) = match name.strip_prefix("ds_read_") {
            Some(r) => (false, r),
            None => (true, name.strip_prefix("ds_write_")?),
        };
        let (ty, d16) = match rest.split_once("_d16") {
            None => (rest, None),
            Some((t, "")) if !write => (t, Some(false)),
            Some((t, "_hi")) => (t, Some(true)),
            Some(_) => return None,
        };
        let (signed, bits) = match (write, ty) {
            (false, "u8") => (false, 8),
            (false, "i8") => (true, 8),
            (false, "u16") => (false, 16),
            (false, "i16") => (true, 16),
            (true, "b8") => (false, 8),
            (true, "b16") => (false, 16),
            _ => return None,
        };
        Some(Self {
            write,
            bits,
            signed,
            d16,
        })
    }
}

impl<'a> Lower<'a> {
    fn new(x: &'a Parts<'a>, bad: &'a HashMap<Value, Why>, numeric_selects: bool) -> Self {
        let p = x.p;
        let mut phi_at = HashMap::new();
        for (b, block) in p.blocks.iter().enumerate() {
            for (i, phi) in block.phis.iter().enumerate() {
                phi_at.insert(phi.value, (b, i));
            }
        }
        let mut s = Shader::new(x.native.stage == HwStage::Ps);
        s.inputs = x.inputs.clone();
        s.outputs = x.outputs.clone();
        let insts = || p.blocks.iter().flat_map(|b| &b.insts);
        let dead = crate::sgpr_spill::dead(p);
        let writes_uavs = insts().any(|i| {
            matches!(i.fields, Fields::Image { .. } | Fields::Buffer { .. })
                && (i.name().contains("store") || i.name().contains("atomic"))
        });
        let wide_lds =
            insts().any(|i| ds_integer_atomic(i.name()).is_some_and(|(_, wide, _)| wide));
        let mut console_lanes = false;
        let mut split = false;
        let mut wants_console_flow = false;
        if let StageIo::Cs(cs) = x.native.io {
            s.threads = Some(cs.threads);
            s.lds_dwords = cs.lds_bytes() / 4;
            // Code reading other lanes needs the console's wave size and
            // lane order; so do lane masks read as numbers, and LDS shared
            // within one wave, where the console compiler leaves out barriers.
            // Instructions that run (not the spills forwarding leaves dead).
            let live = || {
                p.blocks.iter().enumerate().flat_map(|(b, block)| {
                    let dead = &dead;
                    block
                        .insts
                        .iter()
                        .enumerate()
                        .filter(move |&(i, _)| !dead.contains(&(b, i)))
                        .map(|(_, inst)| inst)
                })
            };
            // Swizzles, DPP and quad derivatives read lanes of the same 32
            // (rows of 16, groups of 8, 32 or 4): the rest may read the
            // other half of a wave64.
            let in_half = |i: &Inst| {
                i.name().starts_with("ds_swizzle")
                    || implicit_derivatives(i.name())
                    || matches!(&i.fields, Fields::Valu(v) if v.dpp.is_some())
            };
            let whole = |i: &Inst| {
                let n = i.name();
                n.contains("permute")
                    || n.starts_with("v_permlane")
                    || n.starts_with("v_mbcnt")
                    // Its derivatives are the quad's.
                    || n == "image_sample"
                    || matches!(n, "v_readlane_b32" | "v_writelane_b32")
                    || reads_mask_as_data(x.st, i)
                    || numeric_selects && numeric_select(x.st, i)
            };
            let reads_whole = live().any(whole);
            let masks_held = {
                // Masks held as numbers or constant lane sets: the lane
                // tests its own bit by its console lane.
                let whens = x.st.exits.iter().filter_map(|e| match *e {
                    LaneExit::Branch { when, .. } => Some(when),
                    _ => None,
                });
                let mut all: Vec<Bool> =
                    x.st.guards
                        .iter()
                        .flatten()
                        .flatten()
                        .copied()
                        .chain(whens)
                        .collect();
                all.sort_unstable();
                all.dedup();
                all.into_iter().any(|g| {
                    x.st.bools
                        .support(g)
                        .iter()
                        .any(|a| matches!(a, Atom::Opaque(_) | Atom::Constant(_)))
                })
            };
            console_lanes = reads_whole || masks_held || live().any(in_half);
            let wave = if x.native.wave64 { 64 } else { 32 };
            let one_wave = cs.threads.iter().product::<u32>() <= wave;
            // On a host without 64-lane waves, a wave64 program whose lanes
            // read only their own half runs as two 32-lane waves: what each
            // lane reads is in its host wave, at its host lane's place in the
            // row. Not with LDS (the console compiler may leave out barriers
            // between the halves) nor whole-wave sections (their reductions
            // become wave operations over the host's wave).
            split = console_lanes
                && x.native.wave64
                && !reads_whole
                && !masks_held
                && s.lds_dwords == 0
                && !x.target.runs(64)
                && x.target.runs(32)
                && (0..p.blocks.len()).all(|b| super::wave::sections(p, b).is_empty());
            // The rest of such programs need the halves to exchange values
            // (masks read whole, lanes of the other half): the console flow,
            // for a group of one console wave without LDS.
            wants_console_flow = console_lanes
                && !split
                && x.native.wave64
                && !x.target.runs(64)
                && x.target.runs(32)
                && s.lds_dwords == 0
                && cs.threads.iter().product::<u32>() <= 64
                && !insts().any(|i| i.name().contains("permute"))
                && (0..p.blocks.len()).all(|b| super::wave::sections(p, b).is_empty());
            // On a host that runs the console's wave size: lane reads and
            // writes in a program whose lanes branch differently (see
            // [`CONSOLE_FLOW`]).
            let uniform = |when: Bool| {
                x.st.bools
                    .support(when)
                    .iter()
                    .all(|a| matches!(a, Atom::Uniform(_)))
            };
            wants_console_flow |= x.target.runs(wave)
                && live().any(|i| matches!(i.name(), "v_readlane_b32" | "v_writelane_b32"))
                && x.st
                    .exits
                    .iter()
                    .any(|e| matches!(*e, LaneExit::Branch { when, .. } if !uniform(when)))
                && !insts().any(|i| i.name().contains("permute"))
                && (0..p.blocks.len()).all(|b| super::wave::sections(p, b).is_empty());
            if x.console_cf {
                console_lanes = true;
                // In halves only where the host cannot run the console wave.
                split = x.native.wave64 && !x.target.runs(64);
            }
            if split {
                s.wave_size = Some(32);
            } else if console_lanes || one_wave && s.lds_dwords > 0 {
                s.wave_size = Some(wave);
            }
        }
        let mut no_barrier = HashSet::new();
        if let StageIo::Ms(ms) = &x.native.io {
            s.prims = x.prims.clone();
            s.threads = Some([ms.threads, 1, 1]);
            s.lds_dwords = ms.lds_bytes.div_ceil(4);
            s.mesh = Some(super::shader::Mesh {
                max_vertices: ms.max_vertices,
                max_primitives: ms.max_primitives,
                topology: ms.topology,
            });
            // Barriers order the lanes' LDS accesses; the console's
            // allocation request needs them too, but the host does not. One
            // lane sends that request, and what it dominates (the path for
            // counts out of range) runs in that lane alone, so its barrier
            // would hang the host.
            let order = crate::ir::reverse_postorder(&p.blocks);
            let mut index = vec![usize::MAX; p.blocks.len()];
            for (n, &b) in order.iter().enumerate() {
                index[b] = n;
            }
            let idom = crate::ir::idoms(&p.blocks, &order, &index);
            let alloc: Vec<usize> = (0..p.blocks.len())
                .filter(|&b| {
                    p.blocks[b].insts.iter().any(|i| {
                        i.name() == "s_sendmsg"
                            && matches!(i.fields, Fields::Sopp { imm } if imm & 0xF == GS_ALLOC_REQ)
                    })
                })
                .collect();
            for &b in &order {
                let mut d = b;
                let dominated = loop {
                    if alloc.contains(&d) {
                        break true;
                    }
                    if idom[d] == d || idom[d] == usize::MAX {
                        break false;
                    }
                    d = idom[d];
                };
                if ms.lds_bytes == 0 || dominated {
                    no_barrier.insert(b);
                }
            }
        }
        Lower {
            s,
            x,
            bad,
            lane_fail: HashMap::new(),
            vals: HashMap::new(),
            floats: HashMap::new(),
            block: vec![None; p.blocks.len()],
            cur: 0,
            bools: HashMap::new(),
            lane_bits: HashMap::new(),
            phi_at,
            mask_phis: HashMap::new(),
            pending_masks: Vec::new(),
            data_phis: Vec::new(),
            read_phis: HashSet::new(),
            params: x.res.params.iter().map(|&(v, p, d)| (v, (p, d))).collect(),
            user_data: None,
            user_reads: BTreeMap::new(),
            samplers: Vec::new(),
            descriptors: x
                .res
                .descriptors
                .iter()
                .map(|d| ((d.site, d.role == Role::Sampler), &d.source))
                .collect(),
            scalar: x.res.scalar.iter().copied().collect(),
            exports: x.io.exports.iter().map(|e| (e.site, e)).collect(),
            interps: x.io.interps.iter().map(|(s, i)| (*s, i)).collect(),
            handles: HashMap::new(),
            edges: HashMap::new(),
            exported: vec![false; p.blocks.len()],
            rpo: vec![usize::MAX; p.blocks.len()],
            tail: vec![None; p.blocks.len()],
            writes_uavs,
            wide_lds,
            src0: None,
            console_lanes,
            numeric_selects,
            split,
            console_cf: x.console_cf,
            wants_console_flow,
            console_flat: None,
            xchg: None,
            flat: None,
            no_barrier,
            half_outputs: half_outputs(&x.native.io, &x.outputs),
            half_inputs: {
                let hi = half_inputs(&x.native.io, &x.inputs);
                hi
            },
            half_input_spills: half_input_spills(&half_inputs(&x.native.io, &x.inputs), &x.inputs),
            mask_data: match x.native.stage {
                HwStage::Cs => MaskData::Console,
                HwStage::Ps | HwStage::Vs | HwStage::Ms => MaskData::Host,
                _ => MaskData::None,
            },
            mask_ok: false,
            ballots: HashMap::new(),
            mask_scc: mask_sccs(x.st, p),
            mask_numbers: mask_numbers(x.st, p),
            balloting: HashSet::new(),
            def_block: p
                .blocks
                .iter()
                .enumerate()
                .flat_map(|(b, block)| {
                    block
                        .insts
                        .iter()
                        .flat_map(move |i| i.writes.iter().map(move |w| (w.1, b)))
                        .chain(block.phis.iter().map(move |phi| (phi.value, b)))
                })
                .collect(),
            cur_ir: 0,
            sections: HashMap::new(),
            section_out: HashSet::new(),
            one_lane: Default::default(),
            coherent: None,
            ordered: None,
            made: Default::default(),
            code_table: None,
            scratch: None,
            counters: HashMap::new(),
            loop_exits: HashMap::new(),
            loop_bodies: HashMap::new(),
            finals: HashMap::new(),
            abort: None,
            lane_paths: Vec::new(),
            pending_paths: Vec::new(),
            dead,
        }
    }

    /// The dword of the program's code at byte `byte` (a constant table
    /// the compiler appends to the code), as an internal constant array.
    // shortcut: an index outside the table reads its last dword (the
    // console would read whatever memory follows the code).
    fn code_dword(&mut self, byte: V) -> Result<V, Why> {
        let (g, base, len) = match self.code_table {
            Some(t) => t,
            None => {
                let first = self
                    .x
                    .res
                    .scalar
                    .iter()
                    .map(|s| s.1)
                    .chain(self.x.res.descriptors.iter().flat_map(|d| {
                        d.source.leaves().into_iter().filter_map(|l| match l {
                            Source::Built { address, .. } => Some(*address),
                            _ => None,
                        })
                    }))
                    .filter(|a| a.param == CODE)
                    .map(|a| if a.includes_base { 0 } else { a.offset & !3 })
                    .min()
                    .ok_or("a code table")?;
                let code = &self.x.native.code;
                let words: Vec<u32> = code
                    .get(first as usize..)
                    .ok_or("a code table past the code")?
                    .chunks(4)
                    .map(|c| {
                        let mut w = [0u8; 4];
                        w[..c.len()].copy_from_slice(c);
                        u32::from_le_bytes(w)
                    })
                    .collect();
                if words.is_empty() {
                    return Err("a code table past the code");
                }
                let elems: Vec<V> = words.iter().map(|&w| self.s.m.i32(w)).collect();
                let arr = self.s.m.array(words.len() as u32, self.s.t.i32);
                let init = self.s.m.aggregate(arr, &elems);
                let g = self.s.m.constant_global("code", init);
                let t = (g, first, words.len() as u32);
                self.code_table = Some(t);
                t
            }
        };
        let c = self.cur;
        let k = self.s.m.i32(base);
        let rel = self.s.m.bin(c, Bin::Sub, byte, k);
        let two = self.s.m.i32(2);
        let index = self.s.m.bin(c, Bin::LShr, rel, two);
        let last = self.s.m.i32(len - 1);
        let over = self.s.m.cmp(c, pred::IUGT, index, last);
        let index = self.s.m.select(c, over, last, index);
        let at = self.s.m.element(c, g, index);
        Ok(self.s.m.load(c, at))
    }

    /// Private (scratch) memory, `scratch_load_*` and `scratch_store_*`.
    /// Each lane has its own: the address (VGPR, SGPR or none, plus the
    /// offset) is a byte offset into the lane's memory, and the hardware
    /// interleaves the lanes' dwords itself (RDNA 2 ISA, "Scratch
    /// (Private)"; LLVM AMDGPUUsage, "Private Segment"). On the host it is
    /// an array of dwords per lane (`alloca`, as DXC makes local arrays).
    // shortcut: a dword access is taken as aligned, a byte or short one
    // not to cross a dword (console compilers keep private data aligned).
    fn scratch(&mut self, inst: &Inst, b: usize, i: usize) -> Result<(), Why> {
        let Fields::Flat {
            dst,
            addr,
            data,
            saddr,
            offset,
            lds,
            ..
        } = &inst.fields
        else {
            unreachable!()
        };
        if *lds {
            return Err("a scratch load into LDS");
        }
        let name = inst.name();
        let (load, rest) = match name.strip_prefix("scratch_load_") {
            Some(rest) => (true, rest),
            None => (false, name.strip_prefix("scratch_store_").ok_or(name)?),
        };
        let width = scratch_width(rest).ok_or(name)?;
        let (signed, hi) = (rest.starts_with('s'), rest.ends_with("_hi"));
        let guard = self.guard(b, i)?;
        let base = match (addr, saddr) {
            (Some(o), _) => self.vgprs(inst, o, 1)?[0],
            (None, Some(o)) => self.operand(inst, o)?,
            (None, None) => self.s.m.i32(0),
        };
        let (arr, dwords) = match self.scratch {
            Some(a) => a,
            None => {
                let dwords = scratch_bytes(self.x.p).div_ceil(4);
                let ty = self.s.m.array(dwords, self.s.t.i32);
                let a = (self.s.m.alloca(self.s.entry, ty), dwords);
                self.scratch = Some(a);
                a
            }
        };
        let c = self.cur;
        let off = self.s.m.i32(i32::from(*offset) as u32);
        let at = self.s.m.bin(c, Bin::Add, base, off);
        let two = self.s.m.i32(2);
        let first = self.s.m.bin(c, Bin::LShr, at, two);
        // Dword `k` of the access: its element (the first when out of
        // range) and whether it is in range.
        let element = |l: &mut Self, k: u32| -> (V, V) {
            let c = l.cur;
            let k = l.s.m.i32(k);
            let index = l.s.m.bin(c, Bin::Add, first, k);
            let n = l.s.m.i32(dwords);
            let inside = l.s.m.cmp(c, pred::IULT, index, n);
            let zero = l.s.m.i32(0);
            let index = l.s.m.select(c, inside, index, zero);
            (l.s.m.element(c, arr, index), inside)
        };
        let three = self.s.m.i32(3);
        let byte = self.s.m.bin(c, Bin::And, at, three);
        let shift = self.s.m.bin(c, Bin::Shl, byte, three);
        if load {
            let Some(Operand::Vector { reg, .. }) = *dst else {
                return Err("a scratch load result");
            };
            let zero = self.s.m.i32(0);
            if width >= 4 {
                for k in 0..width / 4 {
                    let (p, inside) = element(self, k);
                    let x = self.s.m.load(c, p);
                    let x = self.s.m.select(c, inside, x, zero);
                    self.write(inst, Reg::V(reg + k as u8), x, guard);
                }
                return Ok(());
            }
            let (p, inside) = element(self, 0);
            let w = self.s.m.load(c, p);
            let w = self.s.m.select(c, inside, w, zero);
            let x = self.s.m.bin(c, Bin::LShr, w, shift);
            let left = self.s.m.i32(32 - 8 * width);
            let x = self.s.m.bin(c, Bin::Shl, x, left);
            let mut x = self
                .s
                .m
                .bin(c, if signed { Bin::AShr } else { Bin::LShr }, x, left);
            if rest.contains("d16") {
                // Into one half of the VGPR (extended to 16 bits); the
                // other half keeps its bits.
                let old = match inst.read(Reg::V(reg)).map(|o| self.i32_of(o)) {
                    Some(Ok(o)) => o,
                    _ => self.s.m.i32(0),
                };
                let low = self.s.m.i32(0xFFFF);
                let h = self.s.m.bin(c, Bin::And, x, low);
                let (h, keep) = if hi {
                    let sixteen = self.s.m.i32(16);
                    (self.s.m.bin(c, Bin::Shl, h, sixteen), low)
                } else {
                    (h, self.s.m.i32(0xFFFF_0000))
                };
                let o = self.s.m.bin(c, Bin::And, old, keep);
                x = self.s.m.bin(c, Bin::Or, o, h);
            }
            self.write(inst, Reg::V(reg), x, guard);
            return Ok(());
        }
        let data = data.as_ref().ok_or("scratch store data")?;
        let vals = self.vgprs(inst, data, width.div_ceil(4) as u8)?;
        for (k, mut x) in vals.into_iter().enumerate() {
            let (p, inside) = element(self, k as u32);
            let g = match guard {
                Some(g) => self.s.m.bin(self.cur, Bin::And, g, inside),
                None => inside,
            };
            let at = self.begin_guarded(Some(g));
            let c = self.cur;
            if width < 4 {
                // The byte or short within its dword (`_d16_hi`: from the
                // high half of the VGPR).
                if hi {
                    let sixteen = self.s.m.i32(16);
                    x = self.s.m.bin(c, Bin::LShr, x, sixteen);
                }
                let mask = self.s.m.i32((1 << (8 * width)) - 1);
                let mask = self.s.m.bin(c, Bin::Shl, mask, shift);
                let x = self.s.m.bin(c, Bin::Shl, x, shift);
                let x = self.s.m.bin(c, Bin::And, x, mask);
                let ones = self.s.m.i32(u32::MAX);
                let keep = self.s.m.bin(c, Bin::Xor, mask, ones);
                let old = self.s.m.load(c, p);
                let old = self.s.m.bin(c, Bin::And, old, keep);
                let new = self.s.m.bin(c, Bin::Or, old, x);
                self.s.m.store(c, p, new);
            } else {
                self.s.m.store(c, p, x);
            }
            self.end_guarded(at, &[]);
        }
        Ok(())
    }

    /// A global memory load (`global_load_*`): from the program's constant
    /// table, or from a root SRV at the address the title sets.
    fn flat(&mut self, inst: &Inst, b: usize, i: usize) -> Result<(), Why> {
        let Fields::Flat { dst, lds, .. } = &inst.fields else {
            unreachable!()
        };
        if *lds {
            return Err("a global load directly into LDS");
        }
        let name = inst.name();
        if self.x.res.addresses && !self.scalar.contains_key(&(b, i)) {
            return self.flat_memory(inst, b, i);
        }
        let rest = name
            .strip_prefix("global_load_")
            .ok_or("a global store or atomic without root UAV address support")?;
        let sub = subword_load(rest);
        let n = match rest {
            "dword" => 1,
            "dwordx2" => 2,
            "dwordx3" => 3,
            "dwordx4" => 4,
            "dword_addtid" => return Err("a global load with a console thread ID offset"),
            _ if sub.is_some() => 1,
            _ => return Err(name),
        };
        let guard = self.guard(b, i)?;
        let Some(Operand::Vector { reg, .. }) = *dst else {
            return Err("a global load result");
        };
        let Some(&a) = self.scalar.get(&(b, i)) else {
            if sub.is_none() && crate::resource::traces_rays(self.x.p) {
                // shortcut: out of the acceleration structure.
                let z = self.s.m.i32(0);
                for k in 0..n {
                    self.write(inst, Reg::V(reg + k as u8), z, guard);
                }
                return Ok(());
            }
            return Err("an untracked global address");
        };
        let c = self.cur;
        let mut off = self.s.m.i32(a.offset);
        if let Some(d) = self.dynamic_term(&a)? {
            off = self.s.m.bin(c, Bin::Add, off, d);
        }
        let h = if a.param == CODE {
            None
        } else {
            let src = Source::Built {
                address: a,
                bits: 0,
                words: [0, 0],
            };
            Some(self.handle((b, i), &src, false, (11, 0))?)
        };
        let dword = |l: &mut Self, byte: V| -> Result<V, Why> {
            match h {
                None => l.code_dword(byte),
                Some(h) => {
                    let u = l.s.m.undef(l.s.t.i32);
                    Ok(l.raw_dword(h, [byte, u]))
                }
            }
        };
        if let Some((width, signed, half)) = sub {
            let x = self.read_subword(off, width, signed, dword)?;
            self.write_subword_result(inst, reg, x, half, guard)?;
        } else {
            for k in 0..n {
                let four = self.s.m.i32(4 * k);
                let at = self.s.m.bin(c, Bin::Add, off, four);
                let x = dword(self, at)?;
                self.write(inst, Reg::V(reg + k as u8), x, guard);
            }
        }
        Ok(())
    }

    /// Finds the whole-wave sections and the values they leave.
    fn find_sections(&mut self) {
        let p = self.x.p;
        let mut inside: HashMap<Value, (usize, usize, usize)> = HashMap::new();
        for b in 0..p.blocks.len() {
            let found = super::wave::sections(p, b);
            for (&start, sec) in &found {
                for i in start + 1..sec.end {
                    for &(_, w) in &p.blocks[b].insts[i].writes {
                        inside.insert(w, (b, start, sec.end));
                    }
                }
            }
            if !found.is_empty() {
                self.sections.insert(b, found);
            }
        }
        if inside.is_empty() {
            return;
        }
        for (b, block) in p.blocks.iter().enumerate() {
            for phi in &block.phis {
                self.section_out
                    .extend(phi.args.iter().filter(|a| inside.contains_key(a)));
            }
            for (i, inst) in block.insts.iter().enumerate() {
                if self.dead.contains(&(b, i)) {
                    continue;
                }
                for &(_, r) in &inst.reads {
                    if let Some(&(sb, start, end)) = inside.get(&r)
                        && !(sb == b && i > start && i < end)
                    {
                        self.section_out.insert(r);
                    }
                }
            }
        }
    }

    /// The instructions of the whole-wave section from `start` to `end`:
    /// the values that leave it are made from what the simulation found
    /// them to be. A value it found no shape for, written by a step that
    /// does not look at other lanes (scalar code the compiler scheduled
    /// into the section, or lane-local vector code), is lowered as outside
    /// a section: the lanes active on entry compute it as they would under
    /// their own EXEC, and only they run on the host.
    fn section(&mut self, b: usize, start: usize, end: usize) -> Result<(), Stop> {
        const UNKNOWN: Why = "a whole-wave section step the simulation does not follow";
        let insts = &self.x.p.blocks[b].insts;
        let inside: HashSet<Value> = insts[start + 1..end]
            .iter()
            .flat_map(|inst| inst.writes.iter().map(|w| w.1))
            .collect();
        let mut need: HashSet<Value> = inside
            .iter()
            .copied()
            .filter(|w| self.section_out.contains(w))
            .collect();
        let mut plain = vec![false; end - start];
        for j in (start + 1..end).rev() {
            let inst = &insts[j];
            let shapes = &self.sections[&b][&start].shapes;
            let wanted = inst
                .writes
                .iter()
                .any(|&(_, w)| need.contains(&w) && !shapes.contains_key(&w));
            if wanted && lane_local(inst) {
                plain[j - start] = true;
                need.extend(
                    inst.reads
                        .iter()
                        .map(|r| r.1)
                        .filter(|r| inside.contains(r)),
                );
            }
        }
        for j in start + 1..end {
            if plain[j - start] {
                self.inst(b, j)?;
            }
            for &(_, w) in &insts[j].writes {
                let shapes = &self.sections[&b][&start].shapes;
                let mut shape = need.contains(&w).then(|| shapes.get(&w).copied()).flatten();
                // Lanes that do not agree, of which one is read after.
                if shape.is_none()
                    && need.contains(&w)
                    && let Some(lane) = self.lane_read(w, b, start, end)
                    && let Some(sh) = self.sections[&b][&start].lane_shape(w, lane)
                {
                    shape = Some(sh);
                    self.one_lane.insert(w);
                }
                let val = match (shape.map(|sh| self.shape(sh)), plain[j - start]) {
                    (Some(Ok(x)), _) => Val::I32(x),
                    (_, true) => continue,
                    (Some(Err(why)), false) => Val::Missing(why),
                    (None, false) => Val::Missing(UNKNOWN),
                };
                self.vals.insert(w, val);
            }
        }
        Ok(())
    }

    /// The one lane of `w` read outside the section from `start` to `end` of
    /// block `b`, if nothing else reads it there.
    fn lane_read(&self, w: Value, b: usize, start: usize, end: usize) -> Option<u32> {
        let mut found = None;
        for (ob, block) in self.x.p.blocks.iter().enumerate() {
            if block.phis.iter().any(|phi| phi.args.contains(&w)) {
                return None;
            }
            for (i, inst) in block.insts.iter().enumerate() {
                if ob == b && i > start && i < end || !inst.reads.iter().any(|r| r.1 == w) {
                    continue;
                }
                let Fields::Valu(v) = &inst.fields else {
                    return None;
                };
                let lane = match (inst.name(), v.src.get(1)) {
                    ("v_readlane_b32", Some(&Operand::Const(c))) => crate::resource::inline(c)?,
                    _ => return None,
                };
                if found.is_some_and(|f| f != lane) {
                    return None;
                }
                found = Some(lane);
            }
        }
        found
    }

    /// A value a whole-wave section leaves, as DXIL wave operations over the
    /// host's active lanes: the console's result with console lanes; on
    /// another wave size, what the HLSL wave operation the console compiler
    /// expanded gives on that wave.
    fn shape(&mut self, shape: super::wave::Shape) -> Result<V, Why> {
        use super::wave::{Op, Shape};
        let (op, src, mask, ident, prefix) = match shape {
            Shape::K(k) => return Ok(self.s.m.i32(k)),
            Shape::Value(v) => return self.i32_of(v),
            Shape::Active(op, src, mask, ident) => (op, src, mask, ident, None),
            Shape::ActiveF64(lo, hi, mask, is_hi) => {
                let c = self.cur;
                let &m = self
                    .x
                    .st
                    .masks
                    .get(&mask)
                    .ok_or("a wave reduction's untracked mask")?;
                let on = self.boolean(m, c)?;
                let lo_raw = self.i32_of(lo)?;
                let hi_raw = self.i32_of(hi)?;
                let z = self.s.m.i32(0);
                let lo_v = self.s.m.select(c, on, lo_raw, z);
                let hi_v = self.s.m.select(c, on, hi_raw, z);
                self.s.flags |= flags::WAVE_OPS | flags::DOUBLES;
                let f64t = self.s.m.double();
                let d = self
                    .s
                    .op(c, "makeDouble.f64", Attr::ReadNone, f64t, 101, &[lo_v, hi_v]);
                let (k0, u0) = (self.s.m.i8(0), self.s.m.i8(0));
                let sum = self
                    .s
                    .op(c, "waveActiveOp.f64", Attr::NoUnwind, f64t, 119, &[d, k0, u0]);
                let i32t = self.s.t.i32;
                let pair = self.s.m.named_struct("dx.types.splitdouble", vec![i32t, i32t]);
                let split = self
                    .s
                    .op(c, "splitDouble.f64", Attr::ReadNone, pair, 102, &[sum]);
                return Ok(self.s.m.extract(c, split, u32::from(is_hi)));
            }
            Shape::Masked(src, mask, ident) => {
                let c = self.cur;
                let &m = self
                    .x
                    .st
                    .masks
                    .get(&mask)
                    .ok_or("a wave reduction's untracked mask")?;
                let on = self.boolean(m, c)?;
                let x = self.i32_of(src)?;
                let id = self.s.m.i32(ident);
                return Ok(self.s.m.select(c, on, x, id));
            }
            Shape::Prefix(op, src, mask, ident, inclusive) => {
                (op, src, mask, ident, Some(inclusive))
            }
        };
        if self.mask_data == MaskData::None {
            return Err("a wave reduction in a mesh shader");
        }
        if self.split {
            return Err("a wave reduction in a wave64 program split in halves");
        }
        let c = self.cur;
        let &m = self
            .x
            .st
            .masks
            .get(&mask)
            .ok_or("a wave reduction's untracked mask")?;
        let on = self.boolean(m, c)?;
        let x = self.i32_of(src)?;
        let id = self.s.m.i32(ident);
        let x = self.s.m.select(c, on, x, id);
        self.s.flags |= flags::WAVE_OPS;
        let f16 = op == Op::AddF16;
        let float = matches!(op, Op::AddF | Op::AddF16 | Op::MinF | Op::MaxF);
        let (i32t, f32t) = (self.s.t.i32, self.s.t.f32);
        let arg = if f16 {
            self.s
                .op(c, "legacyF16ToF32", Attr::ReadNone, f32t, 131, &[x])
        } else if float {
            self.as_float(x)
        } else {
            x
        };
        let ty = if float { f32t } else { i32t };
        let sfx = if float { "f32" } else { "i32" };
        let r = match (op, prefix) {
            (Op::AddU | Op::AddF | Op::AddF16, Some(inclusive)) => {
                let (k0, k1) = (self.s.m.i8(0), self.s.m.i8(1));
                let r = self.s.op(
                    c,
                    &format!("wavePrefixOp.{sfx}"),
                    Attr::NoUnwind,
                    ty,
                    121,
                    &[arg, k0, k1],
                );
                if inclusive {
                    self.s.m.bin(c, Bin::Add, r, arg)
                } else {
                    r
                }
            }
            (_, Some(_)) => return Err("a wave prefix of an operation other than a sum"),
            (Op::Or | Op::And | Op::Xor, None) => {
                let k = self.s.m.i8(match op {
                    Op::And => 0,
                    Op::Or => 1,
                    _ => 2,
                });
                self.s
                    .op(c, "waveActiveBit.i32", Attr::NoUnwind, i32t, 120, &[arg, k])
            }
            (_, None) => {
                let (code, unsigned) = match op {
                    Op::AddU => (0, 1),
                    Op::AddF | Op::AddF16 => (0, 0),
                    Op::MinU => (2, 1),
                    Op::MaxU => (3, 1),
                    Op::MinI | Op::MinF => (2, 0),
                    _ => (3, 0),
                };
                let (k, u) = (self.s.m.i8(code), self.s.m.i8(unsigned));
                self.s.op(
                    c,
                    &format!("waveActiveOp.{sfx}"),
                    Attr::NoUnwind,
                    ty,
                    119,
                    &[arg, k, u],
                )
            }
        };
        Ok(if f16 {
            self.s
                .op(c, "legacyF32ToF16", Attr::ReadNone, i32t, 130, &[r])
        } else if float {
            self.as_int(r)
        } else {
            r
        })
    }

    fn run(mut self) -> Result<Lowered, Stop> {
        let user_data = self.lower_ir()?;
        let root_cbvs = self.s.root_cbvs.iter().map(|r| r.0).collect();
        let root_srvs = self.s.root_srvs.iter().map(|r| r.0).collect();
        let root_uavs = self.s.root_uavs.iter().map(|r| r.0).collect();
        let stride_table = self.s.stride_table.is_some();
        let cmp_samplers = self.s.cmp_samplers;
        Ok(Lowered {
            cmp_samplers,
            root_cbvs,
            root_srvs,
            root_uavs,
            #[cfg(test)]
            nonuniform_handles: self.s.nonuniform_handles,
            #[cfg(test)]
            wave_ops: self.s.flags & flags::WAVE_OPS != 0,
            #[cfg(test)]
            annotations: std::mem::take(&mut self.s.annotations),
            stride_table,
            dxil: self.s.finish(),
            user_data,
            samplers: self.samplers,
        })
    }

    fn lower_ir(&mut self) -> Result<Vec<UserData>, Stop> {
        if self.wants_console_flow && !self.x.console_cf && !self.x.per_lane_only {
            return Err(unsupported(0, CONSOLE_FLOW));
        }
        if let Some(n) = self.s.wave_size
            && !self.x.target.runs(n)
        {
            // shortcut: no form of these lanes on the host's wave size yet
            // (two 32-lane waves standing for one console wave64 need the
            // halves to exchange masks and lane values).
            return Err(unsupported(
                0,
                match (n, self.console_lanes) {
                    (64, true) => "console wave64 lanes on a host without 64-lane waves",
                    (64, false) => {
                        "a wave64 group sharing LDS without barriers on a host without 64-lane waves"
                    }
                    _ => "a console wave size the host does not run",
                },
            ));
        }
        let p = self.x.p;
        let order = crate::ir::reverse_postorder(&p.blocks);
        for (n, &b) in order.iter().enumerate() {
            self.rpo[b] = n;
        }
        for (n, &b) in order.iter().enumerate() {
            self.block[b] = Some(if n == 0 {
                self.s.entry
            } else {
                self.s.m.new_block()
            });
        }
        // Loop headers count iterations (`LOOP_LIMIT`).
        for &b in &order {
            let header = p.blocks[b]
                .preds
                .iter()
                .any(|&q| self.block[q].is_some() && self.rpo[b] <= self.rpo[q]);
            if header {
                let blk = self.block[b].expect("reachable");
                let phi = self.s.m.phi(blk, self.s.t.i32);
                self.counters.insert(b, (phi, Vec::new()));
            }
        }
        self.find_loop_exits();
        self.cur = self.s.entry;
        self.find_sections();
        self.inputs()?;
        if let Some(mesh) = self.s.mesh {
            self.mesh_prologue(mesh).map_err(|w| unsupported(0, w))?;
            self.block[order[0]] = Some(self.cur);
        }
        // Dispatches count threads too: the last group's others do not run.
        if self.x.res.addresses
            && self.s.mesh.is_none()
            && let Some([x, ..]) = self.s.threads
        {
            let e = self.s.entry;
            let zero = self.s.m.i32(0);
            let i32t = self.s.t.i32;
            let id = self.s.op(e, "threadId.i32", Attr::ReadNone, i32t, 93, &[zero]);
            let start = self.host_data(1, RootType::GROUPS, 0);
            let group = self.s.m.i32(x);
            let start = self.s.m.bin(e, Bin::Mul, start, group);
            let id = self.s.m.bin(e, Bin::Add, id, start);
            let most = self.host_data(2, RootType::THREADS, x);
            let runs = self.s.m.cmp(e, pred::IULT, id, most);
            let stop = self.abort_block();
            let go = self.s.m.new_block();
            self.s.m.terminate(e, Term::CondBr(runs, go, stop));
            self.cur = go;
            self.block[order[0]] = Some(go);
        }
        for &b in &order {
            self.cur = self.block[b].expect("reachable");
            self.cur_ir = b;
            let preds: Vec<usize> = p.blocks[b]
                .preds
                .iter()
                .copied()
                .filter(|&q| self.block[q].is_some())
                .collect();
            self.exported[b] = !preds.is_empty() && preds.iter().all(|&q| self.exported_out(q));
            for (i, phi) in p.blocks[b].phis.iter().enumerate() {
                let val = match phi.reg {
                    Reg::S(c) if c == EXEC || c == EXEC + 1 => Val::Missing("a lane mask"),
                    _ if self.bad.contains_key(&phi.value) => Val::Missing(self.bad[&phi.value]),
                    Reg::Scc => {
                        let v = self.s.m.phi(self.cur, self.s.t.i1);
                        self.data_phis.push((v, b, i));
                        Val::Bit(v)
                    }
                    _ => {
                        let v = self.s.m.phi(self.cur, self.s.t.i32);
                        self.data_phis.push((v, b, i));
                        Val::I32(v)
                    }
                };
                self.vals.insert(phi.value, val);
            }
            // Compute programs only: their host waves are the console's, so a
            // wave's value is the console wave's.
            if self.mask_data == MaskData::Console {
                self.leave_loops(b);
                if !self.loop_exits.contains_key(&b) {
                    self.join_scalars(b);
                }
            }
            let mut i = 0;
            while i < p.blocks[b].insts.len() {
                self.inst(b, i)?;
                if let Some(end) = self.sections.get(&b).and_then(|s| s.get(&i)).map(|s| s.end) {
                    self.section(b, i, end)?;
                    i = end;
                    continue;
                }
                i += 1;
            }
            self.mask_ok = self.mask_data == MaskData::Console;
            self.tail[b] = Some(self.cur);
            self.exit(b)?;
        }
        self.fill_phis()?;
        if self.s.pixel {
            settle_position_mode(&mut self.s.inputs);
            settle_row_modes(&mut self.s.inputs, true).map_err(|w| unsupported(0, w))?;
        } else {
            settle_row_modes(&mut self.s.outputs, false).map_err(|w| unsupported(0, w))?;
        }
        let user_data = self.user_reads.values().copied().collect();
        self.s.samplers = self.samplers.len() as u32;
        Ok(user_data)
    }

    fn exported_out(&self, b: usize) -> bool {
        self.exported[b] || self.has_done_export(b)
    }

    fn has_done_export(&self, b: usize) -> bool {
        (0..self.x.p.blocks[b].insts.len())
            .any(|i| self.exports.get(&(b, i)).is_some_and(|e| e.done))
    }

    /// Values of the program's input registers.
    fn inputs(&mut self) -> Result<(), Stop> {
        let io_inputs: HashMap<Value, Input> = self.x.io.inputs.iter().copied().collect();
        // RDNA2 POPS (ROV) pixel shaders read a hardware wave-ordering SGPR
        // (bit 31 = overlap flag) and spin on `src_pops_exiting_wave_id` (239)
        // when bit 31 is set. Zeroing that SGPR takes the non-overlapping fast
        // path straight past the `s_sleep` spin loop into the critical section.
        let uses_pops = self.s.pixel
            && self.x.p.blocks.iter().flat_map(|b| &b.insts).any(|inst| {
                matches!(
                    &inst.fields,
                    Fields::Sop { src, .. } if src.contains(&Operand::Const(239))
                )
            });
        for &(reg, v) in &self.x.p.inputs {
            let val = match reg {
                Reg::S(_) if self.params.contains_key(&v) => continue, // loaded on first use
                // Most programs only interpolate with them: loaded on first
                // use as a value (`get`).
                Reg::V(_) if matches!(io_inputs.get(&v), Some(Input::Barycentric { .. })) => {
                    continue;
                }
                Reg::S(n) => match io_inputs.get(&v) {
                    Some(&input) => self.input(input),
                    // Above the SGPRs the hardware initializes, a register
                    // holds what the last wave left, so no program depends
                    // on it: a read the paths only seem to allow (a branch
                    // past the write, taken only with one past the read)
                    // reads zero, as VGPRs do.
                    None if n <= 105 && self.x.io.initialized_sgprs.is_some_and(|k| n >= k) => {
                        Val::I32(self.s.m.i32(0))
                    }
                    // Compute: past the user SGPRs the hardware sets only
                    // the group's, which are known. Others no one set (a
                    // path that does not need them); they read as zero, as
                    // VGPRs do. In a pixel program the hardware's come
                    // within the first few after the 32 user SGPRs.
                    None if n <= 105
                        && (self.s.threads.is_some() && self.s.mesh.is_none()
                            || self.s.pixel && (n >= 40 || uses_pops)) =>
                    {
                        Val::I32(self.s.m.i32(0))
                    }
                    None => {
                        if std::env::var_os("VICTORY_WHY").is_some() {
                            eprintln!("input {reg:?}");
                        }
                        Val::Missing("a hardware SGPR input")
                    }
                },
                Reg::Scc => Val::Missing("SCC on entry"),
                Reg::V(_) => match io_inputs.get(&v) {
                    Some(&input) => self.input(input),
                    // Registers no one set read as zero (the validator
                    // refuses reads of undef).
                    None => Val::I32(self.s.m.i32(0)),
                },
            };
            self.vals.insert(v, val);
        }
        Ok(())
    }

    fn input(&mut self, input: Input) -> Val {
        let (kind, col) = match input {
            Input::VertexId => (1, 0),
            Input::InstanceId => (2, 0),
            Input::Position(c) => (3, c),
            Input::Attribute { row, channel } => {
                return match self.find_input(row, channel) {
                    Some((id, col)) => self.load_input(id, col),
                    None => Val::Missing("a vertex input the signature lacks"),
                };
            }
            Input::Barycentric { kind, index } => return self.barycentric(kind, index),
            Input::PrimitiveId => return Val::Missing("the primitive id"),
            // The console code tests it against 0 (`v_cmp_ne_u32`), as the
            // host's value can be.
            Input::FrontFace => (13, 0),
            Input::ThreadIdInGroup(c) => return self.thread_id(c),
            Input::GroupId(0) if self.x.res.addresses => {
                let Val::I32(g) = self.compute_id("groupId.i32", 94, 0) else {
                    unreachable!()
                };
                let first = self.host_data(1, RootType::GROUPS, 0);
                return Val::I32(self.s.m.bin(self.s.entry, Bin::Add, g, first));
            }
            Input::GroupId(c) => return self.compute_id("groupId.i32", 94, c),
            Input::GroupInfo => return Val::Missing("the thread-group info SGPR"),
            Input::MeshGroup => {
                let Val::I32(g) = self.compute_id("groupId.i32", 94, 0) else {
                    unreachable!()
                };
                let n = self.s.m.i32(self.x.mesh_stride);
                return Val::I32(self.s.m.bin(self.s.entry, Bin::Mul, g, n));
            }
            Input::MergedWaveInfo => {
                // shortcut: only the wave index (bits 24-27) is set; the
                // console's other bits (lane counts) are read by no program.
                let f = self.mesh_thread();
                let shift = self.s.m.i32(self.wave().trailing_zeros());
                let w = self.s.m.bin(self.s.entry, Bin::LShr, f, shift);
                let k24 = self.s.m.i32(24);
                return Val::I32(self.s.m.bin(self.s.entry, Bin::Shl, w, k24));
            }
            _ => return Val::Missing("a pixel system input"),
        };
        let free = self
            .s
            .inputs
            .iter()
            .filter_map(|e| e.row)
            .map(|r| r + 1)
            .max()
            .unwrap_or(0);
        let id = match self.s.inputs.iter().position(|e| e.kind == kind) {
            Some(id) => {
                if kind != 3 {
                    self.s.inputs[id].row.get_or_insert(free);
                }
                id
            }
            None if kind >= 3 => return Val::Missing("a pixel system input the signature lacks"),
            None => {
                // The console signature leaves out vertex and instance ids; PC
                // gives them a row after the others.
                let row = self
                    .s
                    .inputs
                    .iter()
                    .filter_map(|e| e.row)
                    .map(|r| r + 1)
                    .max()
                    .unwrap_or(0);
                self.s.inputs.push(SigElement {
                    name: ["", "SV_VertexID", "SV_InstanceID"][kind as usize].into(),
                    index: 0,
                    kind,
                    comp: 5,
                    interp: 0,
                    row: Some(row),
                    col: 0,
                    cols: 1,
                    usage: 0,
                    half: false,
                });
                self.s.inputs.len() - 1
            }
        };
        self.load_input(id, col)
    }

    /// A component of a compute thread or group id, read in the entry block.
    fn compute_id(&mut self, name: &str, opcode: u32, c: u8) -> Val {
        let (entry, c) = (self.s.entry, self.s.m.i32(u32::from(c)));
        Val::I32(
            self.s
                .op(entry, name, Attr::ReadNone, self.s.t.i32, opcode, &[c]),
        )
    }

    fn find_input(&self, row: u8, channel: u8) -> Option<(usize, u8)> {
        self.s.inputs.iter().enumerate().find_map(|(id, e)| {
            let r = e.row?;
            // `then`, not `then_some`: the channel is only past `col` when
            // the condition holds.
            (r == u32::from(row) && channel >= e.col && channel < e.col + e.cols)
                .then(|| (id, channel - e.col))
        })
    }

    fn load_input(&mut self, id: usize, col: u8) -> Val {
        if self.s.inputs[id].half {
            // shortcut: 16-bit vertex inputs (the fetch routine's) are not
            // unpacked; pixel inputs are (`interp`).
            return Val::Missing("a 16-bit input");
        }
        self.load_any_input(id, col)
    }

    /// An input element's channel as declared (16-bit ones as 32-bit floats).
    fn load_any_input(&mut self, id: usize, col: u8) -> Val {
        let e = &mut self.s.inputs[id];
        e.usage |= 1 << col;
        let (int, bit) = (e.comp != 9, e.comp == 1);
        let ty = if bit {
            self.s.t.i1
        } else if int {
            self.s.t.i32
        } else {
            self.s.t.f32
        };
        let suffix = if bit {
            ".i1"
        } else if int {
            ".i32"
        } else {
            ".f32"
        };
        let entry = self.s.entry;
        let args = [
            self.s.m.i32(id as u32),
            self.s.m.i32(0),
            self.s.m.i8(col),
            self.s.m.undef(self.s.t.i32),
        ];
        let v = self.s.op(
            entry,
            &format!("loadInput{suffix}"),
            Attr::ReadNone,
            ty,
            4,
            &args,
        );
        if bit {
            let z = self.s.m.cast(entry, Cast::ZExt, v, self.s.t.i32);
            Val::I32(z)
        } else if int {
            Val::I32(v)
        } else {
            Val::I32(self.s.m.cast(entry, Cast::Bitcast, v, self.s.t.i32))
        }
    }

    /// The DXIL form of an IR value.
    fn get(&mut self, v: Value) -> Result<Val, Why> {
        if self.phi_at.contains_key(&v) {
            self.read_phis.insert(v);
        }
        let is_mask = self.x.st.masks.contains_key(&v) || self.x.st.highs.contains_key(&v);
        if let Some(&(val, h)) = self.finals.get(&v)
            && !self.loop_bodies[&h].contains(&self.cur_ir)
        {
            if is_mask && !self.mask_ok {
                return Err(LANE_MASK);
            }
            return Ok(val);
        }
        if is_mask && !self.mask_ok && std::env::var_os("VICTORY_WHY").is_some() {
            let def = self.def_block.get(&v).map(|&b| (b, self.x.p.blocks[b].insts.iter().find(|i| i.writes.iter().any(|w| w.1 == v)).map(|i| (format!("{:#x}", i.machine.offset), i.name().to_owned())), self.x.p.blocks[b].phis.iter().find(|p| p.value == v).map(|p| (p.reg, p.args.clone()))));
            eprintln!("mask data {v:?} {:?} def {def:?} cur {} high {:?} val {:?}", self.x.st.masks.get(&v), self.cur_ir, self.x.st.highs.get(&v), self.vals.get(&v));
        }
        match self.vals.get(&v) {
            Some(&Val::Missing(_)) | None if self.mask_ok && is_mask => {
                return self.mask_dword(v).map(Val::I32);
            }
            Some(&Val::Missing(_)) if self.mask_ok && self.mask_scc.contains_key(&v) => {
                let lo = self.mask_scc[&v];
                return self.mask_any(lo).map(Val::Bit);
            }
            // A mask's number (from a set operation) only for the uses
            // `MaskData` allows.
            Some(&Val::I32(_)) if is_mask && !self.mask_ok && !self.phi_at.contains_key(&v) => {
                return Err(LANE_MASK);
            }
            // shortcut: descriptors a program that traces rays reads as data
            // (a hit's material, say): no ray hits anything.
            Some(&Val::Missing(
                "a descriptor's address or format read as data"
                | "a descriptor read only as data"
                | "a constant buffer's descriptor read as data",
            )) if crate::resource::traces_rays(self.x.p) => {
                return Ok(Val::I32(self.s.m.i32(0)));
            }
            Some(&Val::Missing(w)) => return Err(w),
            Some(&val) => return Ok(val),
            None => {}
        }
        if let Some(&(param, dword)) = self.params.get(&v) {
            let val = self.user(param, dword);
            self.vals.insert(v, val);
            return Ok(val);
        }
        if let Some(loaded) = self.lazy_barycentric(v) {
            loaded?;
            return Ok(self.vals[&v]);
        }
        Err("a value defined after its use")
    }

    /// Loads `v` if it is a barycentric input VGPR not loaded yet (see
    /// `inputs`): `None` if it is not one.
    fn lazy_barycentric(&mut self, v: Value) -> Option<Result<(), Why>> {
        let (kind, index) = self.x.io.inputs.iter().find_map(|&(w, i)| match i {
            Input::Barycentric { kind, index } if w == v => Some((kind, index)),
            _ => None,
        })?;
        let val = self.barycentric(kind, index);
        self.vals.insert(v, val);
        Some(match val {
            Val::Missing(w) => Err(w),
            _ => Ok(()),
        })
    }

    /// Barycentric `index` (0 i, 1 j) of `kind` as a value: a channel of
    /// `SV_Barycentrics` declared with the same interpolation mode.
    ///
    /// GFX10 interpolates P0 + i (P1 - P0) + j (P2 - P0), so i and j are
    /// the weights of vertices 1 and 2. The host's weights (x, y, z) are
    /// those of vertices 0, 1 and 2, vertex 0 being the provoking vertex,
    /// which is the console's P0 (`v_interp_mov` P0 is lowered to a flat
    /// input): i is y and j is z.
    /// shortcut: triangles only; on lines and points the console's i and j
    /// are not checked against the host's weights.
    fn barycentric(&mut self, kind: Barycentric, index: u8) -> Val {
        let Some(mode) = interp_mode(kind) else {
            return Val::Missing("pull-model barycentrics");
        };
        if index > 1 {
            return Val::Missing("pull-model barycentrics");
        }
        // One element per interpolation mode (the console's own, if its
        // signature declares one): DXIL allows two.
        let bary = |e: &SigElement| e.kind == 28;
        let id = match self
            .s
            .inputs
            .iter()
            .position(|e| bary(e) && (e.interp == mode || e.usage == 0))
        {
            Some(id) => id,
            None => {
                let index = self.s.inputs.iter().filter(|e| bary(e)).count() as u32;
                if index >= 2 {
                    return Val::Missing("barycentrics of a third interpolation mode");
                }
                // A system value the validator wants unpacked (packing
                // location -1), like other non-packed system values.
                self.s.inputs.push(SigElement {
                    name: "SV_Barycentrics".into(),
                    index,
                    kind: 28,
                    comp: 9,
                    interp: mode,
                    row: None,
                    col: 0,
                    cols: 3,
                    usage: 0,
                    half: false,
                });
                self.s.inputs.len() - 1
            }
        };
        let e = &mut self.s.inputs[id];
        if e.col != 0 || e.cols < 3 {
            return Val::Missing("a console barycentrics element without i and j");
        }
        e.row = None;
        e.interp = mode;
        self.s.flags |= flags::BARYCENTRICS;
        self.load_any_input(id, index + 1)
    }

    /// The computed byte offset `a` adds to its parameter's address: its
    /// computed term, less the parameter's address when that term is a
    /// pointer that holds it (see [`Address::includes_base`]).
    fn dynamic_term(&mut self, a: &Address) -> Result<Option<V>, Why> {
        let Some(d) = a.dynamic else { return Ok(None) };
        let d = self.i32_of(d)?;
        if !a.includes_base {
            return Ok(Some(d));
        }
        if a.param == CODE {
            return Ok(Some(d));
        }
        let base = match self.user(a.param, 0) {
            Val::I32(x) => x,
            _ => return Err("an advanced pointer into a root descriptor"),
        };
        Ok(Some(self.s.m.bin(self.cur, Bin::Sub, d, base)))
    }

    fn i32_of(&mut self, v: Value) -> Result<V, Why> {
        match self.get(v)? {
            Val::I32(x) => Ok(x),
            Val::Bit(x) => Ok(self.s.m.cast(self.cur, Cast::ZExt, x, self.s.t.i32)),
            Val::Missing(w) => Err(w),
        }
    }

    /// A user-data dword: a root parameter's value.
    fn user(&mut self, param: usize, dword: u32) -> Val {
        let rp = self.x.native.layout[param];
        match rp.root {
            RootType::ResourceHeap | RootType::SamplerHeap => return Val::I32(self.s.m.i32(0)),
            RootType::Constants | RootType::Table => {}
            // Its address, for programs that use memory by address.
            RootType::Descriptor if self.x.res.addresses => {}
            _ => return Val::Missing("a root descriptor"),
        }
        let index = u32::from(rp.user_data) + dword;
        self.user_reads.insert(
            index,
            UserData {
                index,
                slot: rp.slot,
                root: rp.root,
                dword,
            },
        );
        self.s.user_data = self.s.user_data.max(index + 1);
        let h = match self.user_data {
            Some(h) => h,
            None => {
                let h = self.s.user_data_handle();
                self.user_data = Some(h);
                h
            }
        };
        let entry = self.s.entry;
        let row = self.s.m.i32(index / 4);
        let r = self.s.op(
            entry,
            "cbufferLoadLegacy.i32",
            Attr::ReadOnly,
            self.s.t.cbuf_ret,
            59,
            &[h, row],
        );
        Val::I32(self.s.m.extract(entry, r, index % 4))
    }

    /// The heap index of the host's first view of memory by address: the
    /// map, then the memory it names.
    fn memory_base(&mut self) -> V {
        self.host_data(0, RootType::MEMORY, 0)
    }

    /// User data the host adds behind the title's: the `nth` such dword.
    fn host_data(&mut self, nth: u32, root: RootType, dword: u32) -> V {
        let layout = &self.x.native.layout;
        let index = layout
            .iter()
            .map(|p| u32::from(p.user_data) + p.sgprs.max(1))
            .max()
            .unwrap_or(0)
            + nth;
        self.user_reads.insert(
            index,
            UserData {
                index,
                slot: u8::MAX,
                root,
                dword,
            },
        );
        self.s.user_data = self.s.user_data.max(index + 1);
        let h = match self.user_data {
            Some(h) => h,
            None => {
                let h = self.s.user_data_handle();
                self.user_data = Some(h);
                h
            }
        };
        let entry = self.s.entry;
        let row = self.s.m.i32(index / 4);
        let r = self.s.op(
            entry,
            "cbufferLoadLegacy.i32",
            Attr::ReadOnly,
            self.s.t.cbuf_ret,
            59,
            &[h, row],
        );
        self.s.m.extract(entry, r, index % 4)
    }

    /// The host's view of the title memory at `hi:lo` (40 bits) and the
    /// byte in it. The map holds two dwords per MiB: which view, and the
    /// byte in it of that MiB's start.
    fn memory(&mut self, lo: V, hi: V, nonuniform: bool) -> (V, V) {
        let c = self.cur;
        let first = self.memory_base();
        let map = self.s.handle_at(c, first, false, false, (11, 0));
        let k = |l: &mut Self, x: u32| l.s.m.i32(x);
        let (k255, k12, k20, k3, low) =
            (k(self, 255), k(self, 12), k(self, 20), k(self, 3), k(self, 0xF_FFFF));
        let h = self.s.m.bin(c, Bin::And, hi, k255);
        let h = self.s.m.bin(c, Bin::Shl, h, k12);
        let l = self.s.m.bin(c, Bin::LShr, lo, k20);
        let mib = self.s.m.bin(c, Bin::Or, h, l);
        let at = self.s.m.bin(c, Bin::Shl, mib, k3);
        let (mask, align, u) = (self.s.m.i8(3), self.s.m.i32(4), self.s.m.undef(self.s.t.i32));
        let r = self.s.op(
            c,
            "rawBufferLoad.f32",
            Attr::ReadOnly,
            self.s.t.res_ret,
            139,
            &[map, at, u, mask, align],
        );
        let view = self.s.m.extract(c, r, 0);
        let view = self.as_int(view);
        let start = self.s.m.extract(c, r, 1);
        let start = self.as_int(start);
        // Never past the views of ours, whatever the map holds.
        let (i32t, most) = (self.s.t.i32, self.s.m.i32(super::VIEW_SLOTS - super::OWN_SLOTS - 1));
        let view = self.s.op(c, "binary.i32", Attr::ReadNone, i32t, 40, &[view, most]);
        let index = self.s.m.bin(c, Bin::Add, first, view);
        // Builders read what other groups of the dispatch wrote.
        let p = self.x.p;
        let shared = *self.coherent.get_or_insert_with(|| {
            p.blocks.iter().flat_map(|b| &b.insts).any(|i| {
                let n = i.name();
                n.starts_with("global_store") || n.starts_with("global_atomic")
            })
        });
        let shared = if shared { 0x4000 } else { 0 };
        let handle = self.s.handle_at(c, index, false, nonuniform, (11 | 0x1000 | shared, 0));
        let within = self.s.m.bin(c, Bin::And, lo, low);
        (handle, self.s.m.bin(c, Bin::Add, start, within))
    }

    /// `hi:lo` plus `x` (sign-extended).
    fn add_address(&mut self, (lo, hi): (V, V), x: V, signed: bool) -> (V, V) {
        let c = self.cur;
        let (r, carry) = self.add_sub_carry(c, true, lo, x, None);
        let up = self.s.m.cast(c, Cast::ZExt, carry, self.s.t.i32);
        let mut hi = self.s.m.bin(c, Bin::Add, hi, up);
        if signed {
            let k31 = self.s.m.i32(31);
            let s = self.s.m.bin(c, Bin::AShr, x, k31);
            hi = self.s.m.bin(c, Bin::Add, hi, s);
        }
        (r, hi)
    }

    /// `count` dwords of title memory at `hi:lo`.
    fn memory_dwords(&mut self, at: (V, V), count: u32, nonuniform: bool) -> Vec<V> {
        let (h, off) = self.memory(at.0, at.1, nonuniform);
        let mut out = Vec::new();
        let mut done = 0;
        while done < count {
            let n = (count - done).min(4);
            let c = self.cur;
            let d = self.s.m.i32(4 * done);
            let a = self.s.m.bin(c, Bin::Add, off, d);
            let (mask, align, u) =
                (self.s.m.i8((1u8 << n) - 1), self.s.m.i32(4), self.s.m.undef(self.s.t.i32));
            let r = self.s.op(
                c,
                "rawBufferLoad.f32",
                Attr::ReadOnly,
                self.s.t.res_ret,
                139,
                &[h, a, u, mask, align],
            );
            for k in 0..n {
                let x = self.s.m.extract(c, r, k);
                let x = self.as_int(x);
                out.push(x);
            }
            done += n;
        }
        out
    }

    /// The address a global memory instruction gives each lane.
    fn flat_at(&mut self, inst: &Inst) -> Result<(V, V), Why> {
        let Fields::Flat {
            addr, saddr, offset, ..
        } = &inst.fields
        else {
            unreachable!()
        };
        let Some(Operand::Vector { reg, .. }) = *addr else {
            return Err("a global address");
        };
        let lo = inst.read(Reg::V(reg)).ok_or("a global address")?;
        let lo = self.i32_of(lo)?;
        let mut at = match *saddr {
            Some(Operand::Scalar { code, .. }) if code != NULL => {
                let (a, b) = (
                    inst.read(Reg::S(code)).ok_or("a global address")?,
                    inst.read(Reg::S(code + 1)).ok_or("a global address")?,
                );
                let base = (self.i32_of(a)?, self.i32_of(b)?);
                self.add_address(base, lo, false)
            }
            _ => {
                let hi = inst.read(Reg::V(reg + 1)).ok_or("a global address")?;
                (lo, self.i32_of(hi)?)
            }
        };
        if *offset != 0 {
            let x = self.s.m.i32(i32::from(*offset) as u32);
            at = self.add_address(at, x, true);
        }
        Ok(at)
    }

    /// A global memory access by address.
    fn flat_memory(&mut self, inst: &Inst, b: usize, i: usize) -> Result<(), Why> {
        let Fields::Flat { dst, data, .. } = &inst.fields else {
            unreachable!()
        };
        let name = inst.name();
        let guard = self.guard(b, i)?;
        let at = self.flat_at(inst)?;
        let dwords = |s: &str| match s {
            "dword" => Some(1u32),
            "dwordx2" => Some(2),
            "dwordx3" => Some(3),
            "dwordx4" => Some(4),
            _ => None,
        };
        if let Some(rest) = name.strip_prefix("global_load_") {
            let Some(Operand::Vector { reg, .. }) = *dst else {
                return Err("a global load result");
            };
            if let Some((width, signed, half)) = subword_load(rest) {
                let (h, off) = self.memory(at.0, at.1, true);
                let x = self.read_subword(off, width, signed, |l, byte| {
                    let u = l.s.m.undef(l.s.t.i32);
                    Ok(l.raw_dword(h, [byte, u]))
                })?;
                self.write_subword_result(inst, reg, x, half, guard)?;
                return Ok(());
            }
            let n = dwords(rest).ok_or(name)?;
            let xs = self.memory_dwords(at, n, true);
            for (k, x) in xs.into_iter().enumerate() {
                self.write(inst, Reg::V(reg + k as u8), x, guard);
            }
            return Ok(());
        }
        let data = data.as_ref().ok_or("global data")?;
        if let Some(n) = name.strip_prefix("global_store_").and_then(dwords) {
            let vals = self.vgprs(inst, data, n as u8)?;
            let place = self.begin_guarded(guard);
            let (h, off) = self.memory(at.0, at.1, true);
            let iu = self.s.m.undef(self.s.t.i32);
            let mut args = vec![h, off, iu];
            args.extend(&vals);
            args.resize(3 + 4, iu);
            args.push(self.s.m.i8((1u8 << n) - 1));
            args.push(self.s.m.i32(4));
            let void = self.s.t.void;
            self.s.op(self.cur, "rawBufferStore.i32", Attr::NoUnwind, void, 140, &args);
            self.end_guarded(place, &[]);
            return Ok(());
        }
        let op = name.strip_prefix("global_atomic_").ok_or(name)?;
        let code = match op {
            "add" | "sub" => Some(0),
            "and" => Some(1),
            "or" => Some(2),
            "xor" => Some(3),
            "smin" => Some(4),
            "smax" => Some(5),
            "umin" => Some(6),
            "umax" => Some(7),
            "swap" => Some(8),
            "cmpswap" | "fmin" | "fmax" => None,
            _ => return Err(name),
        };
        let vals = self.vgprs(inst, data, if op == "cmpswap" { 2 } else { 1 })?;
        let zero = self.s.m.i32(0);
        let place = self.begin_guarded(guard);
        let (h, off) = self.memory(at.0, at.1, true);
        let iu = self.s.m.undef(self.s.t.i32);
        let coords = [off, iu, iu];
        let r = match (op, code) {
            ("cmpswap", _) => self.buffer_compare_exchange(h, coords, vals[1], vals[0], false),
            // shortcut: no denormal or NaN rules of the console's.
            ("fmin" | "fmax", _) => {
                // Not exchanged where nothing changes: a whole scene's lanes
                // meet at its bounds.
                let head = self.cur;
                let (mask, align) = (self.s.m.i8(1), self.s.m.i32(4));
                let ret = self.s.t.res_ret;
                let first = self.s.op(
                    head,
                    "rawBufferLoad.f32",
                    Attr::ReadOnly,
                    ret,
                    139,
                    &[h, off, iu, mask, align],
                );
                let first = self.s.m.extract(head, first, 0);
                let first = self.as_int(first);
                let retry = self.s.m.new_block();
                let change = self.s.m.new_block();
                let done = self.s.m.new_block();
                self.s.m.terminate(head, Term::Br(retry));
                self.cur = retry;
                let i32t = self.s.t.i32;
                let expected = self.s.m.phi(retry, i32t);
                let (a, b) = (self.as_float(expected), self.as_float(vals[0]));
                let x = self.binary(if op == "fmin" { 36 } else { 35 }, a, b);
                let x = self.as_int(x);
                let same = self.s.m.cmp(retry, pred::EQ, x, expected);
                self.s.m.terminate(retry, Term::CondBr(same, done, change));
                self.cur = change;
                let old = self.buffer_compare_exchange(h, coords, expected, x, false);
                let ok = self.s.m.cmp(change, pred::EQ, old, expected);
                self.s.m.set_phi(expected, vec![(first, head), (old, change)]);
                self.s.m.terminate(change, Term::CondBr(ok, done, retry));
                self.cur = done;
                expected
            }
            (_, Some(code)) => {
                let code = self.s.m.i32(code);
                let data = if op == "sub" {
                    self.s.m.bin(self.cur, Bin::Sub, zero, vals[0])
                } else {
                    vals[0]
                };
                let i32t = self.s.t.i32;
                self.s.op(
                    self.cur,
                    "atomicBinOp.i32",
                    Attr::NoUnwind,
                    i32t,
                    78,
                    &[h, code, off, iu, iu, data],
                )
            }
            _ => return Err(name),
        };
        let r = self.end_guarded(place, &[(r, zero)])[0];
        if let Some(Operand::Vector { reg, .. }) = *dst {
            self.write(inst, Reg::V(reg), r, guard);
        }
        Ok(())
    }

    /// `image_bvh64_intersect_ray`: the ray `words` (node pointer, extent,
    /// origin, direction, inverse direction) against a node of the
    /// acceleration structure, as the console's ray accelerator answers.
    ///
    /// A node pointer is an address over 8 with the node's kind in its low
    /// bits: 0 to 3 a triangle of a node of 5 vertices, 4 a box of 4
    /// children with 16-bit floats, 5 with 32-bit ones (4 child pointers,
    /// then each child's minimum and maximum); the rest is the title's own.
    /// A box gives its children hit, nearest first, then -1; a triangle the
    /// distance and barycentrics i and j, each over the second value
    /// (negative from behind), as the title divides them.
    // shortcut: no watertight edges, no box growing.
    fn intersect(&mut self, words: &[V; 12], guard: Option<V>) -> [V; 4] {
        let k = |l: &mut Self, x: u32| l.s.m.i32(x);
        let kf = |l: &mut Self, x: f32| l.s.m.f32(x.to_bits());
        let [lo, hi, rest @ ..] = *words;
        let f: Vec<V> = rest.iter().map(|&x| self.as_float(x)).collect();
        let extent = f[0];
        let (o, d, inv) = ([f[1], f[2], f[3]], [f[4], f[5], f[6]], [f[7], f[8], f[9]]);
        let c = self.cur;
        let (k3, k7, k29, not7, ones) = (k(self, 3), k(self, 7), k(self, 29), k(self, !7), k(self, u32::MAX));
        let kind = self.s.m.bin(c, Bin::And, lo, k7);
        let base = self.s.m.bin(c, Bin::And, lo, not7);
        let alo = self.s.m.bin(c, Bin::Shl, base, k3);
        let ahi = self.s.m.bin(c, Bin::Shl, hi, k3);
        let up = self.s.m.bin(c, Bin::LShr, lo, k29);
        let ahi = self.s.m.bin(c, Bin::Or, ahi, up);
        let (k4, k5, k6) = (k(self, 4), k(self, 5), k(self, 6));
        let and = |l: &mut Self, a: V, b: V| {
            let c = l.cur;
            l.s.m.bin(c, Bin::And, a, b)
        };
        let boxes = self.s.m.bin(c, Bin::And, kind, k6);
        let is_box = self.s.m.cmp(c, pred::EQ, boxes, k4);
        let is_tri = self.s.m.cmp(c, pred::IULT, kind, k4);
        let (is_box, is_tri) = match guard {
            Some(g) => (and(self, is_box, g), and(self, is_tri, g)),
            None => (is_box, is_tri),
        };

        // Boxes.
        let place = self.begin_guarded(Some(is_box));
        let node = self.memory_dwords((alo, ahi), 28, true);
        let c = self.cur;
        let wide = self.s.m.cmp(c, pred::EQ, kind, k5);
        let (k16, kffff) = (k(self, 16), k(self, 0xFFFF));
        let f32t = self.s.t.f32;
        let mut bounds = Vec::new();
        for n in 0..24usize {
            let full = self.as_float(node[4 + n]);
            let w = node[4 + n / 2];
            let h = if n % 2 == 1 { self.s.m.bin(c, Bin::LShr, w, k16) } else { w };
            let h = self.s.m.bin(c, Bin::And, h, kffff);
            let half = self.s.op(c, "legacyF16ToF32", Attr::ReadNone, f32t, 131, &[h]);
            bounds.push(self.s.m.select(c, wide, full, half));
        }
        let (zero, far) = (kf(self, 0.0), kf(self, f32::INFINITY));
        let mut hits: Vec<(V, V)> = Vec::new();
        for child in 0..4usize {
            let (mut near_all, mut far_all) = (zero, extent);
            for axis in 0..3usize {
                let side = |l: &mut Self, which: usize| {
                    let x = bounds[6 * child + 3 * which + axis];
                    let x = l.s.m.bin(c, Bin::Sub, x, o[axis]);
                    l.s.m.bin(c, Bin::Mul, x, inv[axis])
                };
                let (a, b) = (side(self, 0), side(self, 1));
                let (n, x) = (self.binary(36, a, b), self.binary(35, a, b));
                near_all = self.binary(35, near_all, n);
                far_all = self.binary(36, far_all, x);
            }
            let pointer = node[child];
            let there = self.s.m.cmp(c, pred::NE, pointer, ones);
            let meets = self.s.m.cmp(c, pred::OLE, near_all, far_all);
            let hit = self.s.m.bin(c, Bin::And, there, meets);
            let p = self.s.m.select(c, hit, pointer, ones);
            let t = self.s.m.select(c, hit, near_all, far);
            hits.push((p, t));
        }
        for (a, b) in [(0, 1), (2, 3), (0, 2), (1, 3), (1, 2)] {
            let ((pa, ta), (pb, tb)) = (hits[a], hits[b]);
            // Misses last, also behind hits infinitely far.
            let later = self.s.m.cmp(c, pred::OGT, ta, tb);
            let (pn, pf) = (self.s.m.select(c, later, pb, pa), self.s.m.select(c, later, pa, pb));
            let (tn, tf) = (self.s.m.select(c, later, tb, ta), self.s.m.select(c, later, ta, tb));
            hits[a] = (pn, tn);
            hits[b] = (pf, tf);
        }
        let found: Vec<(V, V)> = hits.iter().map(|h| (h.0, ones)).collect();
        let r = self.end_guarded(place, &found);

        // Triangles.
        let place = self.begin_guarded(Some(is_tri));
        let node = self.memory_dwords((alo, ahi), 16, true);
        let c = self.cur;
        let vertex = |l: &mut Self, v: usize, axis: usize| l.as_float(node[3 * v + axis]);
        // The vertices of each of the node's triangles.
        const VERTICES: [[usize; 3]; 4] = [[0, 1, 2], [1, 3, 2], [2, 3, 4], [2, 4, 0]];
        let is: Vec<V> = (0..3u32)
            .map(|n| {
                let n = k(self, n);
                self.s.m.cmp(c, pred::EQ, kind, n)
            })
            .collect();
        let mut v = [[zero; 3]; 3];
        for (corner, row) in v.iter_mut().enumerate() {
            for (axis, x) in row.iter_mut().enumerate() {
                let mut at = vertex(self, VERTICES[3][corner], axis);
                for n in (0..3).rev() {
                    let other = vertex(self, VERTICES[n][corner], axis);
                    at = self.s.m.select(c, is[n], other, at);
                }
                *x = at;
            }
        }
        let sub = |l: &mut Self, a: [V; 3], b: [V; 3]| [0, 1, 2].map(|i| l.s.m.bin(c, Bin::Sub, a[i], b[i]));
        let cross = |l: &mut Self, a: [V; 3], b: [V; 3]| {
            [(1, 2), (2, 0), (0, 1)].map(|(i, j)| {
                let x = l.s.m.bin(c, Bin::Mul, a[i], b[j]);
                let y = l.s.m.bin(c, Bin::Mul, a[j], b[i]);
                l.s.m.bin(c, Bin::Sub, x, y)
            })
        };
        let dot = |l: &mut Self, a: [V; 3], b: [V; 3]| {
            let x = l.s.m.bin(c, Bin::Mul, a[0], b[0]);
            let x = l.fmad(a[1], b[1], x);
            l.fmad(a[2], b[2], x)
        };
        let (e1, e2) = (sub(self, v[1], v[0]), sub(self, v[2], v[0]));
        let p = cross(self, d, e2);
        let det = dot(self, e1, p);
        let tv = sub(self, o, v[0]);
        let b1 = dot(self, tv, p);
        let q = cross(self, tv, e1);
        let b2 = dot(self, d, q);
        let t = dot(self, e2, q);
        let rest = self.s.m.bin(c, Bin::Sub, det, b1);
        let b0 = self.s.m.bin(c, Bin::Sub, rest, b2);
        // All on the side of the determinant.
        let back = self.s.m.cmp(c, pred::OLT, det, zero);
        let mut inside = self.s.m.cmp(c, pred::ONE, det, zero);
        for x in [b0, b1, b2, t] {
            let neg = self.s.m.bin(c, Bin::Sub, zero, x);
            let x = self.s.m.select(c, back, neg, x);
            let ok = self.s.m.cmp(c, pred::OGE, x, zero);
            inside = self.s.m.bin(c, Bin::And, inside, ok);
        }
        // Which barycentrics i and j are: 8 bits per triangle.
        let shift = self.s.m.bin(c, Bin::Shl, kind, k3);
        let id = self.s.m.bin(c, Bin::LShr, node[15], shift);
        let (one, two) = (k(self, 1), k(self, 2));
        let pick = |l: &mut Self, id: V| {
            let n = l.s.m.bin(c, Bin::And, id, k3);
            let first = l.s.m.cmp(c, pred::EQ, n, one);
            let second = l.s.m.cmp(c, pred::EQ, n, two);
            let x = l.s.m.select(c, second, b2, b0);
            l.s.m.select(c, first, b1, x)
        };
        let i = pick(self, id);
        let id = self.s.m.bin(c, Bin::LShr, id, two);
        let j = pick(self, id);
        let unit = kf(self, 1.0);
        let out = [(t, far), (det, unit), (i, zero), (j, zero)].map(|(x, miss)| {
            let x = self.s.m.select(c, inside, x, miss);
            self.as_int(x)
        });
        let found: Vec<(V, V)> = out.iter().zip(&r).map(|(&x, &before)| (x, before)).collect();
        let r = self.end_guarded(place, &found);
        [r[0], r[1], r[2], r[3]]
    }

    /// The host's view of a buffer whose descriptor the code makes, the byte
    /// its base is at, and its stride.
    fn made_buffer(&mut self, inst: &Inst, rsrc: &Operand) -> Result<(V, V, V), Why> {
        let Operand::Scalar { code, .. } = *rsrc else {
            return Err("a buffer descriptor");
        };
        let word = |l: &mut Self, k: u8| -> Result<V, Why> {
            let v = inst.read(Reg::S(code + k)).ok_or("a buffer descriptor")?;
            l.i32_of(v)
        };
        let (w0, w1) = (word(self, 0)?, word(self, 1)?);
        let c = self.cur;
        let (k16, k3fff) = (self.s.m.i32(16), self.s.m.i32(0x3FFF));
        let s = self.s.m.bin(c, Bin::LShr, w1, k16);
        let stride = self.s.m.bin(c, Bin::And, s, k3fff);
        let (h, base) = self.memory(w0, w1, false);
        Ok((h, base, stride))
    }

    /// Both dwords `(lo, hi)` of a lane mask's ballot, taken in the block
    /// that writes the mask (see [`MaskData`]).
    fn mask_ballot(&mut self, lo: Value) -> Result<(V, V), Why> {
        let &m = self.x.st.masks.get(&lo).ok_or(LANE_MASK)?;
        if self.console_cf {
            // Every lane runs every block: the mask's bits are the lanes'
            // own, both halves' words exchanged.
            if let Some(&xy) = self.ballots.get(&lo) {
                return Ok(xy);
            }
            let at = match self.def_block.get(&lo) {
                None => self.s.entry,
                Some(&b) if b == self.cur_ir => self.cur,
                Some(&b) => self.tail[b].ok_or("a lane mask written after its use")?,
            };
            let bit = self.boolean(m, at)?;
            let xy = self.console_ballot(at, bit);
            self.ballots.insert(lo, xy);
            return Ok(xy);
        }
        if self.split {
            return Err("a lane mask read whole in a wave64 program split in halves");
        }
        if !self.x.st.active.contains(&lo) {
            if std::env::var_os("VICTORY_WHY").is_some() {
                let def = self.def_block.get(&lo).map(|&b| (b, self.x.p.blocks[b].insts.iter().find(|i| i.writes.iter().any(|w| w.1 == lo)).map(|i| (i.machine.offset, i.name().to_owned())), self.x.p.blocks[b].phis.iter().find(|p| p.value == lo).map(|p| (p.reg, p.args.clone()))));
                eprintln!("mask {lo:?} {m:?} def {def:?} cur {}", self.cur_ir);
            }
            return Err("a lane mask holding lanes elsewhere read as data");
        }
        if let Some(&xy) = self.ballots.get(&lo) {
            return Ok(xy);
        }
        let at = match self.def_block.get(&lo) {
            // Program inputs (EXEC on entry).
            None => self.s.entry,
            Some(&b) if b == self.cur_ir => self.cur,
            Some(&b) => self.tail[b].ok_or("a lane mask written after its use")?,
        };
        if !self.balloting.insert(lo) {
            return Err("a lane mask whose lanes depend on its own value");
        }
        let bit = self.boolean(m, at);
        self.balloting.remove(&lo);
        let bit = bit?;
        let i32t = self.s.t.i32;
        let four = self.s.m.named_struct("dx.types.fouri32", vec![i32t; 4]);
        let r = self
            .s
            .op(at, "waveActiveBallot", Attr::NoUnwind, four, 116, &[bit]);
        self.s.flags |= flags::WAVE_OPS;
        let xy = (self.s.m.extract(at, r, 0), self.s.m.extract(at, r, 1));
        self.ballots.insert(lo, xy);
        Ok(xy)
    }

    /// A dword of a lane mask as a number: the ballot of the lanes' bits,
    /// taken in the block that writes the mask (see [`MaskData`]).
    fn mask_dword(&mut self, v: Value) -> Result<V, Why> {
        let (lo, high) = match self.x.st.highs.get(&v) {
            Some(&lo) => (lo, true),
            None => (v, false),
        };
        let (x, y) = self.mask_ballot(lo)?;
        Ok(if high { y } else { x })
    }

    /// Whether some lane of the mask with low dword `lo` is set, as data
    /// (SCC of the operation writing it): its ballot is not zero.
    fn mask_any(&mut self, lo: Value) -> Result<V, Why> {
        // When `lo` is the new EXEC written in block `b` (e.g. `s_*_wrexec_*` or
        // `s_*_saveexec_*`) and `b` exits via `s_cbranch_execz`/`execnz` without
        // rewriting EXEC, a block entered solely through one branch edge knows
        // whether `lo != 0` without balloting lanes from elsewhere.
        if let Some(&b) = self.def_block.get(&lo) {
            let blk = &self.x.p.blocks[b];
            if let crate::ir::Exit::Branch { taken, not_taken } = blk.exit
                && taken != not_taken
                && self.x.p.blocks[self.cur_ir].preds.as_slice() == [b]
                && let Some((idx, def)) = blk
                    .insts
                    .iter()
                    .enumerate()
                    .find(|(_, inst)| inst.writes.iter().any(|&(_, w)| w == lo))
                && (def.write(Reg::S(EXEC)) == Some(lo) || def.name().contains("_wrexec_"))
                && !blk.insts[idx + 1..]
                    .iter()
                    .any(|inst| inst.write(Reg::S(EXEC)).is_some())
                && let Some(term) = blk.insts.last()
            {
                let nonzero = match term.name() {
                    "s_cbranch_execnz" => Some(self.cur_ir == taken),
                    "s_cbranch_execz" => Some(self.cur_ir == not_taken),
                    _ => None,
                };
                if let Some(bit) = nonzero {
                    return Ok(self.s.m.int_const(self.s.t.i1, i64::from(bit)));
                }
            }
        }
        let x = self.mask_dword(lo)?;
        let (_, y) = self.ballots[&lo];
        let c = self.cur;
        let any = if self.x.p.wave64 {
            self.s.m.bin(c, Bin::Or, x, y)
        } else {
            x
        };
        let z = self.s.m.i32(0);
        Ok(self.s.m.cmp(c, pred::NE, any, z))
    }

    /// For a wave32 program under [`MaskData::Host`], a 32-bit scalar operand
    /// holding a tracked mask still represents a host-wave ballot, so its
    /// `(lo, hi)` pair must be read to preserve lanes 32-63 on wave64 hosts.
    fn host_mask_pair(&mut self, inst: &Inst, op: &Operand) -> Result<Option<(V, V)>, Why> {
        if self.mask_data != MaskData::Host || self.x.native.wave64 {
            return Ok(None);
        }
        let Operand::Scalar { code, .. } = *op else {
            return Ok(None);
        };
        if code == NULL {
            return Ok(None);
        }
        let Some(v) = inst.read(Reg::S(code)) else {
            return Ok(None);
        };
        if matches!(self.vals.get(&v), Some(Val::I32(_) | Val::Bit(_)))
            || !self.x.st.masks.contains_key(&v)
        {
            return Ok(None);
        }
        self.mask_ballot(v).map(Some)
    }

    fn is_zero_operand(op: &Operand) -> bool {
        matches!(op, Operand::Scalar { code: NULL, .. } | Operand::Literal(0))
            || matches!(op, Operand::Const(c) if crate::resource::inline(*c) == Some(0))
    }

    fn bcnt1_64(&mut self, c: usize, a: V, b: V) -> V {
        let i32t = self.s.t.i32;
        let na = self
            .s
            .op(c, "unaryBits.i32", Attr::ReadNone, i32t, 31, &[a]);
        let nb = self
            .s
            .op(c, "unaryBits.i32", Attr::ReadNone, i32t, 31, &[b]);
        self.s.m.bin(c, Bin::Add, na, nb)
    }

    fn ff1_64(&mut self, c: usize, a: V, b: V) -> V {
        // The lowest set bit; -1 for none (as DXIL's first bit low).
        let i32t = self.s.t.i32;
        let fa = self
            .s
            .op(c, "unaryBits.i32", Attr::ReadNone, i32t, 32, &[a]);
        let fb = self
            .s
            .op(c, "unaryBits.i32", Attr::ReadNone, i32t, 32, &[b]);
        let (z, k32) = (self.s.m.i32(0), self.s.m.i32(32));
        let fb32 = self.s.m.bin(c, Bin::Add, fb, k32);
        let none = self.s.m.cmp(c, pred::EQ, b, z);
        let fb = self.s.m.select(c, none, fb, fb32);
        let low = self.s.m.cmp(c, pred::NE, a, z);
        self.s.m.select(c, low, fa, fb)
    }

    fn eq_64(&mut self, c: usize, (a0, a1): (V, V), (b0, b1): (V, V)) -> V {
        let e0 = self.s.m.cmp(c, pred::EQ, a0, b0);
        let e1 = self.s.m.cmp(c, pred::EQ, a1, b1);
        self.s.m.bin(c, Bin::And, e0, e1)
    }

    /// `(1 << below) - 1`, all ones from 32 on.
    fn below_mask(&mut self, c: usize, below: V) -> V {
        let (one, k31, k32, ones) = (
            self.s.m.i32(1),
            self.s.m.i32(31),
            self.s.m.i32(32),
            self.s.m.i32(u32::MAX),
        );
        let sh = self.s.m.bin(c, Bin::And, below, k31);
        let m = self.s.m.bin(c, Bin::Shl, one, sh);
        let m = self.s.m.bin(c, Bin::Sub, m, one);
        let full = self.s.m.cmp(c, pred::IUGE, below, k32);
        self.s.m.select(c, full, ones, m)
    }

    // ---- operands ----

    fn operand(&mut self, inst: &Inst, op: &Operand) -> Result<V, Why> {
        match *op {
            Operand::Scalar { code: NULL, .. } => Ok(self.s.m.i32(0)),
            Operand::Scalar { code, .. } => {
                let v = inst.read(Reg::S(code)).ok_or("an unread operand")?;
                self.i32_of(v)
            }
            Operand::Vector { reg, .. } => {
                let v = inst.read(Reg::V(reg)).ok_or("an unread operand")?;
                self.i32_of(v)
            }
            // RDNA2 `src_pops_exiting_wave_id`: POPS spin-wait counter (bypassed
            // on host by zeroing the POPS overlap SGPR at PS entry).
            Operand::Const(239) => Ok(self.s.m.i32(0)),
            // SCC as a number.
            Operand::Const(253) => {
                let bit = self.scc_in(inst)?;
                let c = self.cur;
                let (one, zero) = (self.s.m.i32(1), self.s.m.i32(0));
                Ok(self.s.m.select(c, bit, one, zero))
            }
            Operand::Const(c) => crate::resource::inline(c)
                .map(|x| self.s.m.i32(x))
                .ok_or("a special operand (vccz, execz, scc, lds_direct)"),
            Operand::Literal(x) => Ok(self.s.m.i32(x)),
            Operand::Attr { .. } => Err("an attribute operand"),
        }
    }

    /// An operand read as f16 data: an inline float constant is its f16
    /// value in both halves (so op_sel picks either), not the f32 bits
    /// `operand` gives, whose low half is 0.
    fn f16_operand(&mut self, inst: &Inst, op: &Operand) -> Result<V, Why> {
        match *op {
            Operand::Const(c @ 240..=248) => Ok(self.s.m.i32(inline_f16(c))),
            _ => self.operand(inst, op),
        }
    }

    fn as_float(&mut self, x: V) -> V {
        self.s.m.cast(self.cur, Cast::Bitcast, x, self.s.t.f32)
    }

    fn as_int(&mut self, f: V) -> V {
        self.s.m.cast(self.cur, Cast::Bitcast, f, self.s.t.i32)
    }

    fn sign_extend(&mut self, x: V, bits: u32) -> V {
        let n = self.s.m.i32(32 - bits);
        let x = self.s.m.bin(self.cur, Bin::Shl, x, n);
        self.s.m.bin(self.cur, Bin::AShr, x, n)
    }

    fn pack_halves(&mut self, lo: V, hi: V) -> V {
        let mask = self.s.m.i32(0xFFFF);
        let k16 = self.s.m.i32(16);
        let lo = self.s.m.bin(self.cur, Bin::And, lo, mask);
        let hi = self.s.m.bin(self.cur, Bin::Shl, hi, k16);
        self.s.m.bin(self.cur, Bin::Or, lo, hi)
    }

    fn int_pick(&mut self, p: u8, a: V, b: V) -> V {
        let choose = self.s.m.cmp(self.cur, p, a, b);
        self.s.m.select(self.cur, choose, a, b)
    }

    fn clamp_int(&mut self, x: V, bits: u32, min: i64, max: i64, signed: bool) -> V {
        let t = self.s.m.int(bits);
        let (min, max) = (self.s.m.int_const(t, min), self.s.m.int_const(t, max));
        let (ge, le) = if signed { (pred::SGE, pred::SLE) } else { (pred::IUGE, pred::IULE) };
        let x = self.int_pick(ge, x, min);
        self.int_pick(le, x, max)
    }

    /// The SDWA operand selection of source `k`: a byte or word of the
    /// dword, zero- or sign-extended (none: the whole dword).
    fn sdwa_src(inst: &Inst, k: usize) -> Option<(u8, bool)> {
        let Fields::Valu(v) = &inst.fields else {
            return None;
        };
        let s = v.sdwa?;
        let sel = *s.src_sel.get(k)?;
        let sext = s.sext >> k & 1 != 0;
        (sel < 6).then_some((sel, sext))
    }

    /// Bits `sel` (0-3 a byte, 4-5 a word) of `x`, extended.
    fn select_bits(&mut self, x: V, sel: u8, sext: bool) -> V {
        let c = self.cur;
        let (width, at) = if sel < 4 {
            (8, 8 * sel)
        } else {
            (16, 16 * (sel - 4))
        };
        let left = self.s.m.i32(u32::from(32 - width - at));
        let t = self.s.m.bin(c, Bin::Shl, x, left);
        let right = self.s.m.i32(32 - u32::from(width));
        let op = if sext { Bin::AShr } else { Bin::LShr };
        self.s.m.bin(c, op, t, right)
    }

    /// A float source with its abs/neg modifiers.
    fn fsrc(&mut self, inst: &Inst, k: usize) -> Result<V, Why> {
        let Fields::Valu(v) = &inst.fields else {
            unreachable!()
        };
        let op = *v.src.get(k).ok_or("a missing operand")?;
        let selected = Self::sdwa_src(inst, k);
        let mut f = match op {
            _ if selected.is_some() => {
                let (sel, sext) = selected.expect("checked");
                let x = self.operand(inst, &op)?;
                let x = self.select_bits(x, sel, sext);
                self.as_float(x)
            }
            _ if k == 0 && self.src0.is_some() => {
                let x = self.src0.expect("checked");
                self.as_float(x)
            }
            Operand::Vector { reg, .. } | Operand::Scalar { code: reg, .. }
                if !matches!(op, Operand::Scalar { code: NULL, .. }) =>
            {
                let r = if matches!(op, Operand::Vector { .. }) {
                    Reg::V(reg)
                } else {
                    Reg::S(reg)
                };
                let val = inst.read(r).ok_or("an unread operand")?;
                match self.floats.get(&val) {
                    Some(&f) => f,
                    None => {
                        let x = self.i32_of(val)?;
                        self.as_float(x)
                    }
                }
            }
            _ => {
                let x = self.operand(inst, &op)?;
                self.as_float(x)
            }
        };
        if v.abs >> k & 1 != 0 {
            f = self.unary(6, f);
        }
        if v.neg >> k & 1 != 0 {
            let z = self.s.m.f32(0x8000_0000);
            f = self.s.m.bin(self.cur, Bin::Sub, z, f);
        }
        Ok(f)
    }

    fn src(&mut self, inst: &Inst, k: usize) -> Result<V, Why> {
        let Fields::Valu(v) = &inst.fields else {
            unreachable!()
        };
        let op = *v.src.get(k).ok_or("a missing operand")?;
        if v.abs >> k & 1 != 0 || v.neg >> k & 1 != 0 {
            return Err("integer operand modifiers");
        }
        match self.src0 {
            Some(x) if k == 0 => Ok(x),
            _ => {
                let x = self.operand(inst, &op)?;
                Ok(match Self::sdwa_src(inst, k) {
                    Some((sel, sext)) => self.select_bits(x, sel, sext),
                    None => x,
                })
            }
        }
    }

    fn unary(&mut self, op: u32, f: V) -> V {
        self.s.op(
            self.cur,
            "unary.f32",
            Attr::ReadNone,
            self.s.t.f32,
            op,
            &[f],
        )
    }

    fn binary(&mut self, op: u32, a: V, b: V) -> V {
        self.s.op(
            self.cur,
            "binary.f32",
            Attr::ReadNone,
            self.s.t.f32,
            op,
            &[a, b],
        )
    }

    /// The hardware's maximum (`max`) or minimum of two floats: as DXIL's
    /// FMax/FMin, but -0 orders below +0 (RDNA2 ISA, `v_max_f32`), where
    /// the compare and select DXIL specifies gives either zero.
    fn max_min(&mut self, max: bool, a: V, b: V) -> V {
        let c = self.cur;
        let r = self.binary(if max { 35 } else { 36 }, a, b);
        let (ia, ib) = (self.as_int(a), self.as_int(b));
        let either = self.s.m.bin(c, Bin::Or, ia, ib);
        let magnitude = self.s.m.i32(0x7FFF_FFFF);
        let either = self.s.m.bin(c, Bin::And, either, magnitude);
        let zero = self.s.m.i32(0);
        let zeros = self.s.m.cmp(c, pred::EQ, either, zero);
        // Of two zeros, +0 unless both are -0 (max), -0 unless neither (min).
        let op = if max { Bin::And } else { Bin::Or };
        let sign = self.s.m.bin(c, op, ia, ib);
        let signed = self.as_float(sign);
        self.s.m.select(c, zeros, signed, r)
    }

    fn fmad(&mut self, a: V, b: V, c: V) -> V {
        self.s.op(
            self.cur,
            "tertiary.f32",
            Attr::ReadNone,
            self.s.t.f32,
            46,
            &[a, b, c],
        )
    }

    /// `f` as an f16 in the low bits, rounded toward zero: the nearest-even
    /// conversion, one step back toward zero where it rounded away (which
    /// also turns an overflow to infinity into the largest finite half).
    fn f16_toward_zero(&mut self, f: V) -> V {
        let c = self.cur;
        let i32t = self.s.t.i32;
        let h = self
            .s
            .op(c, "legacyF32ToF16", Attr::ReadNone, i32t, 130, &[f]);
        let back = self
            .s
            .op(c, "legacyF16ToF32", Attr::ReadNone, self.s.t.f32, 131, &[h]);
        let (fa, ba) = (self.unary(6, f), self.unary(6, back));
        // Ordered: never for NaN.
        let away = self.s.m.cmp(c, pred::OGT, ba, fa);
        let one = self.s.m.i32(1);
        let stepped = self.s.m.bin(c, Bin::Sub, h, one);
        self.s.m.select(c, away, stepped, h)
    }

    /// `(a >> off[4:0]) & ((1 << width) - 1)`, sign-extended from the field's
    /// top bit when `signed`; widths from 32 on take every bit. A signed
    /// field that runs past bit 31 extends from bit 31, as the hardware
    /// does: the shift is arithmetic then.
    fn bfe(&mut self, a: V, off: V, width: V, signed: bool) -> V {
        self.bitfield(a, off, width, signed, 32)
    }

    fn bitfield(&mut self, a: V, off: V, width: V, signed: bool, bits: u32) -> V {
        let c = self.cur;
        let ty = self.s.m.int(bits);
        let last = self.s.m.i32(bits - 1);
        let size = self.s.m.i32(bits);
        let zero = self.s.m.i32(0);
        let one = self.s.m.int_const(ty, 1);
        let ones = self.s.m.int_const(ty, -1);
        let off = self.s.m.bin(c, Bin::And, off, last);
        let off = if bits == 64 { self.s.m.cast(c, Cast::ZExt, off, ty) } else { off };
        let shift = if signed { Bin::AShr } else { Bin::LShr };
        let shifted = self.s.m.bin(c, shift, a, off);
        let full = self.s.m.cmp(c, pred::IUGE, width, size);
        let w = self.s.m.bin(c, Bin::And, width, last);
        let count = if bits == 64 { self.s.m.cast(c, Cast::ZExt, w, ty) } else { w };
        let mask = self.s.m.bin(c, Bin::Shl, one, count);
        let mask = self.s.m.bin(c, Bin::Sub, mask, one);
        let mask = self.s.m.select(c, full, ones, mask);
        let r = self.s.m.bin(c, Bin::And, shifted, mask);
        if !signed {
            return r;
        }
        // Sign-extend a field narrower than the register (none for width 0).
        let left = self.s.m.bin(c, Bin::Sub, size, w);
        let left = self.s.m.bin(c, Bin::And, left, last);
        let left = if bits == 64 { self.s.m.cast(c, Cast::ZExt, left, ty) } else { left };
        let t = self.s.m.bin(c, Bin::Shl, r, left);
        let t = self.s.m.bin(c, Bin::AShr, t, left);
        let is0 = self.s.m.cmp(c, pred::EQ, w, zero);
        let keep = self.s.m.bin(c, Bin::Or, full, is0);
        self.s.m.select(c, keep, r, t)
    }

    /// The median of three. With a NaN among them the hardware gives the
    /// minimum of the three instead (the host's minimum skips NaNs, so that
    /// is the smallest number among them).
    ///
    /// Uses `dx.op.isSpecialFloat.f32` (`IsNaN`) instead of `fcmp uno`: some
    /// D3D12 drivers (observed on Intel Iris Xe) fold `fcmp uno x, x` to false
    /// under default shader fast-math, returning the wrong operand when the
    /// third input is NaN.
    fn med3(&mut self, a: V, b: V, third: V) -> V {
        let c = self.cur;
        let lo = self.binary(36, a, b);
        let hi = self.binary(35, a, b);
        let t = self.binary(36, hi, third);
        let median = self.binary(35, lo, t);
        let least = self.binary(36, lo, third);
        let is_nan = |l: &mut Self, x: V| {
            l.s.op(c, "isSpecialFloat.f32", Attr::ReadNone, l.s.t.i1, 8, &[x])
        };
        let nan_a = is_nan(self, a);
        let nan_b = is_nan(self, b);
        let nan_c = is_nan(self, third);
        let ab = self.s.m.bin(c, Bin::Or, nan_a, nan_b);
        let nan = self.s.m.bin(c, Bin::Or, ab, nan_c);
        self.s.m.select(c, nan, least, median)
    }

    /// `a - floor(a)`, which on the hardware never reaches 1.0: where it
    /// would round to it (a tiny negative `a`), the result is `below_one`,
    /// the largest value under 1.0 in the operation's width. Hosts differ
    /// here (some give 1.0). Comparing against a `threshold` distinct from
    /// `below_one` prevents drivers (observed on NVIDIA) from folding
    /// `r > k ? k : r` into `minNum` (`FMNMX`) and swallowing NaN.
    fn fract(&mut self, a: V, threshold: f32, below_one: f32) -> V {
        let c = self.cur;
        let r = self.unary(22, a);
        let limit = self.s.m.f32(threshold.to_bits());
        let k = self.s.m.f32(below_one.to_bits());
        let over = self.s.m.cmp(c, pred::OGE, r, limit);
        self.s.m.select(c, over, k, r)
    }

    /// Sine, or cosine, of an angle in turns, as the hardware takes it. The
    /// whole turns are taken off first (`x - round_even(x)`, exact, within
    /// half a turn), so a large angle keeps its fraction; scaling to radians
    /// first rounds it away. A zero passes unreduced (the sine of -0 is -0).
    /// The sine of a half turn and the cosine of a quarter turn are exactly
    /// +0 on the hardware, where the host's function of the rounded angle
    /// gives about 1e-7. Infinities and NaN give NaN.
    /// shortcut: below 2^-12 turns the hardware scales by a slightly
    /// different constant than 2π (a last-place difference).
    fn trig_turns(&mut self, x: V, cosine: bool) -> V {
        let c = self.cur;
        let zero = self.s.m.f32(0);
        let whole = self.unary(26, x);
        let part = self.s.m.bin(c, Bin::Sub, x, whole);
        let is_zero = self.s.m.cmp(c, pred::OEQ, x, zero);
        let part = self.s.m.select(c, is_zero, x, part);
        let k = self.s.m.f32(TWO_PI.to_bits());
        let angle = self.s.m.bin(c, Bin::Mul, part, k);
        let r = self.unary(if cosine { 12 } else { 13 }, angle);
        let at = if cosine { 0.25f32 } else { 0.5 };
        let at = self.s.m.f32(at.to_bits());
        let size = self.unary(6, part);
        let exact = self.s.m.cmp(c, pred::OEQ, size, at);
        self.s.m.select(c, exact, zero, r)
    }

    /// The high dword of a 32 × 32-bit product.
    fn mul_hi(&mut self, a: V, b: V, signed: bool) -> V {
        let c = self.cur;
        let i64t = self.s.m.int(64);
        let ext = if signed { Cast::SExt } else { Cast::ZExt };
        let a = self.s.m.cast(c, ext, a, i64t);
        let b = self.s.m.cast(c, ext, b, i64t);
        let m = self.s.m.bin(c, Bin::Mul, a, b);
        let k32 = self.s.m.int_const(i64t, 32);
        let hi = self.s.m.bin(c, Bin::LShr, m, k32);
        self.s.flags |= flags::INT64;
        self.s.m.cast(c, Cast::Trunc, hi, self.s.t.i32)
    }

    /// `0 × anything = 0`, as in D3D9.
    fn legacy_mul(&mut self, a: V, b: V) -> V {
        let c = self.cur;
        let m = self.s.m.bin(c, Bin::Mul, a, b);
        let zero = self.s.m.f32(0);
        let az = self.s.m.cmp(c, pred::OEQ, a, zero);
        let bz = self.s.m.cmp(c, pred::OEQ, b, zero);
        let z = self.s.m.bin(c, Bin::Or, az, bz);
        self.s.m.select(c, z, zero, m)
    }

    /// `a × b + addend` fused, with `0 × anything = 0` as in D3D9
    /// (`v_fma_legacy_f32`).
    fn fma_legacy(&mut self, a: V, b: V, addend: V) -> V {
        let c = self.cur;
        let zero = self.s.m.f32(0);
        let az = self.s.m.cmp(c, pred::OEQ, a, zero);
        let bz = self.s.m.cmp(c, pred::OEQ, b, zero);
        let z = self.s.m.bin(c, Bin::Or, az, bz);
        let product_zero = self.s.m.bin(c, Bin::Add, zero, addend);
        // shortcut: FMad lets the driver fuse, as for `v_fma_f32`.
        let fused = self.fmad(a, b, addend);
        self.s.m.select(c, z, product_zero, fused)
    }

    // ---- lane booleans ----

    fn boolean(&mut self, b: Bool, at: usize) -> Result<V, Why> {
        if b == Bool::TRUE || b == Bool::FALSE {
            return Ok(self.s.m.bool(b == Bool::TRUE));
        }
        if let Some(&v) = self.bools.get(&(at, b)) {
            return Ok(v);
        }
        let (atom, lo, hi) = self.x.st.bools.node(b).expect("not a constant");
        let a = self.atom(atom, at)?;
        let m = &mut self.s.m;
        let v = match (lo, hi) {
            (Bool::FALSE, Bool::TRUE) => a,
            (Bool::TRUE, Bool::FALSE) => {
                let t = m.bool(true);
                m.bin(at, Bin::Xor, a, t)
            }
            (Bool::FALSE, h) => {
                let h = self.boolean(h, at)?;
                self.s.m.bin(at, Bin::And, a, h)
            }
            (l, Bool::TRUE) => {
                let l = self.boolean(l, at)?;
                self.s.m.bin(at, Bin::Or, a, l)
            }
            (l, h) => {
                let l = self.boolean(l, at)?;
                let h = self.boolean(h, at)?;
                self.s.m.select(at, a, h, l)
            }
        };
        self.bools.insert((at, b), v);
        Ok(v)
    }

    fn atom(&mut self, atom: Atom, at: usize) -> Result<V, Why> {
        match atom {
            Atom::Lane(v) => self.lane_bits.get(&v).copied().ok_or_else(|| {
                self.lane_fail
                    .get(&v)
                    .copied()
                    .unwrap_or("a lane mask from a carry")
            }),
            Atom::Uniform(v) => match self.get(v)? {
                Val::Bit(x) => Ok(x),
                Val::I32(x) => {
                    let z = self.s.m.i32(0);
                    Ok(self.s.m.cmp(at, pred::NE, x, z))
                }
                Val::Missing(w) => Err(w),
            },
            Atom::Phi(id) => {
                if let Some(&v) = self.mask_phis.get(&id) {
                    return Ok(v);
                }
                let &(b, i) = self
                    .phi_at
                    .get(&id)
                    .ok_or("a lane-mask join without a phi")?;
                let blk = self.block[b].ok_or("an unreachable join")?;
                let v = self.s.m.phi(blk, self.s.t.i1);
                self.mask_phis.insert(id, v);
                self.pending_masks.push((v, b, i));
                Ok(v)
            }
            // shortcut: "some lane of the quad is set" is taken as true: the
            // lanes it adds only feed derivatives, and exports keep their own
            // masks. Wrong if such a section had a side effect.
            Atom::Quad(_) => Ok(self.s.m.bool(true)),
            // With console lanes, a mask held as a number is the console's:
            // the lane tests its own bit.
            Atom::Opaque(v) if self.console_lanes => {
                let hi = match self.pair_high(v) {
                    Some(h) => Some(self.get(h)?),
                    None => None,
                };
                let lo = self.get(v)?;
                let word = |w: Val| match w {
                    Val::I32(x) => Ok(x),
                    _ => Err("an opaque lane mask"),
                };
                let lo = word(lo)?;
                let hi = hi.map(word).transpose()?;
                Ok(self.lane_bit(at, lo, hi))
            }
            Atom::Constant(k) if self.console_lanes => {
                let lo = self.s.m.i32(k as u32);
                let hi = self.s.m.i32((k >> 32) as u32);
                Ok(self.lane_bit(at, lo, Some(hi)))
            }
            Atom::Opaque(_) => Err("an opaque lane mask"),
            Atom::Constant(_) => Err("a constant lane mask"),
        }
    }

    /// This lane's bit of the mask `lo`, `hi` (wave64; host lane = console
    /// lane, so not in a program split in halves).
    fn lane_bit(&mut self, at: usize, lo: V, hi: Option<V>) -> V {
        debug_assert!(!self.split || self.console_cf);
        if self.console_cf {
            // The console lane's bit: its half's word, its host lane's bit.
            let (lane, half) = self.console_lane();
            let (k31, one, z) = (self.s.m.i32(31), self.s.m.i32(1), self.s.m.i32(0));
            let word = match hi {
                Some(hi) => {
                    let low = self.s.m.cmp(at, pred::EQ, half, z);
                    self.s.m.select(at, low, lo, hi)
                }
                None => lo,
            };
            let sh = self.s.m.bin(at, Bin::And, lane, k31);
            let t = self.s.m.bin(at, Bin::LShr, word, sh);
            let t = self.s.m.bin(at, Bin::And, t, one);
            return self.s.m.cmp(at, pred::NE, t, z);
        }
        let lane = self.s.op(
            at,
            "waveGetLaneIndex",
            Attr::ReadOnly,
            self.s.t.i32,
            111,
            &[],
        );
        let (k31, k32, one, z) = (
            self.s.m.i32(31),
            self.s.m.i32(32),
            self.s.m.i32(1),
            self.s.m.i32(0),
        );
        let word = match hi {
            Some(hi) => {
                let low = self.s.m.cmp(at, pred::IULT, lane, k32);
                self.s.m.select(at, low, lo, hi)
            }
            None => lo,
        };
        let sh = self.s.m.bin(at, Bin::And, lane, k31);
        let t = self.s.m.bin(at, Bin::LShr, word, sh);
        let t = self.s.m.bin(at, Bin::And, t, one);
        self.s.m.cmp(at, pred::NE, t, z)
    }

    fn select_condition(&mut self, inst: &Inst, mask: Option<&Operand>) -> Result<V, Why> {
        if let Some(&m) = inst
            .writes
            .first()
            .and_then(|&(_, v)| self.x.st.select_masks.get(&v))
        {
            return self.boolean(m, self.cur);
        }
        let Some(&Operand::Scalar { code, .. }) = mask else {
            return Err(UNTRACKED_SELECT);
        };
        if code == NULL {
            return Ok(self.s.m.bool(false));
        }
        if !self.numeric_selects || !self.console_lanes {
            return Err(UNTRACKED_SELECT);
        }
        // A scalar operation can define either dword independently. The SSA
        // operands at this use give the actual pair, even across a join.
        let lo = self.i32_of(inst.read(Reg::S(code)).ok_or(UNTRACKED_SELECT)?)?;
        let hi = if self.x.p.wave64 {
            Some(self.i32_of(inst.read(Reg::S(code + 1)).ok_or(UNTRACKED_SELECT)?)?)
        } else {
            None
        };
        self.s.flags |= flags::WAVE_OPS;
        Ok(self.lane_bit(self.cur, lo, hi))
    }

    /// The value of the high dword of the wave64 register pair whose low
    /// dword holds `lo` (none in wave32).
    fn pair_high(&self, lo: Value) -> Option<Value> {
        if !self.x.native.wave64 {
            return None;
        }
        let p = self.x.p;
        let pick = |reg: Reg| match reg {
            Reg::S(c) => Some(Reg::S(c + 1)),
            _ => None,
        };
        if let Some(&(reg, _)) = p.inputs.iter().find(|i| i.1 == lo) {
            let h = pick(reg)?;
            return p.inputs.iter().find(|i| i.0 == h).map(|i| i.1);
        }
        if let Some(&(b, i)) = self.phi_at.get(&lo) {
            let h = pick(p.blocks[b].phis[i].reg)?;
            return p.blocks[b]
                .phis
                .iter()
                .find(|q| q.reg == h)
                .map(|q| q.value);
        }
        let b = *self.def_block.get(&lo)?;
        let inst = p.blocks[b]
            .insts
            .iter()
            .find(|i| i.writes.iter().any(|w| w.1 == lo))?;
        let &(reg, _) = inst.writes.iter().find(|w| w.1 == lo)?;
        inst.write(pick(reg)?)
    }

    fn guard(&mut self, b: usize, i: usize) -> Result<Option<V>, Why> {
        if self.console_cf {
            // Every lane is here: EXEC is the guard. Lane reads and writes
            // ignore EXEC on the console.
            let inst = &self.x.p.blocks[b].insts[i];
            let ignores = matches!(
                inst.name(),
                "v_readlane_b32" | "v_writelane_b32" | "v_readfirstlane_b32"
            ) || matches!(
                inst.fields,
                Fields::Sop { .. } | Fields::Sopp { .. } | Fields::Smem { .. }
            );
            return match self.x.st.execs[b].get(i).copied().flatten() {
                Some(e) if !ignores && e != Bool::TRUE => Ok(Some(self.boolean(e, self.cur)?)),
                _ => Ok(None),
            };
        }
        match self.x.st.guards[b][i] {
            None => Ok(None),
            Some(g) => Ok(Some(self.boolean(g, self.cur)?)),
        }
    }

    // ---- results ----

    fn set(&mut self, v: Option<Value>, val: Val) {
        if let Some(v) = v {
            self.vals.insert(v, val);
        }
    }

    /// Writes VGPR `reg` (or the SGPR of a lane read), under `guard`.
    fn write(&mut self, inst: &Inst, reg: Reg, x: V, guard: Option<V>) {
        let x = match (Self::sdwa_dst(inst), reg) {
            (Some((sel, unused)), Reg::V(_)) => self.place_bits(inst, reg, x, sel, unused),
            _ => x,
        };
        self.write_placed(inst, reg, x, guard);
    }

    /// [`Self::write`] of a dword already placed (SDWA applied).
    fn write_placed(&mut self, inst: &Inst, reg: Reg, x: V, guard: Option<V>) {
        let Some(new) = inst.write(reg) else { return };
        let x = match (guard, reg) {
            (Some(g), Reg::V(_)) => {
                let old = match inst.read(reg).map(|o| self.i32_of(o)) {
                    Some(Ok(o)) => o,
                    // The validator refuses reads of undef.
                    _ => self.s.m.i32(0),
                };
                self.s.m.select(self.cur, g, x, old)
            }
            _ => x,
        };
        self.vals.insert(new, Val::I32(x));
    }

    /// The SDWA destination selection: a byte or word, and what the other
    /// bits get (0 zeros, 1 the sign above the field, 2 the old bits).
    fn sdwa_dst(inst: &Inst) -> Option<(u8, u8)> {
        let Fields::Valu(v) = &inst.fields else {
            return None;
        };
        let s = v.sdwa?;
        (s.dst_sel < 6).then_some((s.dst_sel, s.dst_unused))
    }

    /// The low byte or word of `x` placed at `sel` in VGPR `reg`.
    fn place_bits(&mut self, inst: &Inst, reg: Reg, x: V, sel: u8, unused: u8) -> V {
        let c = self.cur;
        let (width, at) = if sel < 4 {
            (8u32, 8 * u32::from(sel))
        } else {
            (16, 16 * u32::from(sel - 4))
        };
        let left = self.s.m.i32(32 - width);
        let t = self.s.m.bin(c, Bin::Shl, x, left);
        let right = self.s.m.i32(32 - width - at);
        if unused == 1 {
            // Bits below the field shift in as zeros.
            return self.s.m.bin(c, Bin::AShr, t, right);
        }
        let field = self.s.m.bin(c, Bin::LShr, t, right);
        if unused != 2 {
            return field;
        }
        let old = match inst.read(reg).map(|o| self.i32_of(o)) {
            Some(Ok(o)) => o,
            _ => self.s.m.i32(0),
        };
        let keep = self.s.m.i32(!((((1u64 << width) - 1) as u32) << at));
        let old = self.s.m.bin(c, Bin::And, old, keep);
        self.s.m.bin(c, Bin::Or, old, field)
    }

    fn write_f(&mut self, inst: &Inst, f: V, guard: Option<V>) -> Result<(), Why> {
        let Fields::Valu(v) = &inst.fields else {
            unreachable!()
        };
        let mut f = f;
        match v.omod {
            0 => {}
            k => {
                let factor = [1.0f32, 2.0, 4.0, 0.5][usize::from(k)];
                let c = self.s.m.f32(factor.to_bits());
                f = self.s.m.bin(self.cur, Bin::Mul, f, c);
            }
        }
        if v.clamp {
            f = self.unary(7, f);
        }
        let Some(Operand::Vector { reg, .. }) = v.dst else {
            return Err("a float result outside a VGPR");
        };
        let x = self.as_int(f);
        self.write(inst, Reg::V(reg), x, guard);
        if guard.is_none()
            && Self::sdwa_dst(inst).is_none()
            && let Some(new) = inst.write(Reg::V(reg))
        {
            self.floats.insert(new, f);
        }
        Ok(())
    }

    fn write_i(&mut self, inst: &Inst, x: V, guard: Option<V>) -> Result<(), Why> {
        let Fields::Valu(v) = &inst.fields else {
            unreachable!()
        };
        if v.clamp || v.omod != 0 {
            return Err("integer output modifiers");
        }
        match v.dst {
            Some(Operand::Vector { reg, .. }) => self.write(inst, Reg::V(reg), x, guard),
            Some(Operand::Scalar { code, .. }) => self.write(inst, Reg::S(code), x, None),
            _ => return Err("a result without a register"),
        }
        Ok(())
    }

    /// Starts code only the lanes where `guard` holds run (a store under a
    /// lane guard): returns where it started and the join block.
    fn begin_guarded(&mut self, guard: Option<V>) -> Option<(usize, usize)> {
        let g = guard?;
        let (then, join) = (self.s.m.new_block(), self.s.m.new_block());
        self.s.m.terminate(self.cur, Term::CondBr(g, then, join));
        let head = self.cur;
        self.cur = then;
        Some((head, join))
    }

    /// Ends guarded code; each (value inside, value outside) becomes a phi.
    fn end_guarded(&mut self, at: Option<(usize, usize)>, values: &[(V, V)]) -> Vec<V> {
        let Some((head, join)) = at else {
            return values.iter().map(|v| v.0).collect();
        };
        let then = self.cur;
        self.s.m.terminate(then, Term::Br(join));
        self.cur = join;
        values
            .iter()
            .map(|&(inside, outside)| {
                let ty = self.s.m.type_of(inside);
                let phi = self.s.m.phi(join, ty);
                self.s.m.set_phi(phi, vec![(inside, then), (outside, head)]);
                phi
            })
            .collect()
    }

    /// Atomically updates one location. The first compare also obtains the
    /// initial value, so every observation is atomic, even when it is i64.
    fn atomic_update(&mut self, zero: V, exchange: impl FnOnce(&mut Self, V) -> V) -> V {
        let head = self.cur;
        let retry = self.s.m.new_block();
        let done = self.s.m.new_block();
        self.s.m.terminate(head, Term::Br(retry));
        self.cur = retry;
        let ty = self.s.m.type_of(zero);
        let expected = self.s.m.phi(retry, ty);
        let old = exchange(self, expected);
        let success = self.s.m.cmp(retry, pred::EQ, expected, old);
        self.s.m.set_phi(expected, vec![(zero, head), (old, retry)]);
        self.s.m.terminate(retry, Term::CondBr(success, done, retry));
        self.cur = done;
        old
    }

    /// AMD RDNA 2 ISA sections 12.13–12.14 (including DS_WRAP).
    fn atomic_new_value(&mut self, op: AtomicUpdate, old: V, data: V, data2: V) -> V {
        use AtomicUpdate::*;
        let c = self.cur;
        let ty = self.s.m.type_of(old);
        let zero = self.s.m.int_const(ty, 0);
        let one = self.s.m.int_const(ty, 1);
        match op {
            Inc => {
                let wrap = self.s.m.cmp(c, pred::IUGE, old, data);
                let next = self.s.m.bin(c, Bin::Add, old, one);
                self.s.m.select(c, wrap, zero, next)
            }
            Dec => {
                let at_zero = self.s.m.cmp(c, pred::EQ, old, zero);
                let above = self.s.m.cmp(c, pred::IUGT, old, data);
                let wrap = self.s.m.bin(c, Bin::Or, at_zero, above);
                let next = self.s.m.bin(c, Bin::Sub, old, one);
                self.s.m.select(c, wrap, data, next)
            }
            ReverseSub => self.s.m.bin(c, Bin::Sub, data, old),
            MaskOr => {
                let ones = self.s.m.int_const(ty, -1);
                let keep = self.s.m.bin(c, Bin::Xor, data, ones);
                let kept = self.s.m.bin(c, Bin::And, old, keep);
                self.s.m.bin(c, Bin::Or, kept, data2)
            }
            Wrap => {
                let wrap = self.s.m.cmp(c, pred::IUGE, old, data);
                let low = self.s.m.bin(c, Bin::Sub, old, data);
                let high = self.s.m.bin(c, Bin::Add, old, data2);
                self.s.m.select(c, wrap, low, high)
            }
            ClampSub => {
                let underflow = self.s.m.cmp(c, pred::IULT, old, data);
                let diff = self.s.m.bin(c, Bin::Sub, old, data);
                self.s.m.select(c, underflow, zero, diff)
            }
        }
    }

    fn byte_shift(&mut self, address: V) -> V {
        let three = self.s.m.i32(3);
        let byte = self.s.m.bin(self.cur, Bin::And, address, three);
        self.s.m.bin(self.cur, Bin::Shl, byte, three)
    }

    fn write_subword_result(
        &mut self,
        inst: &Inst,
        reg: u8,
        x: V,
        half: Option<u32>,
        guard: Option<V>,
    ) -> Result<(), Why> {
        let reg = Reg::V(reg);
        let x = if let Some(shift) = half {
            let old = self.i32_of(inst.read(reg).ok_or("a D16 load's previous register value")?)?;
            let c = self.cur;
            let (mask, keep, at) = (
                self.s.m.i32(0xFFFF),
                self.s.m.i32(!(0xFFFF << shift)),
                self.s.m.i32(shift),
            );
            let field = self.s.m.bin(c, Bin::And, x, mask);
            let field = self.s.m.bin(c, Bin::Shl, field, at);
            let old = self.s.m.bin(c, Bin::And, old, keep);
            self.s.m.bin(c, Bin::Or, old, field)
        } else {
            x
        };
        self.write(inst, reg, x, guard);
        Ok(())
    }

    /// Loads a byte field via aligned dword accesses. A half starting at
    /// byte 3 reads the first byte of the next dword.
    fn read_subword(
        &mut self,
        address: V,
        width: u32,
        signed: bool,
        dword: impl Fn(&mut Self, V) -> Result<V, Why>,
    ) -> Result<V, Why> {
        let c = self.cur;
        let shift = self.byte_shift(address);
        let not3 = self.s.m.i32(!3);
        let aligned = self.s.m.bin(c, Bin::And, address, not3);
        let w = dword(self, aligned)?;
        let low = self.s.m.bin(c, Bin::LShr, w, shift);
        let x = if width == 16 {
            let k24 = self.s.m.i32(24);
            let split = self.s.m.cmp(c, pred::EQ, shift, k24);
            let at = self.begin_guarded(Some(split));
            let four = self.s.m.i32(4);
            let next = self.s.m.bin(self.cur, Bin::Add, aligned, four);
            let high = dword(self, next)?;
            let eight = self.s.m.i32(8);
            let high = self.s.m.bin(self.cur, Bin::Shl, high, eight);
            let both = self.s.m.bin(self.cur, Bin::Or, low, high);
            self.end_guarded(at, &[(both, low)])[0]
        } else {
            low
        };
        let left = self.s.m.i32(32 - width);
        let x = self.s.m.bin(self.cur, Bin::Shl, x, left);
        Ok(self.s.m.bin(self.cur, if signed { Bin::AShr } else { Bin::LShr }, x, left))
    }

    /// Masks the affected bytes in each dword. A CAS keeps concurrent
    /// writes to other bytes in the same dword from being lost.
    fn write_subword(
        &mut self,
        address: V,
        width: u32,
        data: V,
        store: impl Fn(&mut Self, V, V, V),
    ) {
        let c = self.cur;
        let shift = self.byte_shift(address);
        let not3 = self.s.m.i32(!3);
        let aligned = self.s.m.bin(c, Bin::And, address, not3);
        let mask = self.s.m.i32((1 << width) - 1);
        let data = self.s.m.bin(c, Bin::And, data, mask);
        let bits = self.s.m.bin(c, Bin::Shl, data, shift);
        let mask = self.s.m.bin(c, Bin::Shl, mask, shift);
        store(self, aligned, mask, bits);
        if width == 16 {
            let c = self.cur;
            let k24 = self.s.m.i32(24);
            let split = self.s.m.cmp(c, pred::EQ, shift, k24);
            let at = self.begin_guarded(Some(split));
            let four = self.s.m.i32(4);
            let next = self.s.m.bin(self.cur, Bin::Add, aligned, four);
            let (eight, byte_mask) = (self.s.m.i32(8), self.s.m.i32(255));
            let bits = self.s.m.bin(self.cur, Bin::LShr, data, eight);
            store(self, next, byte_mask, bits);
            self.end_guarded(at, &[]);
        }
    }

    fn lds_ptr(&mut self, byte: V) -> V {
        let shift = self.s.m.i32(if self.wide_lds { 3 } else { 2 });
        let index = self.s.m.bin(self.cur, Bin::LShr, byte, shift);
        let lds = self.s.lds(self.wide_lds);
        self.s.m.element(self.cur, lds, index)
    }

    // DXIL forbids reinterpreting TGSM through a differently sized pointer.
    // When a shader needs i64 atomics, every access uses the same i64 array.
    fn lds_half_shift(&mut self, byte: V) -> V {
        let (four, eight) = (self.s.m.i32(4), self.s.m.i32(8));
        let half = self.s.m.bin(self.cur, Bin::And, byte, four);
        let shift = self.s.m.bin(self.cur, Bin::Mul, half, eight);
        let ty = self.s.m.int(64);
        self.s.m.cast(self.cur, Cast::ZExt, shift, ty)
    }

    fn lds_load_dword(&mut self, byte: V) -> V {
        if self.wide_lds {
            let end = self.s.m.i32(self.s.lds_dwords * 4);
            let in_bounds = self.s.m.cmp(self.cur, pred::IULT, byte, end);
            let at = self.begin_guarded(Some(in_bounds));
            let p = self.lds_ptr(byte);
            let ty = self.s.m.int(64);
            let zero = self.s.m.int_const(ty, 0);
            // Atomic access avoids racing with a lane using the other half.
            let pair = self.s.m.atomic(self.cur, Rmw::Or, p, zero);
            let shift = self.lds_half_shift(byte);
            let x = self.s.m.bin(self.cur, Bin::LShr, pair, shift);
            let x = self.s.m.cast(self.cur, Cast::Trunc, x, self.s.t.i32);
            let zero = self.s.m.i32(0);
            return self.end_guarded(at, &[(x, zero)])[0];
        }
        let p = self.lds_ptr(byte);
        self.s.m.load(self.cur, p)
    }

    fn lds_update_dword(&mut self, byte: V, update: impl FnOnce(&mut Self, V) -> V) -> V {
        let p = self.lds_ptr(byte);
        let zero = if self.wide_lds {
            let ty = self.s.m.int(64);
            self.s.m.int_const(ty, 0)
        } else {
            self.s.m.i32(0)
        };
        let shift = self.wide_lds.then(|| self.lds_half_shift(byte));
        let mut result = None;
        self.atomic_update(zero, |l, old| {
            let c = l.cur;
            let field = if let Some(shift) = shift {
                let field = l.s.m.bin(c, Bin::LShr, old, shift);
                l.s.m.cast(c, Cast::Trunc, field, l.s.t.i32)
            } else {
                old
            };
            result = Some(field);
            let next = update(l, field);
            let next = if let Some(shift) = shift {
                let ty = l.s.m.int(64);
                let next = l.s.m.cast(c, Cast::ZExt, next, ty);
                let next = l.s.m.bin(c, Bin::Shl, next, shift);
                let mask = l.s.m.int_const(ty, i64::from(u32::MAX));
                let mask = l.s.m.bin(c, Bin::Shl, mask, shift);
                l.atomic_new_value(AtomicUpdate::MaskOr, old, mask, next)
            } else {
                next
            };
            l.s.m.cmpxchg(c, p, old, next)
        });
        result.expect("the compare-exchange body was emitted")
    }

    fn lds_store_dword(&mut self, byte: V, data: V) {
        if self.wide_lds {
            self.lds_update_dword(byte, |_, _| data);
        } else {
            let p = self.lds_ptr(byte);
            self.s.m.store(self.cur, p, data);
        }
    }

    fn atomic_data(&mut self, inst: &Inst, data: &Operand, wide: bool) -> Result<V, Why> {
        let words = self.vgprs(inst, data, if wide { 2 } else { 1 })?;
        Ok(if wide { self.join64(words[0], words[1]) } else { words[0] })
    }

    /// Code that depends on the lane layout: wave operations, and in compute
    /// the console's wave size (lanes are thread ids in group order there).
    /// Pixel lanes match only within a quad.
    fn cross_lane(&mut self, quad: bool) -> Result<(), Why> {
        if self.s.mesh.is_some() {
            // shortcut: no wave size for mesh shaders in SM 6.6, so host
            // lanes cannot follow the console's; emulate through group-shared
            // memory if mesh programs turn out to need these.
            return Err("a cross-lane operation in a mesh shader");
        }
        if self.s.threads.is_none() && !(quad && self.s.pixel) {
            return Err("a cross-lane operation outside compute");
        }
        debug_assert!(self.s.threads.is_none() || self.console_lanes);
        self.s.flags |= flags::WAVE_OPS;
        Ok(())
    }

    /// Component `c` of the thread id in the group. With console lanes, the
    /// thread is relabelled so that its host lane is its console lane: the
    /// flat id is the wave's first lane's, rounded down to the wave size,
    /// plus the lane index. (A bijection while each host wave holds a
    /// wave-aligned run of flat ids, as the console's waves do.)
    fn thread_id(&mut self, c: u8) -> Val {
        if !self.console_lanes {
            return self.compute_id("threadIdInGroup.i32", 95, c);
        }
        let [x, y, _] = self.s.threads.expect("compute");
        let e = self.s.entry;
        let f = self.console_flat();
        let (xk, yk, xy) = (self.s.m.i32(x), self.s.m.i32(y), self.s.m.i32(x * y));
        Val::I32(match c {
            0 => self.s.m.bin(e, Bin::URem, f, xk),
            1 => {
                let t = self.s.m.bin(e, Bin::UDiv, f, xk);
                self.s.m.bin(e, Bin::URem, t, yk)
            }
            _ => self.s.m.bin(e, Bin::UDiv, f, xy),
        })
    }

    /// Mesh: the counts and the primitives no lane exports. The console's
    /// counts come from one lane's allocation request, which DXIL cannot
    /// express (one call, dominating every output write).
    // shortcut: the counts are the declared maxima and each lane first culls
    // its primitive (all indices 0, zero area), so primitives past the
    // program's count are culled and vertices past it unused; this costs
    // the host the unused vertex and primitive slots. Pass the program's own
    // counts if that shows in frame time.
    fn mesh_prologue(&mut self, mesh: super::shader::Mesh) -> Result<(), Why> {
        let threads = self.s.threads.expect("mesh")[0];
        if mesh.topology != 2 {
            return Err("a mesh shader not drawing triangles");
        }
        if threads > 128 || mesh.max_vertices > 256 || mesh.max_primitives > 256 {
            return Err("a mesh shader beyond the host's limits");
        }
        // The console allows larger outputs than the host; the signatures
        // link to the title's pixel shader, so none are dropped.
        if mesh_output_bytes(&self.s.outputs, &self.s.prims, mesh) > MAX_MESH_OUTPUT_BYTES {
            return Err("mesh outputs beyond the host's 32 KiB");
        }
        let e = self.s.entry;
        let void = self.s.t.void;
        let (v, p) = (
            self.s.m.i32(mesh.max_vertices),
            self.s.m.i32(mesh.max_primitives),
        );
        self.s
            .op(e, "setMeshOutputCounts", Attr::NoUnwind, void, 168, &[v, p]);
        let flat = self.mesh_thread();
        let guard = (threads > mesh.max_primitives).then(|| self.s.m.cmp(e, pred::IULT, flat, p));
        let at = self.begin_guarded(guard);
        let z = self.s.m.i32(0);
        self.s.op(
            self.cur,
            "emitIndices",
            Attr::NoUnwind,
            void,
            169,
            &[flat, z, z, z],
        );
        self.end_guarded(at, &[]);
        Ok(())
    }

    /// Mesh: the thread's index in the group, which is its vertex and
    /// primitive index.
    fn mesh_thread(&mut self) -> V {
        if let Some(f) = self.flat {
            return f;
        }
        let e = self.s.entry;
        let f = self.s.op(
            e,
            "flattenedThreadIdInGroup.i32",
            Attr::ReadNone,
            self.s.t.i32,
            96,
            &[],
        );
        self.flat = Some(f);
        f
    }

    /// The console wave size.
    fn wave(&self) -> u32 {
        if self.x.native.wave64 { 64 } else { 32 }
    }

    fn lane_index(&mut self) -> V {
        if self.s.mesh.is_some() {
            // The console's lane: mesh code reads no other lanes, so host
            // waves need not match the console's.
            let f = self.mesh_thread();
            let m = self.s.m.i32(self.wave() - 1);
            return self.s.m.bin(self.s.entry, Bin::And, f, m);
        }
        self.s.op(
            self.cur,
            "waveGetLaneIndex",
            Attr::ReadOnly,
            self.s.t.i32,
            111,
            &[],
        )
    }

    /// `x` of lane `src` (in this lane's quad) through quad reads, whose
    /// lane must be a constant.
    fn quad_pick(&mut self, x: V, src: V) -> V {
        self.s.flags |= flags::WAVE_OPS;
        let c = self.cur;
        let three = self.s.m.i32(3);
        let q = self.s.m.bin(c, Bin::And, src, three);
        let mut r = x;
        for n in 0..4 {
            let kn = self.s.m.i32(n);
            let read = self.s.op(
                c,
                "quadReadLaneAt.i32",
                Attr::NoUnwind,
                self.s.t.i32,
                122,
                &[x, kn],
            );
            let is = self.s.m.cmp(c, pred::EQ, q, kn);
            r = self.s.m.select(c, is, read, r);
        }
        r
    }

    /// Console flow: `x` of console lane `lane` (uniform), from whichever
    /// half holds it.
    fn console_read_lane(&mut self, x: V, lane: V) -> V {
        if !self.split {
            // One host wave is the console wave, all of its lanes here.
            return self.read_lane(x, lane);
        }
        let c = self.cur;
        let (k31, k5, one, z) = (
            self.s.m.i32(31),
            self.s.m.i32(5),
            self.s.m.i32(1),
            self.s.m.i32(0),
        );
        let at = self.s.m.bin(c, Bin::And, lane, k31);
        let mine = self.read_lane(x, at);
        let (x0, x1) = self.exchange(c, mine);
        let h = self.s.m.bin(c, Bin::LShr, lane, k5);
        let h = self.s.m.bin(c, Bin::And, h, one);
        let low = self.s.m.cmp(c, pred::EQ, h, z);
        self.s.m.select(c, low, x0, x1)
    }

    /// Whether a read of another lane of this lane's quad must reach helper
    /// lanes: in a pixel shader, always. At a triangle's edge the quad's
    /// other pixels are helpers (and so are discarded ones), which only quad
    /// reads see: wave reads give 0 from them on AMD, so a derivative taken
    /// by hand (`quad_perm`) was the whole value there, the texture read
    /// that used it took its smallest level, and the triangles' edges showed
    /// as lines on car paint, glass and skin. Upstream does this after
    /// discards only (77053a0).
    fn helpers_read(&self, quad: bool) -> bool {
        quad && self.s.pixel
    }

    /// `v_readfirstlane_b32`'s result `first` of `own` in a pixel shader:
    /// a helper lane takes its own value instead (upstream 0601955).
    ///
    /// On the console, whole-quad mode puts helper lanes in EXEC, so a
    /// waterfall loop (read the first lane's value, run the lanes with that
    /// value, take them out of EXEC) serves helper lanes too. On the host,
    /// wave operations leave helper lanes out: once only helper lanes are
    /// left in such a loop, `WaveReadLaneFirst` reads no lane, no lane
    /// matches, and the loop runs to its limit in every such lane, with
    /// whatever descriptor the read gave. With its own value a helper lane
    /// matches in the first pass and leaves; what it computes only feeds
    /// derivatives (those of a texture read's dependent coordinates, at
    /// triangle edges), and its stores are dropped.
    fn helper_reads_own(&mut self, own: V, first: V) -> V {
        if !self.s.pixel {
            return first;
        }
        let c = self.cur;
        let helper = self
            .s
            .op(c, "isHelperLane.i1", Attr::ReadOnly, self.s.t.i1, 221, &[]);
        self.s.m.select(c, helper, own, first)
    }

    fn read_lane(&mut self, x: V, lane: V) -> V {
        self.s.op(
            self.cur,
            "waveReadLaneAt.i32",
            Attr::NoUnwind,
            self.s.t.i32,
            117,
            &[x, lane],
        )
    }

    /// Coarse derivatives of `coords` across this lane's quad, as DXIL
    /// gradients (x for up to three coordinates, then y), each times
    /// `scale` if given.
    fn quad_gradients(&mut self, coords: &[V], scale: Option<V>) -> Vec<V> {
        let c = self.cur;
        let lane = self.lane_index();
        let top = self.s.m.i32(!3);
        let base = self.s.m.bin(c, Bin::And, lane, top);
        let lanes: Vec<V> = (1..3u32)
            .map(|k| {
                let k = self.s.m.i32(k);
                self.s.m.bin(c, Bin::Or, base, k)
            })
            .collect();
        let fu = self.s.m.undef(self.s.t.f32);
        let mut g = vec![fu; 6];
        for (k, &x) in coords.iter().enumerate().take(3) {
            let xi = self.as_int(x);
            let pixel = self.s.pixel;
            let at = |l: &mut Self, lane: V| {
                // A pixel's quad through quad reads (see `helpers_read`).
                let r = if pixel { l.quad_pick(xi, lane) } else { l.read_lane(xi, lane) };
                l.as_float(r)
            };
            let origin = at(self, base);
            for (d, &lane) in lanes.iter().enumerate() {
                let there = at(self, lane);
                let mut diff = self.s.m.bin(c, Bin::Sub, there, origin);
                if let Some(s) = scale {
                    diff = self.s.m.bin(c, Bin::Mul, diff, s);
                }
                g[3 * d + k] = diff;
            }
        }
        g
    }

    /// The float value of a register value.
    fn float_of(&mut self, v: Value) -> Result<V, Why> {
        if let Some(&f) = self.floats.get(&v) {
            return Ok(f);
        }
        let x = self.i32_of(v)?;
        Ok(self.as_float(x))
    }

    /// The values of VGPRs `reg..reg + n` as `inst` reads them.
    fn vgprs(&mut self, inst: &Inst, op: &Operand, n: u8) -> Result<Vec<V>, Why> {
        let Operand::Vector { reg, .. } = *op else {
            return Err("a vector operand");
        };
        (0..n)
            .map(|k| {
                let v = inst.read(Reg::V(reg + k)).ok_or("an unread operand")?;
                self.i32_of(v)
            })
            .collect()
    }

    /// A byte or halfword LDS read or write at byte address `a + offset`,
    /// through the dword holding it.
    #[allow(clippy::too_many_arguments)]
    fn ds_sub_dword(
        &mut self,
        inst: &Inst,
        sub: SubDword,
        a: V,
        offset: u32,
        dst_reg: Option<u8>,
        data: &[Operand],
        guard: Option<V>,
    ) -> Result<(), Why> {
        let c = self.cur;
        let k = |l: &mut Self, v: u32| l.s.m.i32(v);
        // shortcut: as for dwords, in bounds; a halfword is taken as
        // 2-aligned (the bytes never straddle two dwords).
        let o = k(self, offset);
        let at = self.s.m.bin(c, Bin::Add, a, o);
        let not3 = k(self, !3);
        let aligned = self.s.m.bin(c, Bin::And, at, not3);
        let three = k(self, 3);
        let byte = self.s.m.bin(c, Bin::And, at, three);
        let shift = self.s.m.bin(c, Bin::Shl, byte, three);
        let mask = k(self, (1 << sub.bits) - 1);
        if !sub.write {
            let reg = dst_reg.ok_or("an LDS read result")?;
            let x = self.lds_load_dword(aligned);
            let c = self.cur;
            let x = self.s.m.bin(c, Bin::LShr, x, shift);
            let x = if sub.signed {
                let up = k(self, 32 - sub.bits);
                let x = self.s.m.bin(c, Bin::Shl, x, up);
                self.s.m.bin(c, Bin::AShr, x, up)
            } else {
                self.s.m.bin(c, Bin::And, x, mask)
            };
            let x = match sub.d16 {
                None => x,
                Some(high) => {
                    let old = inst.read(Reg::V(reg)).ok_or("an unread operand")?;
                    let old = self.i32_of(old)?;
                    let low16 = k(self, 0xFFFF);
                    let half = self.s.m.bin(c, Bin::And, x, low16);
                    if high {
                        let keep = self.s.m.bin(c, Bin::And, old, low16);
                        let sixteen = k(self, 16);
                        let half = self.s.m.bin(c, Bin::Shl, half, sixteen);
                        self.s.m.bin(c, Bin::Or, keep, half)
                    } else {
                        let high16 = k(self, 0xFFFF_0000);
                        let keep = self.s.m.bin(c, Bin::And, old, high16);
                        self.s.m.bin(c, Bin::Or, keep, half)
                    }
                }
            };
            self.write(inst, Reg::V(reg), x, guard);
            return Ok(());
        }
        let d = self.vgprs(inst, data.first().ok_or("an LDS write")?, 1)?[0];
        let d = if sub.d16 == Some(true) {
            let sixteen = k(self, 16);
            self.s.m.bin(c, Bin::LShr, d, sixteen)
        } else {
            d
        };
        let d = self.s.m.bin(c, Bin::And, d, mask);
        let set = self.s.m.bin(c, Bin::Shl, d, shift);
        let bits = self.s.m.bin(c, Bin::Shl, mask, shift);
        let ones = k(self, !0);
        let clear = self.s.m.bin(c, Bin::Xor, bits, ones);
        // Lanes write other bytes of the same dword at the same time, so
        // the bytes go in atomically: cleared, then set. shortcut: lanes
        // writing the same bytes at once leave both values or-ed together
        // where the console keeps one of them.
        let at = self.begin_guarded(guard);
        if self.wide_lds {
            // The LDS is an i64 array (64-bit atomics): update the dword
            // in place through its compare-exchange loop.
            self.lds_update_dword(aligned, |l, old| {
                let c = l.cur;
                let kept = l.s.m.bin(c, Bin::And, old, clear);
                l.s.m.bin(c, Bin::Or, kept, set)
            });
        } else {
            let p = self.lds_ptr(aligned);
            self.s.m.atomic(self.cur, Rmw::And, p, clear);
            self.s.m.atomic(self.cur, Rmw::Or, p, set);
        }
        self.end_guarded(at, &[]);
        Ok(())
    }

    /// LDS (thread-group shared memory) and `ds_swizzle`.
    fn ds(&mut self, inst: &Inst, b: usize, i: usize) -> Result<(), Why> {
        let Fields::Ds {
            dst,
            addr,
            data,
            offset0,
            offset1,
            gds,
        } = &inst.fields
        else {
            unreachable!()
        };
        if *gds {
            return Err("GDS");
        }
        let name = inst.name();
        if name == "ds_nop" {
            return Ok(());
        }
        let guard = self.guard(b, i)?;
        let c = self.cur;
        let offset = u32::from(*offset0) | u32::from(*offset1) << 8;
        let dst_reg = match dst {
            Some(Operand::Vector { reg, .. }) => Some(*reg),
            _ => None,
        };
        if name == "ds_swizzle_b32" {
            // Bit 15 set: each lane of a quad picks a lane of it; else lanes
            // of each group of 32 combine their index with and/or/xor masks.
            let quad = offset & 0x8000 != 0
                || offset & 0x1C == 0x1C && offset & 0x380 == 0 && offset >> 10 & 0x1F < 4;
            // Mesh: host quads are threads 4n..4n+3 of a 1D group, as the
            // console's, so quad-local swizzles read through quad operations.
            let mesh_quad = self.s.mesh.is_some() && quad;
            if !mesh_quad {
                self.cross_lane(quad)?;
            }
            // The source sits in the address field.
            let x = self.vgprs(inst, addr.as_ref().ok_or("a swizzle source")?, 1)?[0];
            let lane = self.lane_index();
            let k = |l: &mut Self, v: u32| l.s.m.i32(v);
            let src = if offset & 0x8000 != 0 {
                let (three, one) = (k(self, 3), k(self, 1));
                let sel = self.s.m.bin(c, Bin::And, lane, three);
                let sh = self.s.m.bin(c, Bin::Shl, sel, one);
                let pattern = k(self, offset & 0xFF);
                let pick = self.s.m.bin(c, Bin::LShr, pattern, sh);
                let pick = self.s.m.bin(c, Bin::And, pick, three);
                let not3 = k(self, !3);
                let base = self.s.m.bin(c, Bin::And, lane, not3);
                self.s.m.bin(c, Bin::Or, base, pick)
            } else {
                let and = k(self, offset & 0x1F | !0x1F);
                let or = k(self, offset >> 5 & 0x1F);
                let xor = k(self, offset >> 10 & 0x1F);
                let t = self.s.m.bin(c, Bin::And, lane, and);
                let t = self.s.m.bin(c, Bin::Or, t, or);
                self.s.m.bin(c, Bin::Xor, t, xor)
            };
            let r = if mesh_quad || self.helpers_read(quad) {
                self.quad_pick(x, src)
            } else {
                self.read_lane(x, src)
            };
            let reg = dst_reg.ok_or("a swizzle result")?;
            self.write(inst, Reg::V(reg), r, guard);
            return Ok(());
        }
        if name == "ds_bpermute_b32" {
            // Each lane reads the data of the lane it names (times 4): 0 if
            // that one does not run this.
            self.cross_lane(false)?;
            if self.split {
                return Err("a lane permutation in a wave64 program split in halves");
            }
            let at = self.vgprs(inst, addr.as_ref().ok_or("a permutation address")?, 1)?[0];
            let x = self.vgprs(inst, data.first().ok_or("permutation data")?, 1)?[0];
            let (by, two, last) = (
                self.s.m.i32(offset),
                self.s.m.i32(2),
                self.s.m.i32(self.wave() - 1),
            );
            let at = self.s.m.bin(c, Bin::Add, at, by);
            let at = self.s.m.bin(c, Bin::LShr, at, two);
            let src = self.s.m.bin(c, Bin::And, at, last);
            let z = self.s.m.i32(0);
            let x = match guard {
                Some(g) => self.s.m.select(c, g, x, z),
                None => x,
            };
            let r = self.read_lane(x, src);
            let reg = dst_reg.ok_or("a permutation result")?;
            self.write(inst, Reg::V(reg), r, guard);
            return Ok(());
        }
        if self.s.lds_dwords == 0 {
            return Err("LDS the program does not allocate");
        }
        if name.contains("addtid") {
            return Err("an LDS address based on the console lane ID and M0");
        }
        if name.contains("_src2_") {
            return Err("an LDS operation with a second memory source");
        }
        if name.starts_with("ds_gws") || matches!(name, "ds_append" | "ds_consume" | "ds_ordered_count") {
            return Err("an LDS or GDS queue or global wave synchronization operation");
        }
        let a = self.vgprs(inst, addr.as_ref().ok_or("an LDS address")?, 1)?[0];
        let byte = |l: &mut Self, bytes: u32| {
            let o = l.s.m.i32(bytes);
            l.s.m.bin(l.cur, Bin::Add, a, o)
        };
        // shortcut: whole-dword LDS addresses are taken as aligned and in bounds.
        let width = |n: &str| match n.rsplit('_').next() {
            Some("b64") => 2,
            Some("b96") => 3,
            Some("b128") => 4,
            _ => 1,
        };
        // (byte offset, dwords) per element; read2/write2 scale their two
        // offsets by the element size (×64 for st64).
        let pair = |n: &str| -> Vec<(u32, u32)> {
            let w = width(n);
            let scale = 4 * w * if n.contains("st64") { 64 } else { 1 };
            vec![
                (u32::from(*offset0) * scale, w),
                (u32::from(*offset1) * scale, w),
            ]
        };
        if let Some(sub) = SubDword::of(name) {
            return self.ds_sub_dword(inst, sub, a, offset, dst_reg, data, guard);
        }
        if let Some(rest) = name.strip_prefix("ds_read") {
            let reg = dst_reg.ok_or("an LDS read result")?;
            let elements = if rest.starts_with('2') {
                pair(name)
            } else if matches!(rest, "_b32" | "_b64" | "_b96" | "_b128") {
                vec![(offset, width(name))]
            } else {
                return Err("an LDS read of bytes or halves");
            };
            let mut n = 0;
            for (off, w) in elements {
                for k in 0..w {
                    let address = byte(self, off + 4 * k);
                    let x = self.lds_load_dword(address);
                    self.write(inst, Reg::V(reg + n), x, guard);
                    n += 1;
                }
            }
            return Ok(());
        }
        if let Some(rest) = name.strip_prefix("ds_write") {
            let elements = if rest.starts_with('2') {
                pair(name)
            } else if matches!(rest, "_b32" | "_b64" | "_b96" | "_b128") {
                vec![(offset, width(name))]
            } else {
                return Err("an LDS write of bytes or halves");
            };
            let mut stores = Vec::new();
            for (e, (off, w)) in elements.into_iter().enumerate() {
                let vals = self.vgprs(inst, data.get(e).ok_or("an LDS write")?, w as u8)?;
                for (k, x) in vals.into_iter().enumerate() {
                    stores.push((off + 4 * k as u32, x));
                }
            }
            let at = self.begin_guarded(guard);
            for (off, x) in stores {
                let address = byte(self, off);
                self.lds_store_dword(address, x);
            }
            self.end_guarded(at, &[]);
            return Ok(());
        }
        // Atomics: the old value returns with `_rtn`.
        if name.ends_with("_f32") || name.ends_with("_f64") {
            // Mesa's aco_insert_fp_mode.cpp tracks the applicable MODE
            // denormal bits for these. NativeProgram does not carry them.
            return Err("an LDS floating-point atomic requiring the console's denormal mode");
        }
        if name.starts_with("ds_condxchg32") {
            return Err("an LDS conditional exchange with undocumented compare operands");
        }
        let (op, wide, kind) = ds_integer_atomic(name).ok_or("an LDS operation")?;
        if matches!(op, "wrxchg2" | "wrxchg2st64") {
            // AMD RDNA 2 ISA §12.13 specifies separate exchanges at the
            // two addresses. It does not specify their order if they alias.
            if offset0 == offset1 {
                return Err("an LDS paired exchange with coincident addresses");
            }
            let reg = dst_reg.ok_or("an LDS exchange result")?;
            let d0 = self.atomic_data(inst, data.first().ok_or("LDS exchange data")?, wide)?;
            let d1 = self.atomic_data(inst, data.get(1).ok_or("LDS exchange data")?, wide)?;
            let ty = self.s.m.type_of(d0);
            let zero = self.s.m.int_const(ty, 0);
            let at = self.begin_guarded(guard);
            let mut old = Vec::new();
            for ((off, _), data) in pair(name).into_iter().zip([d0, d1]) {
                let address = byte(self, off);
                let x = if !wide && self.wide_lds {
                    self.lds_update_dword(address, |_, _| data)
                } else {
                    let p = self.lds_ptr(address);
                    self.s.m.atomic(self.cur, Rmw::Xchg, p, data)
                };
                old.push((x, zero));
            }
            if self.wide_lds {
                self.s.flags |= flags::INT64 | flags::ATOMIC_INT64_GROUP_SHARED;
            }
            let words = if wide { 2 } else { 1 };
            for (k, x) in self.end_guarded(at, &old).into_iter().enumerate() {
                self.write_atomic_result(inst, reg + k as u8 * words, x, wide, guard);
            }
            return Ok(());
        }
        let rmw = match (op, kind) {
            ("add" | "sub", _) => Some(Rmw::Add),
            ("and", _) => Some(Rmw::And),
            ("or", _) => Some(Rmw::Or),
            ("xor", _) => Some(Rmw::Xor),
            ("min", b'i') => Some(Rmw::Min),
            ("max", b'i') => Some(Rmw::Max),
            ("min", _) => Some(Rmw::UMin),
            ("max", _) => Some(Rmw::UMax),
            ("wrxchg", _) => Some(Rmw::Xchg),
            _ => None,
        };
        let update = match op {
            "rsub" => Some(AtomicUpdate::ReverseSub),
            "inc" => Some(AtomicUpdate::Inc),
            "dec" => Some(AtomicUpdate::Dec),
            "mskor" => Some(AtomicUpdate::MaskOr),
            "wrap" => Some(AtomicUpdate::Wrap),
            _ if rmw.is_some() || op == "cmpst" => None,
            _ => return Err("an LDS operation"),
        };
        let v0 = self.atomic_data(inst, data.first().ok_or("LDS atomic data")?, wide)?;
        let ty = self.s.m.type_of(v0);
        let zero = self.s.m.int_const(ty, 0);
        let v1 = if matches!(op, "mskor" | "wrap" | "cmpst") {
            self.atomic_data(inst, data.get(1).ok_or("a second LDS atomic operand")?, wide)?
        } else {
            zero
        };
        let address = byte(self, offset);
        let at = self.begin_guarded(guard);
        if wide || self.wide_lds {
            self.s.flags |= flags::INT64 | flags::ATOMIC_INT64_GROUP_SHARED;
        }
        let r = if !wide && self.wide_lds {
            self.lds_update_dword(address, |l, old| {
                if let Some(update) = update {
                    return l.atomic_new_value(update, old, v0, v1);
                }
                let c = l.cur;
                match op {
                    "add" => l.s.m.bin(c, Bin::Add, old, v0),
                    "sub" => l.s.m.bin(c, Bin::Sub, old, v0),
                    "and" => l.s.m.bin(c, Bin::And, old, v0),
                    "or" => l.s.m.bin(c, Bin::Or, old, v0),
                    "xor" => l.s.m.bin(c, Bin::Xor, old, v0),
                    "wrxchg" => v0,
                    "cmpst" => {
                        let same = l.s.m.cmp(c, pred::EQ, old, v0);
                        l.s.m.select(c, same, v1, old)
                    }
                    _ => {
                        let pred = match (op, kind) {
                            ("min", b'i') => pred::SLT,
                            ("max", b'i') => pred::SGT,
                            ("min", _) => pred::IULT,
                            ("max", _) => pred::IUGT,
                            _ => unreachable!(),
                        };
                        let take = l.s.m.cmp(c, pred, v0, old);
                        l.s.m.select(c, take, v0, old)
                    }
                }
            })
        } else if let Some(rmw) = rmw {
            let p = self.lds_ptr(address);
            // DXIL supports atomic add, with subtraction encoded as -DATA.
            let data = if op == "sub" { self.s.m.bin(self.cur, Bin::Sub, zero, v0) } else { v0 };
            self.s.m.atomic(self.cur, rmw, p, data)
        } else if let Some(update) = update {
            let p = self.lds_ptr(address);
            self.atomic_update(zero, |l, old| {
                let x = l.atomic_new_value(update, old, v0, v1);
                l.s.m.cmpxchg(l.cur, p, old, x)
            })
        } else {
            // Compare with data0, store data1.
            let p = self.lds_ptr(address);
            self.s.m.cmpxchg(self.cur, p, v0, v1)
        };
        let r = self.end_guarded(at, &[(r, zero)])[0];
        if let Some(reg) = dst_reg {
            self.write_atomic_result(inst, reg, r, wide, guard);
        }
        Ok(())
    }

    fn write_atomic_result(&mut self, inst: &Inst, reg: u8, old: V, wide: bool, guard: Option<V>) {
        if wide {
            let (lo, hi) = self.split64(old);
            self.write(inst, Reg::V(reg), lo, guard);
            self.write(inst, Reg::V(reg + 1), hi, guard);
        } else {
            self.write(inst, Reg::V(reg), old, guard);
        }
    }

    /// See [`Lower::ordered`]: programs that reach memory by address or
    /// write through root views, which are the builders of ray tracing's
    /// structures.
    fn ordered(&mut self) -> bool {
        let x = self.x;
        *self.ordered.get_or_insert_with(|| {
            super::rays()
                && (x.res.addresses
                    || x.native.used.iter().any(|u| {
                        u.root == RootType::Descriptor && u.kind == ResourceType::Uav
                    }))
        })
    }

    /// Writes to buffers made before are seen before those made after, by
    /// every group.
    fn uav_fence(&mut self) {
        let (mode, void) = (self.s.m.i32(2), self.s.t.void);
        self.s
            .op(self.cur, "barrier", Attr::NoDuplicate, void, 80, &[mode]);
    }

    // ---- instructions ----

    fn inst(&mut self, b: usize, i: usize) -> Result<(), Stop> {
        let inst: &'a Inst = &self.x.p.blocks[b].insts[i];
        if self.dead.contains(&(b, i)) {
            for &(_, v) in &inst.writes {
                self.vals
                    .insert(v, Val::Missing("a spilled SGPR's lane read as data"));
            }
            return Ok(());
        }
        let offset = inst.machine.offset;
        let name = inst.name();
        self.mask_ok = match self.mask_data {
            MaskData::Console => true,
            MaskData::Host => {
                name.starts_with("s_bcnt1_i32_")
                    || name.starts_with("s_ff1_i32_")
                    || name.starts_with("v_mbcnt_")
                    || matches!(
                        name,
                        "s_cmp_eq_u32" | "s_cmp_lg_u32" | "s_cmp_eq_u64" | "s_cmp_lg_u64"
                    )
            }
            MaskData::None => false,
        };
        let effect = matches!(inst.fields, Fields::Exp { .. })
            || matches!(inst.fields, Fields::Flat { .. })
                && !(name.starts_with("global_load_") || name.starts_with("scratch_load_"))
            || name.contains("store")
            || name.contains("atomic")
            || name == "s_barrier"
            || matches!(inst.fields, Fields::Ds { .. })
                && !(name.starts_with("ds_read")
                    || name.starts_with("ds_swizzle")
                    || name.contains("permute"));
        let fence = (name.starts_with("buffer_atomic") || name.starts_with("global_atomic"))
            && self.ordered();
        if fence {
            self.uav_fence();
        }
        let r = match &inst.fields {
            Fields::Sop { .. } => self.sop(inst),
            Fields::Sopp { .. } if name == "s_barrier" && self.no_barrier.contains(&b) => Ok(()),
            Fields::Sopp { .. } if name == "s_barrier" => {
                // Thread-group sync with a shared-memory fence, and a UAV
                // fence when the program writes UAVs (the waits before an
                // s_barrier make those writes visible on the console).
                let mode = if self.writes_uavs { 1 | 2 | 8 } else { 1 | 8 };
                let mode = self.s.m.i32(mode);
                let void = self.s.t.void;
                self.s
                    .op(self.cur, "barrier", Attr::NoDuplicate, void, 80, &[mode]);
                Ok(())
            }
            Fields::Sopp { .. } => Ok(()),
            Fields::Smem { .. } => self.smem(inst, (b, i)),
            Fields::Valu(_) if name.starts_with("v_interp_") => self.interp(inst, (b, i)),
            Fields::Valu(_) => self.valu(inst, b, i),
            Fields::Vintrp { .. } => self.interp(inst, (b, i)),
            Fields::Image { .. } => self.image(inst, b, i),
            Fields::Buffer { .. } => self.buffer(inst, b, i),
            Fields::Exp { .. } => self.export(b, i),
            Fields::Ds { .. } => self.ds(inst, b, i),
            Fields::Flat { .. } if name.starts_with("global_") => self.flat(inst, b, i),
            Fields::Flat { .. } if name.starts_with("scratch_") => self.scratch(inst, b, i),
            // Generic addresses: private, shared or global memory by aperture.
            Fields::Flat { .. } => Err("flat memory"),
        };
        if fence && r.is_ok() {
            self.uav_fence();
        }
        if let Err(w) = &r && std::env::var_os("VICTORY_WHY").is_some() {
            eprintln!("missing {offset:#x} {name} {w}");
        }
        #[cfg(test)]
        if let Err(why) = r
            && AUDIT_OFFSET.get() == Some(offset)
        {
            return Err(unsupported(offset, why));
        }
        match r {
            Ok(()) => Ok(()),
            Err(why) if effect => Err(unsupported(offset, why)),
            Err(why) => {
                for &(_, v) in &inst.writes {
                    self.vals.insert(v, Val::Missing(why));
                }
                Ok(())
            }
        }
    }

    fn is_mask(&self, v: Option<Value>) -> bool {
        v.is_some_and(|v| self.x.st.masks.contains_key(&v))
    }

    fn sop(&mut self, inst: &Inst) -> Result<(), Why> {
        let Fields::Sop { dst, src, imm } = &inst.fields else {
            unreachable!()
        };
        let name = inst.name();
        let scc = inst.write(Reg::Scc);
        let dst_code = match dst {
            Some(Operand::Scalar { code, .. }) => Some(*code),
            _ => None,
        };
        let lo = dst_code.and_then(|c| inst.write(Reg::S(c)));
        // A lane mask written by a set operation also gets its value as a
        // number, from its sources' numbers. A ballot where the mask is
        // written misses the lanes it holds that do not run there: a loop's
        // exit mask gathers lanes that left in earlier iterations. Only the
        // masks something reads as numbers get one.
        let number = self.mask_ok
            && mask_set_op(name)
            && inst.writes.iter().any(|w| self.mask_numbers.contains(&w.1));
        if inst.write(Reg::S(EXEC)).is_some() || self.is_mask(lo) && !number {
            return Err(LANE_MASK);
        }
        if matches!(
            name,
            "s_setreg_imm32_b32" | "s_setreg_b32" | "s_swappc_b64" | "s_version"
        ) || name.starts_with("s_waitcnt")
        {
            // shortcut: the float MODE register is not modelled; s_swappc_b64 is the
            // vertex fetch call, whose results arrive as inputs.
            return Err("a register the emitter does not model");
        }
        let c = self.cur;
        let src32 = |l: &mut Self, k: usize| -> Result<V, Why> {
            // SOPK arithmetic reads its destination as the first source.
            if k == 0 && matches!(name, "s_addk_i32" | "s_mulk_i32") {
                let d = dst.as_ref().ok_or("a missing operand")?;
                return l.operand(inst, d);
            }
            let op = src.get(k).ok_or("a missing operand")?;
            l.operand(inst, op)
        };
        let sext16 = |l: &mut Self| l.s.m.i32(i32::from(imm.unwrap_or(0) as i16) as u32);
        // Dword `h` of 64-bit source `k` (inline constants sign-extend).
        let half = |l: &mut Self, k: usize, h: u8| -> Result<V, Why> {
            match src.get(k) {
                Some(&Operand::Scalar { code, .. }) if code != NULL => {
                    let v = inst.read(Reg::S(code + h)).ok_or("an unread operand")?;
                    l.i32_of(v)
                }
                Some(op) if h == 0 => l.operand(inst, op),
                Some(Operand::Const(k)) => {
                    let x = crate::resource::inline(*k).ok_or("a special operand")?;
                    Ok(l.s.m.i32(if (x as i32) < 0 { u32::MAX } else { 0 }))
                }
                Some(Operand::Literal(x)) if name == "s_bfe_i64" => {
                    // AMDGPUOperandSyntax: a literal in a signed i64 operand sign-extends.
                    Ok(l.s.m.i32(if (*x as i32) < 0 { u32::MAX } else { 0 }))
                }
                _ => Ok(l.s.m.i32(0)),
            }
        };
        // See AMD's RDNA 2 ISA, sections 12.1–12.4 (SOP2/K/1/C).
        let old = |l: &mut Self, h: u8| -> Result<V, Why> {
            let reg = dst_code.ok_or("a missing destination")?;
            if reg == NULL {
                return Ok(l.s.m.i32(0));
            }
            let value = inst.read(Reg::S(reg + h)).ok_or("an unread destination")?;
            l.i32_of(value)
        };
        if name.starts_with("s_bitcmp") {
            let index = src32(self, 1)?;
            let a = half(self, 0, 0)?;
            let x = if name.ends_with("64") {
                let b = half(self, 0, 1)?;
                let k32 = self.s.m.i32(32);
                let hi = self.s.m.bin(c, Bin::And, index, k32);
                let zero = self.s.m.i32(0);
                let lo = self.s.m.cmp(c, pred::EQ, hi, zero);
                self.s.m.select(c, lo, a, b)
            } else {
                a
            };
            let last = self.s.m.i32(31);
            let index = self.s.m.bin(c, Bin::And, index, last);
            let bit = self.s.m.bin(c, Bin::LShr, x, index);
            let one = self.s.m.i32(1);
            let bit = self.s.m.bin(c, Bin::And, bit, one);
            let k = self.s.m.i32(u32::from(name.starts_with("s_bitcmp1")));
            let test = self.s.m.cmp(c, pred::EQ, bit, k);
            self.set(scc, Val::Bit(test));
            return Ok(());
        }
        if let Some(op) = name.strip_prefix("s_cmp_").filter(|o| o.ends_with("_u64")) {
            // eq and lg only: both dwords.
            let mut r = None;
            for h in 0..2 {
                let (a, b) = (half(self, 0, h)?, half(self, 1, h)?);
                let e = self.s.m.cmp(c, pred::EQ, a, b);
                r = Some(match r {
                    Some(x) => self.s.m.bin(c, Bin::And, x, e),
                    None => e,
                });
            }
            let mut r = r.expect("two dwords");
            if op.starts_with("lg") {
                let t = self.s.m.bool(true);
                r = self.s.m.bin(c, Bin::Xor, r, t);
            }
            self.set(scc, Val::Bit(r));
            return Ok(());
        }
        if let Some(op) = name.strip_prefix("s_cmp_").or(name.strip_prefix("s_cmpk_")) {
            if matches!(name, "s_cmp_eq_u32" | "s_cmp_lg_u32") {
                let s0 = src.first().ok_or("a missing operand")?;
                let s1 = src.get(1).ok_or("a missing operand")?;
                let p0 = self.host_mask_pair(inst, s0)?;
                let p1 = self.host_mask_pair(inst, s1)?;
                let eq = match (p0, p1) {
                    (Some(a), Some(b)) => Some(self.eq_64(c, a, b)),
                    (Some(a), None) if Self::is_zero_operand(s1) => {
                        let z = self.s.m.i32(0);
                        Some(self.eq_64(c, a, (z, z)))
                    }
                    (None, Some(b)) if Self::is_zero_operand(s0) => {
                        let z = self.s.m.i32(0);
                        Some(self.eq_64(c, (z, z), b))
                    }
                    _ => None,
                };
                if let Some(mut r) = eq {
                    if op.starts_with("lg") {
                        let t = self.s.m.bool(true);
                        r = self.s.m.bin(c, Bin::Xor, r, t);
                    }
                    self.set(scc, Val::Bit(r));
                    return Ok(());
                }
            }
            let (a, b) = if name.starts_with("s_cmpk_") {
                let b = if op.ends_with("_i32") {
                    sext16(self)
                } else {
                    self.s.m.i32(u32::from(imm.unwrap_or(0)))
                };
                (src32(self, 0)?, b)
            } else {
                if op.ends_with("64") {
                    return Err("a 64-bit scalar compare");
                }
                (src32(self, 0)?, src32(self, 1)?)
            };
            let signed = op.ends_with("_i32");
            let p = match &op[..2] {
                "eq" => pred::EQ,
                "lg" => pred::NE,
                "gt" if signed => pred::SGT,
                "ge" if signed => pred::SGE,
                "lt" if signed => pred::SLT,
                "le" if signed => pred::SLE,
                "gt" => pred::IUGT,
                "ge" => pred::IUGE,
                "lt" => pred::IULT,
                "le" => pred::IULE,
                _ => return Err("a scalar compare"),
            };
            let r = self.s.m.cmp(c, p, a, b);
            self.set(scc, Val::Bit(r));
            return Ok(());
        }
        let wide = name.ends_with("_b64") || name.ends_with("_u64") || name.ends_with("_i64")
            || name == "s_bitreplicate_b64_b32";
        if wide {
            let hi = dst_code.and_then(|c| inst.write(Reg::S(c + 1)));
            let nonzero = |l: &mut Self, a: V, b: V| {
                let o = l.s.m.bin(c, Bin::Or, a, b);
                let z = l.s.m.i32(0);
                l.s.m.cmp(c, pred::NE, o, z)
            };
            return match name {
                "s_bcnt0_i32_b64" | "s_bcnt1_i32_b64" => {
                    let (a, b) = (half(self, 0, 0)?, half(self, 0, 1)?);
                    let r = self.bcnt1_64(c, a, b);
                    let r = if name == "s_bcnt0_i32_b64" {
                        let size = self.s.m.i32(64);
                        self.s.m.bin(c, Bin::Sub, size, r)
                    } else {
                        r
                    };
                    self.set(lo, Val::I32(r));
                    let z = self.s.m.i32(0);
                    let s = self.s.m.cmp(c, pred::NE, r, z);
                    self.set(scc, Val::Bit(s));
                    Ok(())
                }
                "s_ff0_i32_b64" | "s_ff1_i32_b64" | "s_flbit_i32_b64" | "s_flbit_i32_i64" => {
                    let (mut a, mut b) = (half(self, 0, 0)?, half(self, 0, 1)?);
                    if name == "s_ff0_i32_b64" {
                        let ones = self.s.m.i32(u32::MAX);
                        a = self.s.m.bin(c, Bin::Xor, a, ones);
                        b = self.s.m.bin(c, Bin::Xor, b, ones);
                    } else if name == "s_flbit_i32_i64" {
                        let k31 = self.s.m.i32(31);
                        let sign = self.s.m.bin(c, Bin::AShr, b, k31);
                        a = self.s.m.bin(c, Bin::Xor, a, sign);
                        b = self.s.m.bin(c, Bin::Xor, b, sign);
                    }
                    let op = if name.starts_with("s_flbit") {
                        (a, b) = (b, a);
                        33 // first bit high, counted from the MSB
                    } else {
                        32 // first bit low
                    };
                    let first = self.s.op(c, "unaryBits.i32", Attr::ReadNone, self.s.t.i32, op, &[a]);
                    let second = self.s.op(c, "unaryBits.i32", Attr::ReadNone, self.s.t.i32, op, &[b]);
                    let (z, k32) = (self.s.m.i32(0), self.s.m.i32(32));
                    let offset = self.s.m.bin(c, Bin::Add, second, k32);
                    let none = self.s.m.cmp(c, pred::EQ, b, z);
                    let second = self.s.m.select(c, none, second, offset);
                    let found = self.s.m.cmp(c, pred::NE, a, z);
                    let r = self.s.m.select(c, found, first, second);
                    self.set(lo, Val::I32(r));
                    Ok(())
                }
                "s_brev_b64" => {
                    for (h, dst) in [(0, hi), (1, lo)] {
                        let x = half(self, 0, h)?;
                        let x = self.s.op(c, "unary.i32", Attr::ReadNone, self.s.t.i32, 30, &[x]);
                        self.set(dst, Val::I32(x));
                    }
                    Ok(())
                }
                "s_bitreplicate_b64_b32" => {
                    let x = src32(self, 0)?;
                    let k16 = self.s.m.i32(16);
                    let upper = self.s.m.bin(c, Bin::LShr, x, k16);
                    for (mut x, dst) in [(x, lo), (upper, hi)] {
                        let mask = self.s.m.i32(0xFFFF);
                        x = self.s.m.bin(c, Bin::And, x, mask);
                        // Spread the 16 source bits onto even positions.
                        for (shift, mask) in [(8, 0x00FF_00FF), (4, 0x0F0F_0F0F),
                                              (2, 0x3333_3333), (1, 0x5555_5555)] {
                            let sh = self.s.m.i32(shift);
                            let m = self.s.m.i32(mask);
                            let t = self.s.m.bin(c, Bin::Shl, x, sh);
                            let t = self.s.m.bin(c, Bin::Or, t, x);
                            x = self.s.m.bin(c, Bin::And, t, m);
                        }
                        let one = self.s.m.i32(1);
                        let other = self.s.m.bin(c, Bin::Shl, x, one);
                        let r = self.s.m.bin(c, Bin::Or, x, other);
                        self.set(dst, Val::I32(r));
                    }
                    Ok(())
                }
                "s_bfe_u64" | "s_bfe_i64" | "s_bfm_b64" | "s_bitset0_b64" | "s_bitset1_b64" => {
                    let ty = self.s.m.int(64);
                    self.s.flags |= flags::INT64;
                    let one = self.s.m.int_const(ty, 1);
                    let k63 = self.s.m.i32(63);
                    let count = |l: &mut Self, x: V| {
                        let x = l.s.m.bin(c, Bin::And, x, k63);
                        l.s.m.cast(c, Cast::ZExt, x, ty)
                    };
                    let r = match name {
                        "s_bfe_u64" | "s_bfe_i64" => {
                            let (a, b) = (half(self, 0, 0)?, half(self, 0, 1)?);
                            let a = self.join64(a, b);
                            let off = src32(self, 1)?;
                            let k16 = self.s.m.i32(16);
                            let k127 = self.s.m.i32(127);
                            let w = self.s.m.bin(c, Bin::LShr, off, k16);
                            let w = self.s.m.bin(c, Bin::And, w, k127);
                            self.bitfield(a, off, w, name == "s_bfe_i64", 64)
                        }
                        "s_bfm_b64" => {
                            let (a, b) = (src32(self, 0)?, src32(self, 1)?);
                            let (width, off) = (count(self, a), count(self, b));
                            let m = self.s.m.bin(c, Bin::Shl, one, width);
                            let m = self.s.m.bin(c, Bin::Sub, m, one);
                            self.s.m.bin(c, Bin::Shl, m, off)
                        }
                        _ => {
                            let (a, b) = (old(self, 0)?, old(self, 1)?);
                            let x = self.join64(a, b);
                            let bit = src32(self, 0)?;
                            let bit = count(self, bit);
                            let bit = self.s.m.bin(c, Bin::Shl, one, bit);
                            if name == "s_bitset0_b64" {
                                let ones = self.s.m.int_const(ty, -1);
                                let mask = self.s.m.bin(c, Bin::Xor, bit, ones);
                                self.s.m.bin(c, Bin::And, x, mask)
                            } else {
                                self.s.m.bin(c, Bin::Or, x, bit)
                            }
                        }
                    };
                    let (a, b) = self.split64(r);
                    self.set(lo, Val::I32(a));
                    self.set(hi, Val::I32(b));
                    let nz = nonzero(self, a, b);
                    self.set(scc, Val::Bit(nz));
                    Ok(())
                }
                "s_and_b64" | "s_or_b64" | "s_xor_b64" | "s_andn2_b64" | "s_orn2_b64"
                | "s_nand_b64" | "s_nor_b64" | "s_xnor_b64" | "s_not_b64" => {
                    let ones = self.s.m.i32(u32::MAX);
                    let mut r = Vec::new();
                    for h in 0..2u8 {
                        let a = half(self, 0, h)?;
                        let x = if name == "s_not_b64" {
                            self.s.m.bin(c, Bin::Xor, a, ones)
                        } else {
                            let mut b = half(self, 1, h)?;
                            if name.contains("n2") {
                                b = self.s.m.bin(c, Bin::Xor, b, ones);
                            }
                            let op = match &name[2..5] {
                                "and" | "nan" => Bin::And,
                                "or_" | "orn" | "nor" => Bin::Or,
                                _ => Bin::Xor,
                            };
                            let x = self.s.m.bin(c, op, a, b);
                            if matches!(name, "s_nand_b64" | "s_nor_b64" | "s_xnor_b64") {
                                self.s.m.bin(c, Bin::Xor, x, ones)
                            } else {
                                x
                            }
                        };
                        r.push(x);
                    }
                    self.set(lo, Val::I32(r[0]));
                    self.set(hi, Val::I32(r[1]));
                    let s = nonzero(self, r[0], r[1]);
                    self.set(scc, Val::Bit(s));
                    Ok(())
                }
                "s_lshl_b64" | "s_lshr_b64" | "s_ashr_i64" => {
                    let (a, b) = (half(self, 0, 0)?, half(self, 0, 1)?);
                    let n = src32(self, 1)?;
                    let i64t = self.s.m.int(64);
                    self.s.flags |= flags::INT64;
                    let a = self.s.m.cast(c, Cast::ZExt, a, i64t);
                    let b = self.s.m.cast(c, Cast::ZExt, b, i64t);
                    let k32 = self.s.m.int_const(i64t, 32);
                    let b = self.s.m.bin(c, Bin::Shl, b, k32);
                    let x = self.s.m.bin(c, Bin::Or, a, b);
                    let k63 = self.s.m.i32(63);
                    let n = self.s.m.bin(c, Bin::And, n, k63);
                    let n = self.s.m.cast(c, Cast::ZExt, n, i64t);
                    let op = match name {
                        "s_lshl_b64" => Bin::Shl,
                        "s_lshr_b64" => Bin::LShr,
                        _ => Bin::AShr,
                    };
                    let r = self.s.m.bin(c, op, x, n);
                    let rl = self.s.m.cast(c, Cast::Trunc, r, self.s.t.i32);
                    let rh = self.s.m.bin(c, Bin::LShr, r, k32);
                    let rh = self.s.m.cast(c, Cast::Trunc, rh, self.s.t.i32);
                    self.set(lo, Val::I32(rl));
                    self.set(hi, Val::I32(rh));
                    let s = nonzero(self, rl, rh);
                    self.set(scc, Val::Bit(s));
                    Ok(())
                }
                "s_getpc_b64" => {
                    // Addresses in the code are offsets in it.
                    let next = self.s.m.i32(inst.machine.offset as u32 + 4);
                    let zero = self.s.m.i32(0);
                    self.set(lo, Val::I32(next));
                    self.set(hi, Val::I32(zero));
                    Ok(())
                }
                "s_mov_b64" => {
                    let (a, b) = (half(self, 0, 0)?, half(self, 0, 1)?);
                    self.set(lo, Val::I32(a));
                    self.set(hi, Val::I32(b));
                    Ok(())
                }
                "s_cselect_b64" => {
                    let cond = self.scc_in(inst)?;
                    for (h, v) in [(0, lo), (1, hi)] {
                        let (a, b) = (half(self, 0, h)?, half(self, 1, h)?);
                        let r = self.s.m.select(c, cond, a, b);
                        self.set(v, Val::I32(r));
                    }
                    Ok(())
                }
                "s_cmov_b64" => {
                    // Keeps the old destination when SCC is clear.
                    let cond = self.scc_in(inst)?;
                    for (h, v) in [(0, lo), (1, hi)] {
                        let a = half(self, 0, h)?;
                        let old = dst_code
                            .and_then(|d| inst.read(Reg::S(d + h)))
                            .ok_or("an unread destination")?;
                        let old = self.i32_of(old)?;
                        let r = self.s.m.select(c, cond, a, old);
                        self.set(v, Val::I32(r));
                    }
                    Ok(())
                }
                _ => Err(name),
            };
        }
        let m = &mut self.s.m;
        let _ = m;
        let (r, carry): (V, Option<V>) = match name {
            "s_mov_b32" => (src32(self, 0)?, None),
            "s_movk_i32" => (sext16(self), None),
            "s_cmov_b32" | "s_cmovk_i32" => {
                // Keeps the old destination when SCC is clear.
                let cond = self.scc_in(inst)?;
                let a = if name == "s_cmovk_i32" {
                    sext16(self)
                } else {
                    src32(self, 0)?
                };
                let d = dst.as_ref().ok_or("a missing operand")?;
                let old = self.operand(inst, d)?;
                (self.s.m.select(c, cond, a, old), None)
            }
            "s_cselect_b32" => {
                let cond = self.scc_in(inst)?;
                let (a, b) = (src32(self, 0)?, src32(self, 1)?);
                (self.s.m.select(c, cond, a, b), None)
            }
            "s_brev_b32" => {
                let a = src32(self, 0)?;
                let r = self
                    .s
                    .op(c, "unary.i32", Attr::ReadNone, self.s.t.i32, 30, &[a]);
                (r, None)
            }
            "s_not_b32" => {
                let a = src32(self, 0)?;
                let ones = self.s.m.i32(u32::MAX);
                (self.s.m.bin(c, Bin::Xor, a, ones), None)
            }
            "s_add_u32" | "s_add_i32" | "s_addk_i32" => {
                let a = src32(self, 0)?;
                let b = if name == "s_addk_i32" {
                    sext16(self)
                } else {
                    src32(self, 1)?
                };
                let r = self.s.m.bin(c, Bin::Add, a, b);
                let carry = if name == "s_add_u32" {
                    Some(self.s.m.cmp(c, pred::IULT, r, a))
                } else {
                    None
                };
                (r, carry)
            }
            "s_sub_u32" | "s_sub_i32" => {
                let (a, b) = (src32(self, 0)?, src32(self, 1)?);
                let r = self.s.m.bin(c, Bin::Sub, a, b);
                let carry = (name == "s_sub_u32").then(|| self.s.m.cmp(c, pred::IULT, a, b));
                (r, carry)
            }
            "s_addc_u32" | "s_subb_u32" => {
                let (a, b) = (src32(self, 0)?, src32(self, 1)?);
                let cin = self.scc_in(inst)?;
                let (r, carry) = self.add_sub_carry(c, name == "s_addc_u32", a, b, Some(cin));
                (r, Some(carry))
            }
            "s_mul_i32" | "s_mulk_i32" => {
                let a = src32(self, 0)?;
                let b = if name == "s_mulk_i32" {
                    sext16(self)
                } else {
                    src32(self, 1)?
                };
                (self.s.m.bin(c, Bin::Mul, a, b), None)
            }
            "s_mul_hi_u32" | "s_mul_hi_i32" => {
                let (a, b) = (src32(self, 0)?, src32(self, 1)?);
                (self.mul_hi(a, b, name.ends_with("i32")), None)
            }
            "s_flbit_i32_b32" | "s_flbit_i32" => {
                // The first bit from the top (differing from the sign for
                // the signed form), -1 for none: DXIL's first-bit-high.
                let a = src32(self, 0)?;
                let op = if name == "s_flbit_i32_b32" { 33 } else { 34 };
                let r = self
                    .s
                    .op(c, "unaryBits.i32", Attr::ReadNone, self.s.t.i32, op, &[a]);
                (r, None)
            }
            "s_ff0_i32_b32" | "s_ff1_i32_b32" | "s_bcnt0_i32_b32" | "s_bcnt1_i32_b32" => {
                // First set bit (-1 for zero, as DXIL's), or the bit count.
                let op0 = src.first().ok_or("a missing operand")?;
                let pair = if matches!(name, "s_ff1_i32_b32" | "s_bcnt1_i32_b32") {
                    self.host_mask_pair(inst, op0)?
                } else {
                    None
                };
                let r = if let Some((a, b)) = pair {
                    if name == "s_ff1_i32_b32" {
                        self.ff1_64(c, a, b)
                    } else {
                        self.bcnt1_64(c, a, b)
                    }
                } else {
                    let mut a = src32(self, 0)?;
                    if matches!(name, "s_ff0_i32_b32" | "s_bcnt0_i32_b32") {
                        let ones = self.s.m.i32(u32::MAX);
                        a = self.s.m.bin(c, Bin::Xor, a, ones);
                    }
                    let op = if name.starts_with("s_ff") { 32 } else { 31 };
                    self.s
                        .op(c, "unaryBits.i32", Attr::ReadNone, self.s.t.i32, op, &[a])
                };
                (r, None)
            }
            "s_bfe_u32" | "s_bfe_i32" => {
                // src1: offset in bits 0-4, width in bits 16-22.
                let (a, b) = (src32(self, 0)?, src32(self, 1)?);
                let (k16, k127) = (self.s.m.i32(16), self.s.m.i32(0x7F));
                let w = self.s.m.bin(c, Bin::LShr, b, k16);
                let w = self.s.m.bin(c, Bin::And, w, k127);
                (self.bfe(a, b, w, name == "s_bfe_i32"), None)
            }
            "s_and_b32" | "s_or_b32" | "s_xor_b32" | "s_andn2_b32" | "s_orn2_b32"
            | "s_nand_b32" | "s_nor_b32" | "s_xnor_b32" => {
                let (a, mut b) = (src32(self, 0)?, src32(self, 1)?);
                let ones = self.s.m.i32(u32::MAX);
                if name.contains("n2") {
                    b = self.s.m.bin(c, Bin::Xor, b, ones);
                }
                let op = match &name[2..5] {
                    "and" | "nan" => Bin::And,
                    "or_" | "orn" | "nor" => Bin::Or,
                    _ => Bin::Xor,
                };
                let mut r = self.s.m.bin(c, op, a, b);
                if matches!(name, "s_nand_b32" | "s_nor_b32" | "s_xnor_b32") {
                    r = self.s.m.bin(c, Bin::Xor, r, ones);
                }
                (r, None)
            }
            "s_lshl_b32" | "s_lshr_b32" | "s_ashr_i32" => {
                let (a, b) = (src32(self, 0)?, src32(self, 1)?);
                let k31 = self.s.m.i32(31);
                let b = self.s.m.bin(c, Bin::And, b, k31);
                let op = match name {
                    "s_lshl_b32" => Bin::Shl,
                    "s_lshr_b32" => Bin::LShr,
                    _ => Bin::AShr,
                };
                (self.s.m.bin(c, op, a, b), None)
            }
            "s_lshl1_add_u32" | "s_lshl2_add_u32" | "s_lshl3_add_u32" | "s_lshl4_add_u32" => {
                // SCC: the sum overflows 32 bits, through bits shifted out
                // of src0 or the add's carry.
                let (a, b) = (src32(self, 0)?, src32(self, 1)?);
                let n = u32::from(name.as_bytes()[6] - b'0');
                let k = self.s.m.i32(n);
                let t = self.s.m.bin(c, Bin::Shl, a, k);
                let r = self.s.m.bin(c, Bin::Add, t, b);
                let (rest, z) = (self.s.m.i32(32 - n), self.s.m.i32(0));
                let out = self.s.m.bin(c, Bin::LShr, a, rest);
                let shifted_out = self.s.m.cmp(c, pred::NE, out, z);
                let carry = self.s.m.cmp(c, pred::IULT, r, t);
                (r, Some(self.s.m.bin(c, Bin::Or, shifted_out, carry)))
            }
            "s_min_u32" | "s_max_u32" | "s_min_i32" | "s_max_i32" => {
                // SCC: src0 is the one chosen.
                let (a, b) = (src32(self, 0)?, src32(self, 1)?);
                let p = match name {
                    "s_min_u32" => pred::IULT,
                    "s_max_u32" => pred::IUGT,
                    "s_min_i32" => pred::SLT,
                    _ => pred::SGT,
                };
                let t = self.s.m.cmp(c, p, a, b);
                (self.s.m.select(c, t, a, b), Some(t))
            }
            "s_abs_i32" | "s_absdiff_i32" => {
                let mut a = src32(self, 0)?;
                if name == "s_absdiff_i32" {
                    let b = src32(self, 1)?;
                    a = self.s.m.bin(c, Bin::Sub, a, b);
                }
                let z = self.s.m.i32(0);
                let neg = self.s.m.cmp(c, pred::SLT, a, z);
                let n = self.s.m.bin(c, Bin::Sub, z, a);
                (self.s.m.select(c, neg, n, a), None)
            }
            "s_sext_i32_i8" | "s_sext_i32_i16" => {
                let a = src32(self, 0)?;
                (self.sign_extend(a, if name.ends_with("i8") { 8 } else { 16 }), None)
            }
            "s_pack_ll_b32_b16" | "s_pack_lh_b32_b16" | "s_pack_hh_b32_b16" => {
                let (mut a, mut b) = (src32(self, 0)?, src32(self, 1)?);
                let k16 = self.s.m.i32(16);
                if name == "s_pack_hh_b32_b16" {
                    a = self.s.m.bin(c, Bin::LShr, a, k16);
                }
                if name != "s_pack_ll_b32_b16" {
                    b = self.s.m.bin(c, Bin::LShr, b, k16);
                }
                (self.pack_halves(a, b), None)
            }
            "s_bitset0_b32" | "s_bitset1_b32" => {
                // Sets (clears) bit src[4:0] of the destination it reads.
                let old = self.operand(inst, dst.as_ref().ok_or("a missing operand")?)?;
                let a = src32(self, 0)?;
                let (k31, one) = (self.s.m.i32(31), self.s.m.i32(1));
                let a = self.s.m.bin(c, Bin::And, a, k31);
                let bit = self.s.m.bin(c, Bin::Shl, one, a);
                if name == "s_bitset1_b32" {
                    (self.s.m.bin(c, Bin::Or, old, bit), None)
                } else {
                    let ones = self.s.m.i32(u32::MAX);
                    let nb = self.s.m.bin(c, Bin::Xor, bit, ones);
                    (self.s.m.bin(c, Bin::And, old, nb), None)
                }
            }
            "s_bfm_b32" => {
                let (a, b) = (src32(self, 0)?, src32(self, 1)?);
                let k31 = self.s.m.i32(31);
                let one = self.s.m.i32(1);
                let a = self.s.m.bin(c, Bin::And, a, k31);
                let b = self.s.m.bin(c, Bin::And, b, k31);
                let m = self.s.m.bin(c, Bin::Shl, one, a);
                let m = self.s.m.bin(c, Bin::Sub, m, one);
                (self.s.m.bin(c, Bin::Shl, m, b), None)
            }
            _ => return Err(name),
        };
        self.set(lo, Val::I32(r));
        if let Some(scc) = scc {
            let val = match carry {
                Some(k) => Val::Bit(k),
                None if matches!(
                    name,
                    "s_mov_b32"
                        | "s_movk_i32"
                        | "s_cmov_b32"
                        | "s_cmovk_i32"
                        | "s_cselect_b32"
                        | "s_brev_b32"
                        | "s_mul_i32"
                        | "s_mulk_i32"
                ) =>
                {
                    return Ok(());
                }
                None if name.starts_with("s_add") || name.starts_with("s_sub") => {
                    Val::Missing("the carry of a scalar operation")
                }
                None => {
                    let z = self.s.m.i32(0);
                    Val::Bit(self.s.m.cmp(c, pred::NE, r, z))
                }
            };
            self.vals.insert(scc, val);
        }
        Ok(())
    }

    /// `a + b (+ cin)` or `a - b (- cin)` with the carry (borrow) out, as the
    /// scalar and vector add-with-carry instructions set it.
    fn add_sub_carry(&mut self, c: usize, add: bool, a: V, b: V, cin: Option<V>) -> (V, V) {
        let op = if add { Bin::Add } else { Bin::Sub };
        let mut r = self.s.m.bin(c, op, a, b);
        // Without a carry in: a + b wraps below a; a - b borrows when a < b.
        let mut carry = if add {
            self.s.m.cmp(c, pred::IULT, r, a)
        } else {
            self.s.m.cmp(c, pred::IULT, a, b)
        };
        if let Some(cin) = cin {
            let one = self.s.m.cast(c, Cast::ZExt, cin, self.s.t.i32);
            let t = r;
            r = self.s.m.bin(c, op, t, one);
            // The carry in carries on when the partial result is all ones
            // (add) or zero (sub).
            let edge = self.s.m.i32(if add { u32::MAX } else { 0 });
            let at_edge = self.s.m.cmp(c, pred::EQ, t, edge);
            let more = self.s.m.bin(c, Bin::And, cin, at_edge);
            carry = self.s.m.bin(c, Bin::Or, carry, more);
        }
        (r, carry)
    }

    fn scc_in(&mut self, inst: &Inst) -> Result<V, Why> {
        let v = inst.read(Reg::Scc).ok_or("an unread SCC")?;
        match self.get(v)? {
            Val::Bit(x) => Ok(x),
            _ => Err("SCC without a value"),
        }
    }

    fn smem(&mut self, inst: &Inst, site: Site) -> Result<(), Why> {
        let Fields::Smem {
            data,
            offset,
            soffset,
            ..
        } = &inst.fields
        else {
            unreachable!()
        };
        let name = inst.name();
        if name.starts_with("s_scratch_") {
            return Err("scalar access to the console scratch aperture");
        }
        if name.contains("_store_") {
            return Err("a scalar store without root UAV address support");
        }
        if let Some(a) = self.scalar.get(&site).copied().filter(|a| a.param == CODE) {
            return self.code_smem(inst, a);
        }
        let Some(rest) = name.strip_prefix("s_buffer_load_dword") else {
            if self.scalar.get(&site).is_none()
                && self.x.res.addresses
                && let Some(count) = name.strip_prefix("s_load_dword").and_then(smem_dwords)
                && let Some(Operand::Scalar { code, .. }) = *data
            {
                let Fields::Smem {
                    base: Operand::Scalar { code: at, .. },
                    ..
                } = inst.fields
                else {
                    return Err("a scalar load's address");
                };
                let (lo, hi) = (
                    inst.read(Reg::S(at)).ok_or("a scalar load's address")?,
                    inst.read(Reg::S(at + 1)).ok_or("a scalar load's address")?,
                );
                let mut address = (self.i32_of(lo)?, self.i32_of(hi)?);
                if *offset != 0 {
                    let x = self.s.m.i32(*offset as u32);
                    address = self.add_address(address, x, true);
                }
                if !matches!(soffset, Operand::Scalar { code: NULL, .. }) {
                    let so = self.operand(inst, soffset)?;
                    address = self.add_address(address, so, false);
                }
                let xs = self.memory_dwords(address, count, false);
                for (k, x) in xs.into_iter().enumerate() {
                    self.write(inst, Reg::S(code + k as u8), x, None);
                }
                return Ok(());
            }
            if self.scalar.get(&site).is_none()
                && crate::resource::traces_rays(self.x.p)
                && let Some(count) = name.strip_prefix("s_load_dword").and_then(smem_dwords)
                && let Some(Operand::Scalar { code, .. }) = *data
            {
                // shortcut: out of the acceleration structure.
                let z = self.s.m.i32(0);
                for k in 0..count {
                    self.write(inst, Reg::S(code + k as u8), z, None);
                }
                return Ok(());
            }
            if let Some(a) = self.scalar.get(&site).copied()
                && self
                    .x
                    .native
                    .layout
                    .get(a.param)
                    .is_some_and(|p| p.root == RootType::Descriptor)
            {
                return self.root_smem(inst, site, a);
            }
            return self.descriptor_data(inst, site);
        };
        let count = smem_dwords(rest).ok_or("a scalar buffer load")?;
        let Some(Operand::Scalar { code, .. }) = *data else {
            return Err("a scalar buffer load");
        };
        let src = self
            .descriptors
            .get(&(site, false))
            .copied()
            .ok_or("an untracked descriptor")?;
        let kind = self.used_kind(src);
        let c = self.cur;
        if self.by_address(src)
            && let Fields::Smem { base, .. } = &inst.fields
        {
            let (h, start, _) = self.made_buffer(inst, base)?;
            let mut addr = self.s.m.i32(*offset as u32);
            if !matches!(soffset, Operand::Scalar { code: NULL, .. }) {
                let so = self.operand(inst, soffset)?;
                addr = self.s.m.bin(c, Bin::Add, addr, so);
            }
            let addr = self.s.m.bin(c, Bin::Add, addr, start);
            let words = self.view_dwords(h, Some(0), addr, None, count);
            for (k, x) in words.into_iter().enumerate() {
                self.write(inst, Reg::S(code + k as u8), x, None);
            }
            return Ok(());
        }
        let (base, extra) = self.built_offset(src)?;
        let offset = &((*offset as u32).wrapping_add(base) as i32);
        let mut addr = self.s.m.i32(*offset as u32);
        if let Some(x) = extra {
            addr = self.s.m.bin(c, Bin::Add, addr, x);
        }
        if !matches!(soffset, Operand::Scalar { code: NULL, .. }) {
            let so = self.operand(inst, soffset)?;
            addr = self.s.m.bin(c, Bin::Add, addr, so);
        }
        if matches!(src, Source::Built { address, .. } if address.param == CODE) {
            // A descriptor built around a table in the code.
            for k in 0..count {
                let off = self.s.m.i32(4 * k);
                let a = self.s.m.bin(c, Bin::Add, addr, off);
                let x = self.code_dword(a)?;
                self.write(inst, Reg::S(code + k as u8), x, None);
            }
            return Ok(());
        }
        // A raw or structured buffer view (an SRV, or a UAV the code also
        // reads through scalar memory), else a constant buffer view.
        let raw = kind
            .filter(|u| matches!(u.kind, ResourceType::Srv | ResourceType::Uav))
            .map(|u| {
                let u_bit = if u.kind == ResourceType::Uav { 0x1000 } else { 0 };
                let stride = if u.stride >= 4 && u.stride.is_multiple_of(4) && !u.stride.is_multiple_of(16) {
                    u.stride
                } else {
                    0
                };
                (u_bit, stride)
            });
        let props = match raw {
            Some((u_bit, 0)) => (11 | u_bit, 0),
            Some((u_bit, stride)) => (12 | u_bit, stride),
            None => (13, 0x1_0000),
        };
        let h = self.handle(site, src, false, props)?;
        let constant = match addr {
            V::Const(_)
                if matches!(soffset, Operand::Scalar { code: NULL, .. }) && extra.is_none() =>
            {
                Some(*offset as u32)
            }
            _ => None,
        };
        let words = self.view_dwords(h, raw.map(|(_, s)| s), addr, constant, count);
        for (k, x) in words.into_iter().enumerate() {
            self.write(inst, Reg::S(code + k as u8), x, None);
        }
        Ok(())
    }

    /// `count` dwords from byte `addr` (`constant` when known) of a raw
    /// buffer (`raw` = `Some(0)`), a structured buffer of that stride
    /// (`raw` = `Some(stride)`) or a constant buffer view (`None`) `h`. The
    /// address's two low bits are ignored, as scalar memory does.
    fn view_dwords(
        &mut self,
        h: V,
        raw: Option<u32>,
        addr: V,
        constant: Option<u32>,
        count: u32,
    ) -> Vec<V> {
        let c = self.cur;
        let mut out = Vec::new();
        if let Some(stride) = raw {
            let not3 = self.s.m.i32(!3);
            let addr = self.s.m.bin(c, Bin::And, addr, not3);
            let st = (stride != 0).then(|| self.s.m.i32(stride));
            for k in 0..count {
                let off = self.s.m.i32(4 * k);
                let a = self.s.m.bin(c, Bin::Add, addr, off);
                let (idx, elem_off) = match st {
                    Some(st) => (
                        self.s.m.bin(c, Bin::UDiv, a, st),
                        self.s.m.bin(c, Bin::URem, a, st),
                    ),
                    None => (a, self.s.m.undef(self.s.t.i32)),
                };
                let mask = self.s.m.i8(1);
                let al = self.s.m.i32(4);
                let r = self.s.op(
                    c,
                    "rawBufferLoad.f32",
                    Attr::ReadOnly,
                    self.s.t.res_ret,
                    139,
                    &[h, idx, elem_off, mask, al],
                );
                let x = self.s.m.extract(c, r, 0);
                out.push(self.as_int(x));
            }
            return out;
        }
        let mut rows: HashMap<u32, V> = HashMap::new();
        for k in 0..count {
            let x = match constant {
                Some(base) => {
                    let byte = base.wrapping_add(4 * k);
                    let row = match rows.get(&(byte >> 4)) {
                        Some(&r) => r,
                        None => {
                            let ri = self.s.m.i32(byte >> 4);
                            let r = self.s.op(
                                c,
                                "cbufferLoadLegacy.i32",
                                Attr::ReadOnly,
                                self.s.t.cbuf_ret,
                                59,
                                &[h, ri],
                            );
                            rows.insert(byte >> 4, r);
                            r
                        }
                    };
                    self.s.m.extract(c, row, (byte >> 2) & 3)
                }
                None => {
                    let off = self.s.m.i32(4 * k);
                    let a = self.s.m.bin(c, Bin::Add, addr, off);
                    self.cbuffer_dword(h, a)
                }
            };
            out.push(x);
        }
        out
    }

    /// `s_load_dword*` from the memory of a root CBV or SRV: constants the
    /// code reads through scalar memory at an address it computes from the
    /// root descriptor's, read through the root view (a raw buffer for an
    /// SRV, a constant buffer otherwise).
    // shortcut: like the console's scalar loads, the host's root views are
    // not bounds-checked, so a read past the view's data is undefined on
    // both.
    fn root_smem(&mut self, inst: &Inst, site: Site, a: Address) -> Result<(), Why> {
        let Fields::Smem {
            data: Some(Operand::Scalar { code, .. }),
            ..
        } = inst.fields
        else {
            return Err("a scalar memory read as data");
        };
        let count = inst
            .name()
            .strip_prefix("s_load_dword")
            .and_then(smem_dwords)
            .ok_or("a scalar memory read as data")?;
        let src = Source::Built {
            address: a,
            bits: 0,
            words: [0, 0],
        };
        let srv = self
            .used_kind(&src)
            .is_some_and(|u| u.kind == ResourceType::Srv);
        let props = if srv { (11, 0) } else { (13, 0x1_0000) };
        let h = self.handle(site, &src, false, props)?;
        let c = self.cur;
        let mut addr = self.s.m.i32(a.offset);
        if let Some(d) = self.dynamic_term(&a)? {
            addr = self.s.m.bin(c, Bin::Add, addr, d);
        }
        let constant = a.dynamic.is_none().then_some(a.offset);
        let words = self.view_dwords(h, srv.then_some(0), addr, constant, count);
        for (k, x) in words.into_iter().enumerate() {
            self.write(inst, Reg::S(code + k as u8), x, None);
        }
        Ok(())
    }

    /// Two scalar loads of the same dwords: same opcode and immediate
    /// offset, base read from the same values, and the same or an
    /// equivalent register offset.
    fn twin_loads(&self, a: Site, b: Site) -> bool {
        let (ia, ib): (&Inst, &Inst) = (
            &self.x.p.blocks[a.0].insts[a.1],
            &self.x.p.blocks[b.0].insts[b.1],
        );
        let (
            Fields::Smem {
                base: Operand::Scalar { code: ba, .. },
                offset: oa,
                soffset: sa,
                ..
            },
            Fields::Smem {
                base: Operand::Scalar { code: bb, .. },
                offset: ob,
                soffset: sb,
                ..
            },
        ) = (&ia.fields, &ib.fields)
        else {
            return false;
        };
        let same_base = (0..2).all(|k| {
            let va = ia.read(Reg::S(ba + k));
            va.is_some() && va == ib.read(Reg::S(bb + k))
        });
        let same_soffset = match (sa, sb) {
            (Operand::Scalar { code: NULL, .. }, Operand::Scalar { code: NULL, .. }) => true,
            (Operand::Scalar { code: ca, .. }, Operand::Scalar { code: cb, .. }) => {
                match (ia.read(Reg::S(*ca)), ib.read(Reg::S(*cb))) {
                    (Some(x), Some(y)) => self.same_scalar(x, y),
                    _ => false,
                }
            }
            _ => false,
        };
        ia.name() == ib.name() && oa == ob && same_base && same_soffset
    }

    /// The same value, or two `v_readfirstlane_b32` of the same VGPR value
    /// (each waterfall loop reads the index anew).
    fn same_scalar(&self, x: Value, y: Value) -> bool {
        if x == y {
            return true;
        }
        let first_lane_of = |v: Value| {
            self.x
                .p
                .blocks
                .iter()
                .flat_map(|b| &b.insts)
                .find(|i| i.writes.iter().any(|w| w.1 == v))
                .filter(|i| i.name() == "v_readfirstlane_b32")
                .and_then(|i| {
                    i.reads
                        .iter()
                        .find(|r| matches!(r.0, Reg::V(_)))
                        .map(|r| r.1)
                })
        };
        matches!((first_lane_of(x), first_lane_of(y)), (Some(a), Some(b)) if a == b)
    }

    /// A descriptor load (`s_load_dword*` from a table or heap) whose dwords
    /// the code also reads as numbers: the console compiler computes sizes
    /// from descriptor bits (an image's width, height, depth, levels; a
    /// buffer's record count) instead of querying them. Those fields are
    /// rebuilt from the host view's dimensions in the GFX10 layout (image
    /// dword 1 bits 30-31 and dword 2 bits 0-13 width - 1, dword 2 bits
    /// 14-29 height - 1, dword 3 identity swizzle, last level at 16, type at
    /// 28, dword 4 depth or layers - 1; buffer dword 2 the record count).
    // shortcut: the base level reads 0 (the host view starts at its own
    // level 0, so sizes the code shifts by it come out right) and the other
    // dwords (addresses, formats, tiling) have no host value: reading them
    // stops the program. A cube reads as its six faces (not a cube array).
    fn descriptor_data(&mut self, inst: &Inst, site: Site) -> Result<(), Why> {
        const OTHER: Why = "a descriptor's address or format read as data";
        let Fields::Smem {
            data: Some(Operand::Scalar { code, .. }),
            ..
        } = inst.fields
        else {
            return Err("a scalar memory read as data");
        };
        let count = inst
            .name()
            .strip_prefix("s_load_dword")
            .and_then(smem_dwords)
            .ok_or("a scalar memory read as data")?;
        // How the program uses the descriptor tells the view's kind: this
        // load's dwords, or those of another load of the same descriptor
        // (the code often loads the whole descriptor for the resource and a
        // dword or two of it again for its size). `skew` is the dword of
        // the descriptor this load's first dword is; another load's
        // descriptor counts only if this load starts inside it. Prefer an
        // image or vector buffer use when a descriptor has both scalar and
        // vector uses, since those carry format and indexing cues.
        let find = |own: bool| {
            let mut best = None;
            for d in self
                .x
                .res
                .descriptors
                .iter()
                .filter(|d| d.role == Role::Resource)
            {
                for l in d.source.leaves() {
                    let Source::Loaded { load, first } = *l else {
                        continue;
                    };
                    if (load == site) != own {
                        continue;
                    }
                    // A twin load of the same dwords (a non-uniform index
                    // reloads the descriptor in each waterfall loop, one for
                    // the size bits, another for the access) is at skew 0.
                    let Some(skew) = self
                        .load_skew(inst, site, load)
                        .or_else(|| (!own && self.twin_loads(site, load)).then_some(0))
                        .map(|s| s - i32::from(first))
                    else {
                        continue;
                    };
                    if !own && !(0..8).contains(&skew) {
                        continue;
                    }
                    let ui = &self.x.p.blocks[d.site.0].insts[d.site.1];
                    let found = (d.site, load, first, skew);
                    if matches!(ui.fields, Fields::Image { .. } | Fields::Buffer { .. }) {
                        return Some(found);
                    }
                    best.get_or_insert(found);
                }
            }
            best
        };
        let use_at = find(true).or_else(|| find(false));
        let (load, skew, src, mut props, image, uav) = if let Some((at, load, first, skew)) = use_at {
            let src = Source::Loaded { load, first };
            let used = self.used_kind(&src);
            let ui: &Inst = &self.x.p.blocks[at.0].insts[at.1];
            let un = ui.name();
            let uav = un.contains("store")
                || un.contains("atomic")
                || used.is_some_and(|u| u.kind == ResourceType::Uav);
            let u = if uav { 0x1000 } else { 0 };
            let (props, image) = match &ui.fields {
                Fields::Image { dim, .. } => {
                    let kind = match dim {
                        0 => 1,
                        1 => 2,
                        2 => 4,
                        3 => 5,
                        4 => 6,
                        5 => 7,
                        6 => 3,
                        7 => 8,
                        _ => return Err("an image dimension"),
                    };
                    ((kind | u, 0x409), Some(kind))
                }
                Fields::Buffer { idxen, format, .. } => {
                    let stride = used.map_or(0, |u| u.stride);
                    let props = if un.starts_with("buffer_load_format")
                        || un.starts_with("buffer_store_format")
                    {
                        (10 | u, 0x409)
                    } else if *idxen && format.is_none() && stride >= 4 && stride.is_multiple_of(4)
                    {
                        (12 | u, stride)
                    } else {
                        (11 | u, 0)
                    };
                    if used.is_some_and(|u| u.kind == ResourceType::Cbv) {
                        return Err("a constant buffer's descriptor read as data");
                    }
                    (props, None)
                }
                Fields::Smem { .. } if un.starts_with("s_buffer_") => {
                    if used.is_some_and(|u| u.kind == ResourceType::Cbv) {
                        return Err("a constant buffer's descriptor read as data");
                    }
                    let stride = used.map_or(0, |u| u.stride);
                    let props = if stride >= 4 && stride.is_multiple_of(4) {
                        (12 | u, stride)
                    } else {
                        (11 | u, 0)
                    };
                    (props, None)
                }
                _ => return Err("a scalar memory read as data"),
            };
            (load, skew, src, props, image, uav)
        } else {
            // An image the program only measures (it reads the size bits and
            // samples the image in other programs): eight dwords are an image
            // descriptor, taken here as a 2D texture.
            // shortcut: a 3D or array image would report depth or layers 1.
            let src = Source::Loaded {
                load: site,
                first: 0,
            };
            let used = self.used_kind(&src);
            if count == 8 && !used.is_some_and(|u| u.kind != ResourceType::Srv) {
                (site, 0, src, (2, 0x409), Some(2), false)
            } else {
                // A buffer the program only counts the records of (#329):
                // an exact statically addressed four-dword binding of a
                // structured buffer the container declares, never the
                // nearest earlier entry or a dynamically indexed range.
                let uav = used.is_some_and(|u| u.kind == ResourceType::Uav);
                let address = self
                    .scalar
                    .get(&site)
                    .ok_or("a descriptor read only as data")?;
                let Some(binding) = used.filter(|b| {
                    count == 4
                        && address.dynamic.is_none()
                        && b.table_offset_dwords.checked_mul(4) == Some(address.offset)
                        && b.dwords == 4
                        && matches!(b.kind, ResourceType::Srv | ResourceType::Uav)
                        && (4..=2048).contains(&b.stride)
                        && b.stride.is_multiple_of(4)
                        && self.x.psv.is_some_and(|part| {
                            super::declares_structured_buffer(part, b.lower_bound, uav)
                        })
                }) else {
                    return Err("a descriptor read only as data");
                };
                let u = if uav { 0x1000 } else { 0 };
                (site, 0, src, (12 | u, binding.stride), None, uav)
            }
        };
        // A structured buffer's records, of a raw view's bytes.
        let records = (props.0 & 0xfff == 12 && props.1.is_multiple_of(16)).then_some(props.1);
        if records.is_some() {
            props = (11 | props.0 & 0x1000, 0);
        }
        // A typed or structured buffer's element count comes from the host's
        // table of them by slot: the AMD driver's compiler crashes on a size
        // query of a structured buffer from the heap (stride 60, nine
        // compute programs of the world) and takes minutes over hundreds of
        // a typed one's; the table holds the title's own count.
        // (A structured buffer the host views raw keeps the query, of bytes.)
        if image.is_none() && records.is_none() && matches!(props.0 & 0xfff, 10 | 12) {
            let slot = if load == site {
                self.heap_slot(&src, false)?
            } else {
                self.heap_index_at(site, skew.wrapping_neg(), false)?
            };
            let n = self.slot_dword(slot, super::OWN_SLOTS);
            for k in 0..count {
                if (k as i32).wrapping_add(skew) == 2 {
                    self.write(inst, Reg::S(code + k as u8), n, None);
                } else if let Some(v) = inst.write(Reg::S(code + k as u8)) {
                    self.vals.insert(v, Val::Missing(OTHER));
                }
            }
            return Ok(());
        }
        // The handle is built from this load's own address (the other
        // load's registers need not hold it here): the descriptor starts
        // `skew` dwords before what this load reads.
        let h = if load == site {
            self.handle(site, &src, false, props)?
        } else {
            let index = self.heap_index_at(site, skew.wrapping_neg(), false)?;
            self.s.heap_handle(self.cur, index, false, false, props)
        };
        // The size words are asked of the host only when the program reads
        // one as data: most loads take the descriptor whole for an image or
        // buffer instruction, and hundreds of size queries of heap handles
        // in one program stop the AMD driver's compiler (minutes, then
        // E_FAIL: nine compute programs of the world).
        let sized = if image.is_some() { 1..=4 } else { 2..=2 };
        let wanted = (0..count).any(|k| {
            sized.contains(&(k as i32).wrapping_add(skew))
                && inst
                    .write(Reg::S(code + k as u8))
                    .is_some_and(|v| self.read_as_data(v))
        });
        if !wanted {
            for k in 0..count {
                if let Some(v) = inst.write(Reg::S(code + k as u8)) {
                    self.vals.insert(v, Val::Missing(OTHER));
                }
            }
            return Ok(());
        }
        let c = self.cur;
        let i32t = self.s.t.i32;
        let dims = self.s.m.named_struct("dx.types.Dimensions", vec![i32t; 4]);
        let mip = if image.is_some_and(|k| !matches!(k, 3 | 8)) && !uav {
            self.s.m.i32(0)
        } else {
            self.s.m.undef(i32t)
        };
        let d = self
            .s
            .op(c, "getDimensions", Attr::ReadOnly, dims, 72, &[h, mip]);
        let field = |l: &mut Self, k: u32| l.s.m.extract(c, d, k);
        let words: Vec<(u32, V)> = match image {
            Some(kind) => {
                let one = self.s.m.i32(1);
                let w = field(self, 0);
                let (height, depth) = match kind {
                    1 => (one, one),
                    2 | 3 => (field(self, 1), one),
                    6 => (one, field(self, 1)),
                    // A cube is six faces (DXIL's dimensions have no depth).
                    5 => (field(self, 1), self.s.m.i32(6)),
                    _ => (field(self, 1), field(self, 2)),
                };
                // The level count only feeds word 3. A level query of a 3D
                // image inside a loop crashes NVIDIA's Vulkan compiler (vkd3d-
                // proton turns it into OpImageQueryLevels), so query it only
                // when the program reads that word as data.
                let reads_word3 = (0..count)
                    .filter(|&k| (k as i32).wrapping_add(skew) == 3)
                    .filter_map(|k| inst.write(Reg::S(code + k as u8)))
                    .any(|v| self.read_as_data(v));
                let levels = if uav || matches!(kind, 3 | 8) || !reads_word3 {
                    one
                } else {
                    field(self, 3)
                };
                let less = |l: &mut Self, x: V| l.s.m.bin(c, Bin::Sub, x, one);
                let (w, height, depth, levels) = (
                    less(self, w),
                    less(self, height),
                    less(self, depth),
                    less(self, levels),
                );
                let k = |l: &mut Self, v: u32| l.s.m.i32(v);
                let (k30, k2, k14, k16, mask) = (
                    k(self, 30),
                    k(self, 2),
                    k(self, 14),
                    k(self, 16),
                    k(self, 0x3FFF),
                );
                let d1 = self.s.m.bin(c, Bin::Shl, w, k30);
                let hi = self.s.m.bin(c, Bin::LShr, w, k2);
                let hi = self.s.m.bin(c, Bin::And, hi, mask);
                let ht = self.s.m.bin(c, Bin::Shl, height, k14);
                let d2 = self.s.m.bin(c, Bin::Or, hi, ht);
                let ty = [0, 8, 9, 14, 10, 11, 12, 13, 15][kind as usize];
                let base = k(self, 0xFAC | ty << 28);
                let last = self.s.m.bin(c, Bin::Shl, levels, k16);
                let d3 = self.s.m.bin(c, Bin::Or, base, last);
                vec![(1, d1), (2, d2), (3, d3), (4, depth)]
            }
            None => {
                let mut n = field(self, 0);
                if let Some(stride) = records {
                    let st = self.s.m.i32(stride);
                    n = self.s.m.bin(c, Bin::UDiv, n, st);
                }
                vec![(2, n)]
            }
        };
        for k in 0..count {
            let rel = (k as i32).wrapping_add(skew);
            match words.iter().find(|w| w.0 as i32 == rel) {
                Some(&(_, x)) => self.write(inst, Reg::S(code + k as u8), x, None),
                None => {
                    if let Some(v) = inst.write(Reg::S(code + k as u8)) {
                        self.vals.insert(v, Val::Missing(OTHER));
                    }
                }
            }
        }
        Ok(())
    }

    /// The dword of what the scalar load at `other` reads that `inst` (at
    /// `site`) reads first: the same base and offset registers, immediates
    /// a whole number of dwords apart, or else traced addresses that are
    /// (the same parameter and computed term: the code often builds the
    /// table pointer `{table, 0}` again before the second load); none for
    /// loads that differ otherwise.
    fn load_skew(&self, inst: &Inst, site: Site, other: Site) -> Option<i32> {
        if other == site {
            return Some(0);
        }
        if let (Some(a), Some(b)) = (self.scalar.get(&site), self.scalar.get(&other))
            && (a.param, a.dynamic, a.includes_base) == (b.param, b.dynamic, b.includes_base)
            && a.offset.wrapping_sub(b.offset) % 4 == 0
        {
            return Some(a.offset.wrapping_sub(b.offset) as i32 / 4);
        }
        let o: &Inst = &self.x.p.blocks[other.0].insts[other.1];
        let (
            Fields::Smem {
                base: Operand::Scalar { code: b0, .. },
                offset: f0,
                soffset: s0,
                ..
            },
            Fields::Smem {
                base: Operand::Scalar { code: b1, .. },
                offset: f1,
                soffset: s1,
                ..
            },
        ) = (&inst.fields, &o.fields)
        else {
            return None;
        };
        let reads = |i: &Inst, code: u8| (i.read(Reg::S(code)), i.read(Reg::S(code + 1)));
        let same_soffset = match (s0, s1) {
            (Operand::Scalar { code: NULL, .. }, Operand::Scalar { code: NULL, .. }) => true,
            (Operand::Scalar { code: c0, .. }, Operand::Scalar { code: c1, .. }) => {
                inst.read(Reg::S(*c0)).is_some() && inst.read(Reg::S(*c0)) == o.read(Reg::S(*c1))
            }
            _ => false,
        };
        let delta = f0.wrapping_sub(*f1);
        (reads(inst, *b0).0.is_some()
            && reads(inst, *b0) == reads(o, *b1)
            && same_soffset
            && delta % 4 == 0)
            .then_some(delta / 4)
    }

    /// Whether an instruction or phi reads `v` other than as a resource
    /// operand of a memory instruction (which takes the descriptor whole).
    fn read_as_data(&self, v: Value) -> bool {
        self.x.p.blocks.iter().any(|b| {
            b.phis.iter().any(|p| p.args.contains(&v))
                || b.insts.iter().any(|i| {
                    !matches!(
                        i.fields,
                        Fields::Image { .. } | Fields::Buffer { .. } | Fields::Smem { .. }
                    ) && i.reads.iter().any(|r| r.1 == v)
                })
        })
    }

    /// `s_load_dword*` from the program's constant table.
    fn code_smem(&mut self, inst: &Inst, a: Address) -> Result<(), Why> {
        let Fields::Smem {
            data: Some(Operand::Scalar { code, .. }),
            ..
        } = inst.fields
        else {
            return Err("a scalar load");
        };
        let count = inst
            .name()
            .strip_prefix("s_load_dword")
            .and_then(smem_dwords)
            .ok_or("a scalar load from the code")?;
        let c = self.cur;
        let mut addr = self.s.m.i32(a.offset);
        if let Some(d) = self.dynamic_term(&a)? {
            addr = self.s.m.bin(c, Bin::Add, addr, d);
        }
        for k in 0..count {
            let off = self.s.m.i32(4 * k);
            let at = self.s.m.bin(c, Bin::Add, addr, off);
            let x = self.code_dword(at)?;
            self.write(inst, Reg::S(code + k as u8), x, None);
        }
        Ok(())
    }

    /// The dword at a computed byte address of a constant buffer.
    fn cbuffer_dword(&mut self, h: V, a: V) -> V {
        let c = self.cur;
        let four = self.s.m.i32(4);
        let ri = self.s.m.bin(c, Bin::LShr, a, four);
        let r = self.s.op(
            c,
            "cbufferLoadLegacy.i32",
            Attr::ReadOnly,
            self.s.t.cbuf_ret,
            59,
            &[h, ri],
        );
        let two = self.s.m.i32(2);
        let three = self.s.m.i32(3);
        let comp = self.s.m.bin(c, Bin::LShr, a, two);
        let comp = self.s.m.bin(c, Bin::And, comp, three);
        let mut x = self.s.m.extract(c, r, 0);
        for j in 1..4 {
            let e = self.s.m.extract(c, r, j);
            let jc = self.s.m.i32(j);
            let is = self.s.m.cmp(c, pred::EQ, comp, jc);
            x = self.s.m.select(c, is, e, x);
        }
        x
    }

    /// `channels` channels of the element at byte `off` of raw buffer `h`,
    /// converted from the GFX10 buffer format `format` as the hardware's
    /// typed loads do (channels past the format's read 0, 0, 0, 1).
    // shortcut: the descriptor's channel swizzle is taken as the identity
    // (it is for raw views and the descriptors code builds), elements of 4
    // bytes or more as dword-aligned. Norm formats divide, which the AMD
    // driver does 1 ULP off the hardware's correctly rounded conversion for
    // some values; correct it with an integer check if a title compares
    // converted values exactly.
    fn format_load(&mut self, h: V, off: V, format: u8, channels: u32) -> Result<Vec<V>, Why> {
        let (bits, numeric) = buffer_format(format).ok_or("a buffer format")?;
        let c = self.cur;
        let total: u32 = bits.iter().sum();
        let (u, align) = (self.s.m.undef(self.s.t.i32), self.s.m.i32(4));
        let load = |l: &mut Self, at: V, n: u32| -> Vec<V> {
            let mask = l.s.m.i8((1u8 << n) - 1);
            let r = l.s.op(
                c,
                "rawBufferLoad.f32",
                Attr::ReadOnly,
                l.s.t.res_ret,
                139,
                &[h, at, u, mask, align],
            );
            (0..n)
                .map(|k| {
                    let x = l.s.m.extract(c, r, k);
                    l.as_int(x)
                })
                .collect()
        };
        let words = if total < 32 {
            // A byte or half inside the dword that holds it.
            let (not3, three) = (self.s.m.i32(!3), self.s.m.i32(3));
            let at = self.s.m.bin(c, Bin::And, off, not3);
            let w = load(self, at, 1)[0];
            let byte = self.s.m.bin(c, Bin::And, off, three);
            let sh = self.s.m.bin(c, Bin::Shl, byte, three);
            vec![self.s.m.bin(c, Bin::LShr, w, sh)]
        } else {
            load(self, off, total / 32)
        };
        let float = |l: &mut Self, f: V| l.as_int(f);
        let mut out = Vec::new();
        let mut at = 0;
        for &w in bits {
            let word = words[(at / 32) as usize];
            let field = if w == 32 {
                word
            } else {
                let sh = self.s.m.i32(at % 32);
                let t = self.s.m.bin(c, Bin::LShr, word, sh);
                let m = self.s.m.i32((1 << w) - 1);
                self.s.m.bin(c, Bin::And, t, m)
            };
            at += w;
            // Sign-extended from the field's top bit.
            let signed = |l: &mut Self| {
                if w == 32 {
                    return field;
                }
                let k = l.s.m.i32(32 - w);
                let t = l.s.m.bin(c, Bin::Shl, field, k);
                l.s.m.bin(c, Bin::AShr, t, k)
            };
            let f32t = self.s.t.f32;
            let x = match numeric {
                Numeric::Unorm => {
                    let f = self.s.m.cast(c, Cast::UiToFp, field, f32t);
                    let max = self.s.m.f32((((1u64 << w) - 1) as f32).to_bits());
                    let q = self.s.m.bin(c, Bin::SDiv, f, max);
                    // DXIL's division may be off by an ulp (65535 / 65535 =
                    // 0.99999994 on AMD); the format gives exactly 1.0 for
                    // the largest value, which an f16 conversion keeps.
                    let top = self.s.m.i32(((1u64 << w) - 1) as u32);
                    let full = self.s.m.cmp(c, pred::EQ, field, top);
                    let one = self.s.m.f32(1.0f32.to_bits());
                    let q = self.s.m.select(c, full, one, q);
                    float(self, q)
                }
                Numeric::Snorm => {
                    let s = signed(self);
                    let f = self.s.m.cast(c, Cast::SiToFp, s, f32t);
                    let max = self.s.m.f32((((1u64 << (w - 1)) - 1) as f32).to_bits());
                    let q = self.s.m.bin(c, Bin::SDiv, f, max);
                    let lo = self.s.m.f32((-1.0f32).to_bits());
                    let q = self.binary(35, q, lo);
                    float(self, q)
                }
                Numeric::Uscaled => {
                    let f = self.s.m.cast(c, Cast::UiToFp, field, f32t);
                    float(self, f)
                }
                Numeric::Sscaled => {
                    let s = signed(self);
                    let f = self.s.m.cast(c, Cast::SiToFp, s, f32t);
                    float(self, f)
                }
                Numeric::Uint => field,
                Numeric::Sint => signed(self),
                Numeric::Float if w == 32 => field,
                Numeric::Float => {
                    let h = match half_shift(w) {
                        0 => field,
                        k => {
                            let sh = self.s.m.i32(k);
                            self.s.m.bin(c, Bin::Shl, field, sh)
                        }
                    };
                    let f = self
                        .s
                        .op(c, "legacyF16ToF32", Attr::ReadNone, f32t, 131, &[h]);
                    float(self, f)
                }
            };
            out.push(x);
        }
        let one = match numeric {
            Numeric::Uint | Numeric::Sint => 1,
            _ => 1.0f32.to_bits(),
        };
        for k in out.len() as u32..channels {
            out.push(self.s.m.i32(if k == 3 { one } else { 0 }));
        }
        out.truncate(channels as usize);
        Ok(out)
    }

    /// Channels of buffer format `format`, as [`Self::format_load`] gives
    /// them, as d16 data: 16 bits each, two per dword, low half first (an
    /// odd last high half 0). Integer formats keep their low 16 bits; the
    /// others become f16.
    fn d16_channels(&mut self, format: u8, channels: Vec<V>) -> Result<Vec<V>, Why> {
        let (_, numeric) = buffer_format(format).ok_or("a buffer format")?;
        let c = self.cur;
        let mut halves = Vec::new();
        for x in channels {
            halves.push(if matches!(numeric, Numeric::Uint | Numeric::Sint) {
                let m = self.s.m.i32(0xFFFF);
                self.s.m.bin(c, Bin::And, x, m)
            } else {
                let (f, i32t) = (self.as_float(x), self.s.t.i32);
                self.s
                    .op(c, "legacyF32ToF16", Attr::ReadNone, i32t, 130, &[f])
            });
        }
        let mut out = Vec::new();
        for pair in halves.chunks(2) {
            out.push(match *pair {
                [lo, hi] => {
                    let k16 = self.s.m.i32(16);
                    let hi = self.s.m.bin(c, Bin::Shl, hi, k16);
                    self.s.m.bin(c, Bin::Or, lo, hi)
                }
                [lo] => lo,
                _ => unreachable!("chunks of one or two"),
            });
        }
        Ok(out)
    }

    /// The byte offset a buffer descriptor built around a root descriptor adds
    /// (constant and computed parts); none for other descriptors.
    fn built_offset(&mut self, src: &Source) -> Result<(u32, Option<V>), Why> {
        match src {
            Source::Built { address, .. } => {
                let x = self.dynamic_term(address)?;
                Ok((address.offset, x))
            }
            _ => Ok((0, None)),
        }
    }

    /// The byte offset of element `index` at `off` in a buffer whose
    /// descriptor the code builds around a root descriptor; none for other
    /// descriptors. GFX10 addresses base + offset + stride * index with
    /// swizzling off, the stride in dword 1 bits 16-29 (the constant bits the
    /// code sets there), swizzling in dword 1 bit 31 and the lane id added to
    /// the index in dword 3 bit 23.
    fn built_element(&mut self, src: &Source, index: V, off: V) -> Result<Option<V>, Why> {
        let Source::Built { bits, words, .. } = src else {
            return Ok(None);
        };
        if bits >> 31 != 0 {
            return Err("a swizzled buffer");
        }
        if words[1] >> 23 & 1 != 0 {
            return Err("a buffer indexed by lane");
        }
        let stride = bits >> 16 & 0x3FFF;
        let c = self.cur;
        let s = self.s.m.i32(stride);
        let rows = self.s.m.bin(c, Bin::Mul, index, s);
        Ok(Some(self.s.m.bin(c, Bin::Add, rows, off)))
    }

    /// Whether a buffer is accessed by its address (see
    /// [`crate::resource::track_with`]): no root view leads to it, or one
    /// that is written through.
    fn by_address(&self, src: &Source) -> bool {
        match src {
            Source::Address => true,
            Source::Built { address, .. } => {
                self.x.res.addresses
                    && address.param != CODE
                    && self
                        .x
                        .native
                        .layout
                        .get(address.param)
                        .is_some_and(|rp| rp.root == RootType::Descriptor)
                    && !self
                        .used_kind(src)
                        .is_some_and(|u| u.kind == ResourceType::Cbv)
            }
            _ => false,
        }
    }

    /// The used-resource entry a descriptor loaded from a table falls in.
    fn used_kind(&self, src: &Source) -> Option<&'a UsedResource> {
        if let Source::Phi { args, .. }
        | Source::Select { args, .. }
        | Source::Lanes { args, .. }
        | Source::Paths { args, .. } = src
        {
            return args.iter().find_map(|a| self.used_kind(a));
        }
        if let Source::Built { address, .. } = src {
            let rp = self.x.native.layout.get(address.param)?;
            return self
                .x
                .native
                .used
                .iter()
                .find(|u| u.root == RootType::Descriptor && u.user_data == rp.user_data);
        }
        let (Source::Loaded { load, first } | Source::Fetched { load, first }) = src else {
            return None;
        };
        let a = self.scalar.get(load)?;
        let rp = self.x.native.layout.get(a.param)?;
        if rp.root != RootType::Table {
            return None;
        }
        let dword = (a.offset / 4) + u32::from(*first);
        self.x
            .native
            .used
            .iter()
            .filter(|u| {
                u.root == RootType::Table
                    && u.user_data == rp.user_data
                    && u.table_offset_dwords <= dword
            })
            .max_by_key(|u| u.table_offset_dwords)
    }

    /// A handle for the descriptor `src` read at `site`.
    fn handle(
        &mut self,
        site: Site,
        src: &Source,
        sampler: bool,
        props: (u32, u32),
    ) -> Result<V, Why> {
        if let Some(&h) = self.made.get(&site).filter(|_| self.by_address(src)) {
            return Ok(h);
        }
        match src {
            Source::Address => Err("a buffer descriptor made in code"),
            Source::Loaded { load, first } => {
                // The element type is part of the key: one UAV read as float
                // and used atomically needs two handles (an atomic on a float
                // handle is invalid and crashed the NVIDIA compiler).
                let key = (self.cur, *load, *first, sampler, props);
                if let Some(&h) = self.handles.get(&key) {
                    return Ok(h);
                }
                let index = self.heap_index(*load, *first, sampler)?;
                let h = self.s.heap_handle(self.cur, index, sampler, false, props);
                self.handles.insert(key, h);
                Ok(h)
            }
            Source::Constant(words) if sampler => {
                let words: [u32; 4] = words
                    .get(..4)
                    .and_then(|w| w.try_into().ok())
                    .ok_or("a sampler constant")?;
                let n = match self.samplers.iter().position(|w| *w == words) {
                    Some(n) => n,
                    None => {
                        self.samplers.push(words);
                        self.samplers.len() - 1
                    }
                };
                if props.0 & 0x8000 != 0 {
                    // shortcut: one sampler used with and without comparison
                    // would need two static samplers; it becomes a comparison one.
                    self.s.cmp_samplers |= 1 << n;
                }
                Ok(self.s.sampler_handle(self.cur, n as u32))
            }
            Source::Constant(_) => Err("a constant resource descriptor"),
            Source::Built { address, .. } => {
                let rp = *self
                    .x
                    .native
                    .layout
                    .get(address.param)
                    .ok_or("a descriptor built around a table in the code")?;
                let kind = self
                    .x
                    .native
                    .used
                    .iter()
                    .find(|u| u.user_data == rp.user_data)
                    .map(|u| u.kind);
                match (rp.root, kind, props.0) {
                    (RootType::Descriptor, Some(ResourceType::Cbv) | None, 13) => {
                        Ok(self.s.root_cbv_handle(rp.slot))
                    }
                    (RootType::Descriptor, Some(ResourceType::Srv), 11) => {
                        let h = self.s.root_srv_handle(rp.slot);
                        // A draw with wild vertex numbers must not read past
                        // the memory behind the view (a page fault).
                        if super::limits_indices()
                            && (self.s.threads.is_none() || self.s.mesh.is_some())
                            && !self.s.limits.iter().any(|l| l.0 == h)
                        {
                            let nth = 1 + self.s.limits.len() as u32;
                            let limit = self.host_data(nth, RootType::LIMIT, rp.slot.into());
                            self.s.limits.push((h, limit));
                        }
                        Ok(h)
                    }
                    (RootType::Descriptor, Some(ResourceType::Uav) | None, 0x100B)
                    | (RootType::Descriptor, Some(ResourceType::Uav), 11) => {
                        // Other groups write what a builder reads.
                        let shared = self.ordered();
                        Ok(self.s.root_uav_handle(rp.slot, shared))
                    }
                    _ => Err("a root UAV or typed root view"),
                }
            }
            // shortcut: of samplers given in the code that a branch picks one
            // of (how they treat coordinates off the texture differs), the
            // first is taken.
            Source::Phi { args, .. }
                if sampler
                    && let Some(arg) = args.iter().find(|a| matches!(a, Source::Constant(_))) =>
            {
                self.handle(site, arg, sampler, props)
            }
            Source::Phi { block, args } => {
                // Each predecessor computes its heap index at its end; the
                // join picks one with a phi.
                let _ = site;
                let mut incoming = Vec::new();
                for (&q, arg) in self.x.p.blocks[*block].preds.iter().zip(args) {
                    if self.block[q].is_none() {
                        continue;
                    }
                    let Some(tail) = self.tail[q] else {
                        return Err("a descriptor chosen by control flow");
                    };
                    let at = self.edges.get(&(q, *block)).copied().unwrap_or(tail);
                    let here = std::mem::replace(&mut self.cur, at);
                    let index = match arg {
                        Source::Loaded { load, first } => self.heap_index(*load, *first, sampler),
                        Source::Phi { .. } | Source::Select { .. } => self.heap_slot(arg, sampler),
                        Source::Unset => Ok(self.s.m.i32(0)),
                        _ => {
                            if std::env::var_os("VICTORY_WHY").is_some() {
                                eprintln!("phi of {arg:?}");
                            }
                            Err("a descriptor chosen by control flow")
                        }
                    };
                    self.cur = here;
                    incoming.push((index?, at));
                }
                let join = self.block[*block].ok_or("a descriptor chosen by control flow")?;
                let phi = self.s.m.phi(join, self.s.t.i32);
                self.s.m.set_phi(phi, incoming);
                Ok(self.s.heap_handle(self.cur, phi, sampler, false, props))
            }
            Source::Select { scc, args } => {
                // The heap index of each choice, picked by SCC (uniform).
                let [
                    Source::Loaded { load: a, first: fa },
                    Source::Loaded { load: b, first: fb },
                ] = args[..]
                else {
                    return Err("a descriptor chosen by a condition");
                };
                let cond = match self.get(*scc)? {
                    Val::Bit(x) => x,
                    _ => return Err("SCC without a value"),
                };
                let set = self.heap_index(a, fa, sampler)?;
                let clear = self.heap_index(b, fb, sampler)?;
                let index = self.s.m.select(self.cur, cond, set, clear);
                Ok(self.s.heap_handle(self.cur, index, sampler, false, props))
            }
            Source::Fetched { .. } | Source::Lanes { .. } | Source::Paths { .. } => {
                // A descriptor each lane has its own of (a waterfall loop
                // reads it from the first active lane): each lane's heap
                // index, non-uniform.
                let io = &self.x.io.inputs;
                if self
                    .x
                    .res
                    .waterfall
                    .unset
                    .iter()
                    .any(|u| io.iter().any(|i| i.0 == *u))
                {
                    return Err("a per-lane descriptor joined with a VGPR the hardware sets");
                }
                let index = self.lane_heap_index(src, sampler)?;
                Ok(self.s.heap_handle(self.cur, index, sampler, true, props))
            }
            Source::Carried { .. } | Source::Unset => Err("a per-lane descriptor out of a join"),
        }
    }

    /// The heap index of a descriptor that may differ between lanes,
    /// computed in the current block.
    fn lane_heap_index(&mut self, src: &Source, sampler: bool) -> Result<V, Why> {
        match src {
            Source::Loaded { load, first } => self.heap_index(*load, *first, sampler),
            Source::Fetched { load, first } => self.fetched_index(*load, *first, sampler),
            Source::Lanes { mask, args } | Source::Select { scc: mask, args } => {
                let [a, b] = &args[..] else {
                    return Err("a descriptor chosen per lane");
                };
                let cond = if matches!(src, Source::Lanes { .. }) {
                    let &m = self
                        .x
                        .st
                        .masks
                        .get(mask)
                        .ok_or("a descriptor chosen per lane by an untracked mask")?;
                    self.boolean(m, self.cur)?
                } else {
                    match self.get(*mask)? {
                        Val::Bit(x) => x,
                        _ => return Err("SCC without a value"),
                    }
                };
                let set = self.lane_heap_index(a, sampler)?;
                let clear = self.lane_heap_index(b, sampler)?;
                Ok(self.s.m.select(self.cur, cond, set, clear))
            }
            Source::Paths { block, args } => self.paths_index(*block, args, sampler),
            Source::Carried { block } => self
                .lane_paths
                .iter()
                .rev()
                .find(|p| p.0 == *block)
                .map(|p| p.1)
                .ok_or("a per-lane descriptor carried out of its loop"),
            // Undefined: any index will do.
            Source::Unset => Ok(self.s.m.i32(0)),
            _ => Err("a per-lane descriptor from neither a heap load nor a choice of them"),
        }
    }

    /// The heap index of a descriptor joined per lane at `block`: a phi of
    /// each predecessor's index, computed at its end (later for those not
    /// lowered yet, as a loop's back edge).
    fn paths_index(&mut self, block: usize, args: &[Source], sampler: bool) -> Result<V, Why> {
        const WHY: Why = "a per-lane descriptor joined by control flow";
        let join = self.block[block].ok_or(WHY)?;
        let phi = self.s.m.phi(join, self.s.t.i32);
        self.lane_paths.push((block, phi));
        let here = self.cur;
        let mut incoming = Vec::new();
        let mut result = Ok(());
        for (&q, arg) in self.x.p.blocks[block].preds.iter().zip(args) {
            if self.block[q].is_none() {
                continue;
            }
            let Some(tail) = self.tail[q] else {
                incoming.push((q, Err(arg.clone())));
                continue;
            };
            self.cur = self.edges.get(&(q, block)).copied().unwrap_or(tail);
            let index = self.lane_heap_index(arg, sampler);
            match index {
                Ok(x) => incoming.push((q, Ok(x))),
                Err(why) => {
                    result = Err(why);
                    break;
                }
            }
        }
        self.cur = here;
        let outer = self.lane_paths.clone();
        self.lane_paths.pop();
        result?;
        if incoming.iter().all(|i| i.1.is_ok()) {
            let done = incoming
                .into_iter()
                .map(|(q, x)| (x.unwrap(), self.edge_block(q, block)));
            let done = done.collect();
            self.s.m.set_phi(phi, done);
        } else {
            self.pending_paths.push(PendingPath {
                phi,
                block,
                sampler,
                outer,
                incoming,
            });
        }
        Ok(phi)
    }

    /// The DXIL block that ends the edge from IR block `q` (lowered) to `to`.
    fn edge_block(&self, q: usize, to: usize) -> usize {
        let tail = self.tail[q].expect("a lowered predecessor");
        self.edges.get(&(q, to)).copied().unwrap_or(tail)
    }

    /// Fills the heap index phis [`Self::paths_index`] left for later.
    fn fill_paths(&mut self) -> Result<(), Why> {
        while let Some(pending) = self.pending_paths.pop() {
            let saved = std::mem::replace(&mut self.lane_paths, pending.outer);
            let mut incoming = Vec::new();
            for (q, x) in pending.incoming {
                let at = self.edge_block(q, pending.block);
                let x = match x {
                    Ok(x) => x,
                    Err(arg) => {
                        self.cur = at;
                        self.lane_heap_index(&arg, pending.sampler)?
                    }
                };
                incoming.push((x, at));
            }
            self.lane_paths = saved;
            self.s.m.set_phi(pending.phi, incoming);
        }
        Ok(())
    }

    /// The heap index of the descriptor at dword `first` of what the global
    /// load at `load` reads, in each lane: its address (base plus offsets)
    /// over the descriptor size.
    fn fetched_index(&mut self, load: Site, first: u8, sampler: bool) -> Result<V, Why> {
        const WHY: Why = "a per-lane descriptor load";
        let a = *self
            .scalar
            .get(&load)
            .ok_or("an untracked descriptor load")?;
        match self.x.native.layout.get(a.param).map(|p| p.root) {
            Some(RootType::Table | RootType::ResourceHeap | RootType::SamplerHeap) => {}
            _ => return Err("a descriptor outside the heaps"),
        }
        if self.x.st.guards[load.0][load.1].is_some() {
            // Lanes outside EXEC there keep their old dwords.
            return Err("a per-lane descriptor loaded under a partial EXEC");
        }
        let li: &Inst = &self.x.p.blocks[load.0].insts[load.1];
        if matches!(li.fields, Fields::Buffer { .. }) {
            // A buffer over the heap or a table: its address (the table's
            // byte offset, or 0 for a heap) plus each lane's offset.
            let c = self.cur;
            let mut at = match self.user(a.param, 0) {
                Val::I32(x) => x,
                _ => return Err(WHY),
            };
            let k = self.s.m.i32(a.offset.wrapping_add(4 * u32::from(first)));
            at = self.s.m.bin(c, Bin::Add, at, k);
            if let Some(d) = self.dynamic_term(&a)? {
                at = self.s.m.bin(c, Bin::Add, at, d);
            }
            let shift = self.s.m.i32(if sampler { 4 } else { 5 });
            return Ok(self.s.m.bin(c, Bin::LShr, at, shift));
        }
        let Fields::Flat {
            addr: Some(Operand::Vector { reg, .. }),
            saddr,
            offset,
            ..
        } = li.fields
        else {
            return Err(WHY);
        };
        let c = self.cur;
        // The low dword of the address: SGPR base plus VGPR offset, or the
        // VGPR pair's low half (heap addresses fit in 32 bits).
        let mut at = self.i32_of(li.read(Reg::V(reg)).ok_or(WHY)?)?;
        if let Some(Operand::Scalar { code, .. }) = saddr
            && code != NULL
        {
            let base = self.i32_of(li.read(Reg::S(code)).ok_or(WHY)?)?;
            at = self.s.m.bin(c, Bin::Add, base, at);
        }
        let k = self
            .s
            .m
            .i32((offset as i32 as u32).wrapping_add(4 * u32::from(first)));
        at = self.s.m.bin(c, Bin::Add, at, k);
        let shift = self.s.m.i32(if sampler { 4 } else { 5 });
        Ok(self.s.m.bin(c, Bin::LShr, at, shift))
    }

    /// The view-heap slot of `src` when it comes from a descriptor table or
    /// heap (`Loaded` or `Phi` of `Loaded`).
    fn heap_slot(&mut self, src: &Source, sampler: bool) -> Result<V, Why> {
        match src {
            Source::Loaded { load, first } => self.heap_index(*load, *first, sampler),
            Source::Phi { block, args } => {
                let mut incoming = Vec::new();
                for (&q, arg) in self.x.p.blocks[*block].preds.iter().zip(args) {
                    if self.block[q].is_none() {
                        continue;
                    }
                    let Some(tail) = self.tail[q] else {
                        return Err("a descriptor chosen by control flow");
                    };
                    let at = self.edges.get(&(q, *block)).copied().unwrap_or(tail);
                    let here = std::mem::replace(&mut self.cur, at);
                    let index = match arg {
                        Source::Loaded { load, first } => self.heap_index(*load, *first, sampler),
                        Source::Phi { .. } | Source::Select { .. } => self.heap_slot(arg, sampler),
                        Source::Unset => Ok(self.s.m.i32(0)),
                        _ => {
                            if std::env::var_os("VICTORY_WHY").is_some() {
                                eprintln!("phi of {arg:?}");
                            }
                            Err("a descriptor chosen by control flow")
                        }
                    };
                    self.cur = here;
                    incoming.push((index?, at));
                }
                let join = self.block[*block].ok_or("a descriptor chosen by control flow")?;
                let phi = self.s.m.phi(join, self.s.t.i32);
                self.s.m.set_phi(phi, incoming);
                Ok(phi)
            }
            // A descriptor chosen per lane or by SCC: its index in each lane.
            _ => self.lane_heap_index(src, sampler),
        }
    }

    /// Byte offset `index * stride + off` for a structured buffer whose
    /// stride was not recovered statically, reading `stride` from the host's
    /// runtime stride table (`t0`, [`super::STRIDE_SPACE`]).
    fn runtime_stride_offset(&mut self, src: &Source, index: V, off: V) -> Result<V, Why> {
        let slot = self
            .heap_slot(src, false)
            .map_err(|_| "a structured buffer of unknown stride")?;
        let c = self.cur;
        let stride = self.slot_dword(slot, 0);
        let elem_off = self.s.m.bin(c, Bin::Mul, index, stride);
        Ok(self.s.m.bin(c, Bin::Add, elem_off, off))
    }

    /// What the host's table by heap slot (`t0`, [`super::STRIDE_SPACE`])
    /// holds for `slot`, `from` dwords on: the element stride of its buffer
    /// view from 0, the element count from [`super::OWN_SLOTS`].
    fn slot_dword(&mut self, slot: V, from: u32) -> V {
        let c = self.cur;
        let table = self.s.stride_table_handle();
        // A root view has no bounds: a wild index would read past the table.
        let slot = if super::limits_indices() || from != 0 {
            let (i32t, last) = (self.s.t.i32, self.s.m.i32(super::OWN_SLOTS - 1));
            self.s.op(c, "binary.i32", Attr::ReadNone, i32t, 40, &[slot, last])
        } else {
            slot
        };
        let slot = if from != 0 {
            let from = self.s.m.i32(from);
            self.s.m.bin(c, Bin::Add, slot, from)
        } else {
            slot
        };
        let four = self.s.m.i32(4);
        let addr = self.s.m.bin(c, Bin::Mul, slot, four);
        let iu = self.s.m.undef(self.s.t.i32);
        let mask = self.s.m.i8(1);
        let align = self.s.m.i32(4);
        let res = self.s.op(
            c,
            "rawBufferLoad.f32",
            Attr::ReadOnly,
            self.s.t.res_ret,
            139,
            &[table, addr, iu, mask, align],
        );
        let x = self.s.m.extract(c, res, 0);
        self.as_int(x)
    }

    /// The heap index of the descriptor at dword `first` of what the scalar
    /// load at `load` reads, computed in the current block.
    fn heap_index(&mut self, load: Site, first: u8, sampler: bool) -> Result<V, Why> {
        self.heap_index_at(load, i32::from(first), sampler)
    }

    /// The heap index of the descriptor `dwords` dwords from the first one
    /// the scalar load at `load` reads (before it if negative), computed
    /// in the current block.
    fn heap_index_at(&mut self, load: Site, dwords: i32, sampler: bool) -> Result<V, Why> {
        let a = *self
            .scalar
            .get(&load)
            .ok_or("an untracked descriptor load")?;
        match self.x.native.layout.get(a.param).map(|p| p.root) {
            Some(RootType::Table | RootType::ResourceHeap | RootType::SamplerHeap) => {}
            _ => return Err("a descriptor outside the heaps"),
        }
        let li: &Inst = &self.x.p.blocks[load.0].insts[load.1];
        let Fields::Smem {
            base,
            offset,
            soffset,
            ..
        } = &li.fields
        else {
            return Err("a descriptor load");
        };
        let Operand::Scalar { code, .. } = *base else {
            return Err("a descriptor load");
        };
        let c = self.cur;
        let base = li.read(Reg::S(code)).ok_or("a descriptor load")?;
        let mut addr = self.i32_of(base)?;
        let k = self
            .s
            .m
            .i32((*offset as u32).wrapping_add(4u32.wrapping_mul(dwords as u32)));
        addr = self.s.m.bin(c, Bin::Add, addr, k);
        if !matches!(soffset, Operand::Scalar { code: NULL, .. }) {
            let so = self.operand(li, soffset)?;
            addr = self.s.m.bin(c, Bin::Add, addr, so);
        }
        let shift = self.s.m.i32(if sampler { 4 } else { 5 });
        Ok(self.s.m.bin(c, Bin::LShr, addr, shift))
    }

    fn valu(&mut self, inst: &Inst, b: usize, i: usize) -> Result<(), Why> {
        let Fields::Valu(v) = &inst.fields else {
            unreachable!()
        };
        let name = inst.name();
        if self.x.res.waterfall.compares.contains(&(b, i)) {
            // The first lane's dwords of a waterfall loop are each lane's
            // own (see `resource::Waterfall`).
            self.lane(inst, Some(true));
            return Ok(());
        }
        if let Some(rest) = name.strip_prefix("v_cmp_").or(name.strip_prefix("v_cmpx_")) {
            let r = if v.dpp.is_some() {
                Err("a compare with DPP")
            } else {
                self.compare(inst, rest)
            };
            if let Err(why) = r {
                for &(_, w) in &inst.writes {
                    self.lane_fail.insert(w, why);
                }
            }
            return r;
        }
        let mut guard = self.guard(b, i)?;
        if let Some(d) = v.dpp {
            let (x, enable) = self.dpp(inst, d)?;
            guard = match (guard, enable) {
                (Some(g), Some(e)) => Some(self.s.m.bin(self.cur, Bin::And, g, e)),
                (g, e) => g.or(e),
            };
            self.src0 = Some(x);
        }
        let r = self.valu_op(inst, guard);
        self.src0 = None;
        r
    }

    /// DPP: src0 as read from another lane, and which lanes write their
    /// result (none: all).
    fn dpp(&mut self, inst: &Inst, dpp: Dpp) -> Result<(V, Option<V>), Why> {
        let Fields::Valu(v) = &inst.fields else {
            unreachable!()
        };
        let Some(&Operand::Vector { reg, .. }) = v.src.first() else {
            return Err("a DPP operand");
        };
        let x = self.i32_of(inst.read(Reg::V(reg)).ok_or("a DPP operand")?)?;
        // Quad permutations and DPP8 stay in groups the pixel layout shares.
        let quad = matches!(dpp, Dpp::Row { ctrl: 0..=0xFF, .. });
        let mesh_quad = self.s.mesh.is_some() && quad;
        if !mesh_quad {
            self.cross_lane(quad)?;
        }
        let c = self.cur;
        let lane = self.lane_index();
        let k = |l: &mut Self, n: u32| l.s.m.i32(n);
        let (sixteen_mask, low) = (k(self, !15), k(self, 15));
        let row = self.s.m.bin(c, Bin::And, lane, sixteen_mask);
        let pos = self.s.m.bin(c, Bin::And, lane, low);
        // Source lane, and whether it lies in the row.
        let (src, inside, masks, zero) = match dpp {
            Dpp::Lanes { sel, .. } => {
                let (seven, not7, three) = (k(self, 7), k(self, !7), k(self, 3));
                let p = self.s.m.bin(c, Bin::And, lane, seven);
                let sh = self.s.m.bin(c, Bin::Mul, p, three);
                let sel = k(self, sel);
                let pick = self.s.m.bin(c, Bin::LShr, sel, sh);
                let pick = self.s.m.bin(c, Bin::And, pick, seven);
                let base = self.s.m.bin(c, Bin::And, lane, not7);
                (self.s.m.bin(c, Bin::Or, base, pick), None, None, false)
            }
            Dpp::Row {
                ctrl,
                row_mask,
                bank_mask,
                bound_ctrl,
                ..
            } => {
                let n = u32::from(ctrl & 15);
                let (src, inside) = match ctrl {
                    0..=0xFF => {
                        let (three, one, not3) = (k(self, 3), k(self, 1), k(self, !3));
                        let p = self.s.m.bin(c, Bin::And, lane, three);
                        let sh = self.s.m.bin(c, Bin::Shl, p, one);
                        let perm = k(self, u32::from(ctrl));
                        let pick = self.s.m.bin(c, Bin::LShr, perm, sh);
                        let pick = self.s.m.bin(c, Bin::And, pick, three);
                        let base = self.s.m.bin(c, Bin::And, lane, not3);
                        (self.s.m.bin(c, Bin::Or, base, pick), None)
                    }
                    0x101..=0x10F | 0x111..=0x11F => {
                        let nn = k(self, n);
                        let left = ctrl < 0x110;
                        let (op, p) = if left {
                            (Bin::Add, pred::IULT)
                        } else {
                            (Bin::Sub, pred::IUGE)
                        };
                        let s = self.s.m.bin(c, op, lane, nn);
                        // Left: pos + n < 16; right: pos >= n.
                        let bound = if left { k(self, 16 - n) } else { nn };
                        (s, Some(self.s.m.cmp(c, p, pos, bound)))
                    }
                    0x121..=0x12F => {
                        let nn = k(self, n);
                        let t = self.s.m.bin(c, Bin::Sub, pos, nn);
                        let t = self.s.m.bin(c, Bin::And, t, low);
                        (self.s.m.bin(c, Bin::Or, row, t), None)
                    }
                    0x140 | 0x141 => {
                        let m = k(self, if ctrl == 0x140 { 15 } else { 7 });
                        let t = self.s.m.bin(c, Bin::Xor, lane, m);
                        (t, None)
                    }
                    0x150..=0x15F => {
                        let nn = k(self, n);
                        (self.s.m.bin(c, Bin::Or, row, nn), None)
                    }
                    0x160..=0x16F => {
                        let nn = k(self, n);
                        (self.s.m.bin(c, Bin::Xor, lane, nn), None)
                    }
                    _ => return Err("a DPP lane pattern"),
                };
                let masks = (row_mask != 0xF || bank_mask != 0xF).then_some((row_mask, bank_mask));
                (src, inside, masks, bound_ctrl)
            }
        };
        let mut r = if mesh_quad || self.helpers_read(quad) {
            self.quad_pick(x, src)
        } else {
            self.read_lane(x, src)
        };
        let mut enable = None;
        if let Some(inside) = inside {
            if zero {
                let z = k(self, 0);
                r = self.s.m.select(c, inside, r, z);
            } else {
                enable = Some(inside);
            }
        }
        if let Some((rows, banks)) = masks {
            // Row (16 lanes) and bank (4 lanes of a row) write masks. Rows
            // are the console's: split in halves, the upper half's host rows
            // 0 and 1 are console rows 2 and 3.
            let (four, two, three) = (k(self, 4), k(self, 2), k(self, 3));
            let row_lane = if self.split {
                self.console_lane().0
            } else {
                lane
            };
            let ri = self.s.m.bin(c, Bin::LShr, row_lane, four);
            let ri = self.s.m.bin(c, Bin::And, ri, three);
            let bi = self.s.m.bin(c, Bin::LShr, lane, two);
            let bi = self.s.m.bin(c, Bin::And, bi, three);
            let (rm, bm) = (k(self, u32::from(rows)), k(self, u32::from(banks)));
            let rb = self.s.m.bin(c, Bin::LShr, rm, ri);
            let bb = self.s.m.bin(c, Bin::LShr, bm, bi);
            let both = self.s.m.bin(c, Bin::And, rb, bb);
            let (one, z) = (k(self, 1), k(self, 0));
            let both = self.s.m.bin(c, Bin::And, both, one);
            let on = self.s.m.cmp(c, pred::NE, both, z);
            enable = Some(match enable {
                Some(e) => self.s.m.bin(c, Bin::And, e, on),
                None => on,
            });
        }
        Ok((r, enable))
    }

    fn valu_op(&mut self, inst: &Inst, guard: Option<V>) -> Result<(), Why> {
        let Fields::Valu(v) = &inst.fields else {
            unreachable!()
        };
        let name = inst.name();
        let c = self.cur;
        let f = |l: &mut Self, k| l.fsrc(inst, k);
        let s = |l: &mut Self, k| l.src(inst, k);
        let float = |l: &mut Self, r: V| l.write_f(inst, r, guard);
        let int = |l: &mut Self, r: V| l.write_i(inst, r, guard);
        if let Some(r) = self.valu16(inst, name, guard) {
            return r;
        }
        if name == "v_pk_fmac_f16" {
            return self.packed_fmac(inst, guard);
        }
        if let Some(op) = name.strip_prefix("v_pk_") {
            return self.packed16(inst, op, guard);
        }
        match name {
            "v_frexp_exp_i16_f16" | "v_frexp_exp_i32_f32" | "v_frexp_exp_i32_f64"
            | "v_frexp_mant_f16" | "v_frexp_mant_f32" | "v_frexp_mant_f64" => {
                self.frexp(inst, guard)
            }
            // v_ldexp_f32 lowers below (exact while the result stays normal).
            "v_ldexp_f16" | "v_ldexp_f64" => {
                Err("ldexp requires the console floating-point rounding and denormal mode")
            }
            "v_mullit_f32" => Err("lighting multiply needs the console NaN, infinity and overflow rules"),
            "v_dot2_f32_f16" | "v_dot2c_f32_f16" => Err("half dot product rounding is unspecified by the emitter"),
            "v_fmamk_f16" | "v_fmaak_f16" => Err("half FMA requires a single rounding to f16"),
            n if n.starts_with("v_cvt_norm_") || n.starts_with("v_cvt_pknorm_") => {
                Err("normalized conversion needs the console rounding and NaN rules")
            }
            "v_mad_i32_i16" | "v_mad_u32_u16" => {
                let signed = !name.ends_with("u16");
                if v.omod != 0 || v.abs != 0 || v.neg != 0 || v.opsel & !3 != 0 {
                    return Err("modifiers on integer arithmetic");
                }
                let read = |l: &mut Self, i| -> Result<V, Why> {
                    let x = l.half_src(inst, i)?;
                    Ok(if signed { l.sign_extend(x, 16) } else { x })
                };
                let (a, b) = (read(self, 0)?, read(self, 1)?);
                let d = s(self, 2)?;
                let arithmetic = |l: &mut Self, a, b, d| {
                    let m = l.s.m.bin(c, Bin::Mul, a, b);
                    l.s.m.bin(c, Bin::Add, m, d)
                };
                let r = if v.clamp {
                    let (a, b) = (self.extend64(a, signed), self.extend64(b, signed));
                    let d = self.extend64(d, signed);
                    let r = arithmetic(self, a, b, d);
                    let (min, max) = if signed { (i32::MIN as i64, i32::MAX as i64) } else { (0, u32::MAX as i64) };
                    let r = self.clamp_int(r, 64, min, max, signed);
                    self.s.m.cast(c, Cast::Trunc, r, self.s.t.i32)
                } else {
                    arithmetic(self, a, b, d)
                };
                let Some(Operand::Vector { reg, .. }) = v.dst else { return Err("a result without a VGPR") };
                self.write(inst, Reg::V(reg), r, guard);
                Ok(())
            }
            "v_cvt_pk_u16_u32" | "v_cvt_pk_i16_i32" | "v_sat_pk_u8_i16" => {
                // LLVM's AMDGPU clamp combines use the saturating V_CVT_PK
                // conversions. V_SAT_PK clamps each signed half to a byte.
                let a = s(self, 0)?;
                let r = if name == "v_sat_pk_u8_i16" {
                    let (k16, k8) = (self.s.m.i32(16), self.s.m.i32(8));
                    let hi = self.s.m.bin(c, Bin::AShr, a, k16);
                    let lo = self.sign_extend(a, 16);
                    let lo = self.clamp_int(lo, 32, 0, 255, true);
                    let hi = self.clamp_int(hi, 32, 0, 255, true);
                    let hi = self.s.m.bin(c, Bin::Shl, hi, k8);
                    self.s.m.bin(c, Bin::Or, lo, hi)
                } else {
                    let b = s(self, 1)?;
                    let signed = name == "v_cvt_pk_i16_i32";
                    let (min, max) = if signed { (-32768, 32767) } else { (0, 65535) };
                    let (a, b) = (self.clamp_int(a, 32, min, max, signed), self.clamp_int(b, 32, min, max, signed));
                    self.pack_halves(a, b)
                };
                int(self, r)
            }
            "v_cvt_off_f32_i4" => {
                // RDNA 2 ISA, VOP1: a signed nibble in sixteenths of a pixel.
                let x = s(self, 0)?;
                let x = self.sign_extend(x, 4);
                let x = self.s.m.cast(c, Cast::SiToFp, x, self.s.t.f32);
                let step = self.s.m.f32(0.0625f32.to_bits());
                let x = self.s.m.bin(c, Bin::Mul, x, step);
                float(self, x)
            }
            "v_dot2_i32_i16" | "v_dot2_u32_u16" | "v_dot4_i32_i8" | "v_dot4c_i32_i8"
            | "v_dot4_u32_u8" | "v_dot8_u32_u4" => self.integer_dot(inst, guard),
            "v_mad_u64_u32" | "v_mad_i64_i32" => self.wide_mad(inst, guard),
            "v_qsad_pk_u16_u8" | "v_mqsad_pk_u16_u8" | "v_mqsad_u32_u8" => {
                if v.clamp || v.omod != 0 || v.abs != 0 || v.neg != 0 || v.opsel != 0 {
                    return Err("modifiers on a quad SAD");
                }
                let a = self.src64(inst, 0)?;
                let b = s(self, 1)?;
                let Some(Operand::Vector { reg, .. }) = v.dst else { return Err("a quad SAD result") };
                let acc = if name == "v_mqsad_u32_u8" {
                    self.vgprs(inst, &v.src[2], 4)?
                } else {
                    let acc = self.src64(inst, 2)?;
                    let (lo, hi) = self.split64(acc);
                    vec![lo, hi]
                };
                let mut packed_lo = None;
                for i in 0..4u8 {
                    let ty = self.s.m.int(64);
                    let sh = self.s.m.int_const(ty, i64::from(i) * 8);
                    let window = self.s.m.bin(c, Bin::LShr, a, sh);
                    let window = self.s.m.cast(c, Cast::Trunc, window, self.s.t.i32);
                    let diff = self.sad(window, b, 8, name != "v_qsad_pk_u16_u8");
                    if acc.len() == 4 {
                        let r = self.s.m.bin(c, Bin::Add, diff, acc[usize::from(i)]);
                        self.write(inst, Reg::V(reg + i), r, guard);
                    } else {
                        let d = self.int_field(acc[usize::from(i / 2)], u32::from(i % 2) * 16, 16);
                        let r = self.s.m.bin(c, Bin::Add, diff, d);
                        if let Some(lo) = packed_lo.take() {
                            let r = self.pack_halves(lo, r);
                            self.write(inst, Reg::V(reg + i / 2), r, guard);
                        } else {
                            packed_lo = Some(r);
                        }
                    }
                }
                Ok(())
            }
            "v_lshlrev_b64" | "v_lshrrev_b64" | "v_ashrrev_i64" => {
                let n = s(self, 0)?;
                let x = self.src64(inst, 1)?;
                let i64t = self.s.m.int(64);
                let k63 = self.s.m.i32(63);
                let n = self.s.m.bin(c, Bin::And, n, k63);
                let n = self.s.m.cast(c, Cast::ZExt, n, i64t);
                let op = match name {
                    "v_lshlrev_b64" => Bin::Shl,
                    "v_lshrrev_b64" => Bin::LShr,
                    _ => Bin::AShr,
                };
                let r = self.s.m.bin(c, op, x, n);
                let Some(Operand::Vector { reg, .. }) = v.dst else {
                    return Err("a 64-bit result");
                };
                let (lo, hi) = self.split64(r);
                self.write(inst, Reg::V(reg), lo, guard);
                self.write(inst, Reg::V(reg + 1), hi, guard);
                Ok(())
            }
            "v_cubeid_f32" | "v_cubesc_f32" | "v_cubetc_f32" | "v_cubema_f32" => {
                // Cube-map face coordinates (AMD's ISA): the major axis is z
                // when |z| >= |x| and |z| >= |y|, else y when |y| >= |x|,
                // else x.
                let (x, y, z) = (f(self, 0)?, f(self, 1)?, f(self, 2)?);
                let (ax, ay, az) = (self.unary(6, x), self.unary(6, y), self.unary(6, z));
                let zx = self.s.m.cmp(c, pred::OGE, az, ax);
                let zy = self.s.m.cmp(c, pred::OGE, az, ay);
                let zmaj = self.s.m.bin(c, Bin::And, zx, zy);
                let ymaj = self.s.m.cmp(c, pred::OGE, ay, ax);
                let zero = self.s.m.f32(0);
                let neg = |l: &mut Self, v: V| l.s.m.cmp(c, pred::OLT, v, zero);
                let minus = |l: &mut Self, v: V| {
                    let z = l.s.m.f32(0x8000_0000);
                    l.s.m.bin(c, Bin::Sub, z, v)
                };
                let pick = |l: &mut Self, zv: V, yv: V, xv: V| {
                    let t = l.s.m.select(c, ymaj, yv, xv);
                    l.s.m.select(c, zmaj, zv, t)
                };
                let r = match name {
                    "v_cubeid_f32" => {
                        let k = |l: &mut Self, v: f32| l.s.m.f32(v.to_bits());
                        let (nz, ny, nx) = (neg(self, z), neg(self, y), neg(self, x));
                        let (f5, f4, f3, f2, f1, f0) = (
                            k(self, 5.0),
                            k(self, 4.0),
                            k(self, 3.0),
                            k(self, 2.0),
                            k(self, 1.0),
                            k(self, 0.0),
                        );
                        let zf = self.s.m.select(c, nz, f5, f4);
                        let yf = self.s.m.select(c, ny, f3, f2);
                        let xf = self.s.m.select(c, nx, f1, f0);
                        pick(self, zf, yf, xf)
                    }
                    "v_cubesc_f32" => {
                        let (nz, nx) = (neg(self, z), neg(self, x));
                        let (mx, mz) = (minus(self, x), minus(self, z));
                        let zf = self.s.m.select(c, nz, mx, x);
                        let xf = self.s.m.select(c, nx, z, mz);
                        pick(self, zf, x, xf)
                    }
                    "v_cubetc_f32" => {
                        let ny = neg(self, y);
                        let (my, mz) = (minus(self, y), minus(self, z));
                        let yf = self.s.m.select(c, ny, mz, z);
                        pick(self, my, yf, my)
                    }
                    _ => {
                        let t = pick(self, z, y, x);
                        let two = self.s.m.f32(2.0f32.to_bits());
                        self.s.m.bin(c, Bin::Mul, t, two)
                    }
                };
                float(self, r)
            }
            "v_movrels_b32" | "v_movreld_b32" | "v_movrelsd_b32" => {
                // M0 indexes the source (movrels), the destination (movreld)
                // or both, from the encoded register up to the program's
                // VGPR count (the registers the instruction reads or writes).
                let m0 = self.i32_of(inst.read(Reg::S(crate::ir::M0)).ok_or("M0")?)?;
                let (
                    Some(&Operand::Vector { reg: sreg, .. }),
                    Some(Operand::Vector { reg: dreg, .. }),
                ) = (v.src.first(), v.dst)
                else {
                    return Err("an M0-relative move");
                };
                let indexed_src = name != "v_movreld_b32";
                let value = if indexed_src {
                    let mut r = self.s.m.i32(0);
                    for k in 0..=255u8.saturating_sub(sreg) {
                        let Some(x) = inst.read(Reg::V(sreg + k)) else {
                            break;
                        };
                        let x = self.i32_of(x)?;
                        let kk = self.s.m.i32(u32::from(k));
                        let is = self.s.m.cmp(c, pred::EQ, m0, kk);
                        r = self.s.m.select(c, is, x, r);
                    }
                    r
                } else {
                    s(self, 0)?
                };
                if name == "v_movrels_b32" {
                    return int(self, value);
                }
                for k in 0..=255u8.saturating_sub(dreg) {
                    let Some(_) = inst.write(Reg::V(dreg + k)) else {
                        break;
                    };
                    let old = match inst.read(Reg::V(dreg + k)) {
                        Some(o) => self.i32_of(o)?,
                        None => self.s.m.i32(0),
                    };
                    let kk = self.s.m.i32(u32::from(k));
                    let is = self.s.m.cmp(c, pred::EQ, m0, kk);
                    let x = self.s.m.select(c, is, value, old);
                    self.write_placed(inst, Reg::V(dreg + k), x, guard);
                }
                Ok(())
            }
            "v_swap_b32" => {
                let (
                    Some(&Operand::Vector { reg: sreg, .. }),
                    Some(Operand::Vector { reg: dreg, .. }),
                ) = (v.src.first(), v.dst)
                else {
                    return Err("v_swap_b32");
                };
                let a = self.i32_of(inst.read(Reg::V(dreg)).ok_or("v_swap_b32")?)?;
                let b = self.i32_of(inst.read(Reg::V(sreg)).ok_or("v_swap_b32")?)?;
                self.write_placed(inst, Reg::V(dreg), b, guard);
                if dreg != sreg {
                    self.write_placed(inst, Reg::V(sreg), a, guard);
                }
                Ok(())
            }
            "v_ffbh_u32" | "v_ffbh_i32" => {
                // Counted from the top bit, -1 when none (DXIL's too).
                let a = s(self, 0)?;
                let op = if name == "v_ffbh_u32" { 33 } else { 34 };
                let r = self
                    .s
                    .op(c, "unaryBits.i32", Attr::ReadNone, self.s.t.i32, op, &[a]);
                int(self, r)
            }
            "v_mov_b32" => {
                if v.abs != 0 || v.neg != 0 || v.clamp || v.omod != 0 {
                    let a = f(self, 0)?;
                    return float(self, a);
                }
                let a = s(self, 0)?;
                int(self, a)
            }
            "v_add_f32" | "v_sub_f32" | "v_subrev_f32" | "v_mul_f32" | "v_mul_legacy_f32"
            | "v_max_f32" | "v_min_f32" => {
                let (a, bb) = (f(self, 0)?, f(self, 1)?);
                let r = match name {
                    "v_add_f32" => self.s.m.bin(c, Bin::Add, a, bb),
                    "v_sub_f32" => self.s.m.bin(c, Bin::Sub, a, bb),
                    "v_subrev_f32" => self.s.m.bin(c, Bin::Sub, bb, a),
                    "v_mul_f32" => self.s.m.bin(c, Bin::Mul, a, bb),
                    "v_mul_legacy_f32" => self.legacy_mul(a, bb),
                    "v_max_f32" => self.max_min(true, a, bb),
                    _ => self.max_min(false, a, bb),
                };
                float(self, r)
            }
            "v_fma_f32" | "v_fmaak_f32" | "v_fmamk_f32" => {
                // shortcut: DXIL has no fused f32 multiply-add; FMad lets the driver fuse.
                let (a, bb, cc) = (f(self, 0)?, f(self, 1)?, f(self, 2)?);
                let r = self.fmad(a, bb, cc);
                float(self, r)
            }
            "v_div_scale_f32" => {
                // RDNA2 ISA: src0 = value to scale, src1 = denominator, src2 = numerator.
                let (a, den, num) = (f(self, 0)?, f(self, 1)?, f(self, 2)?);
                let (ai, ni, di) = (self.as_int(a), self.as_int(num), self.as_int(den));
                let (k23, k255, kabs) = (
                    self.s.m.i32(23),
                    self.s.m.i32(0xFF),
                    self.s.m.i32(0x7FFF_FFFF),
                );
                let (zero, k23_exp, k232_exp, k96) = (
                    self.s.m.i32(0),
                    self.s.m.i32(23),
                    self.s.m.i32(232),
                    self.s.m.i32(96),
                );
                let exp_of = |l: &mut Self, x: V| {
                    let s = l.s.m.bin(c, Bin::LShr, x, k23);
                    l.s.m.bin(c, Bin::And, s, k255)
                };
                let (e1, e2) = (exp_of(self, ni), exp_of(self, di));
                let n_abs = self.s.m.bin(c, Bin::And, ni, kabs);
                let d_abs = self.s.m.bin(c, Bin::And, di, kabs);
                let z1 = self.s.m.cmp(c, pred::EQ, n_abs, zero);
                let z2 = self.s.m.cmp(c, pred::EQ, d_abs, zero);
                let any_zero = self.s.m.bin(c, Bin::Or, z1, z2);
                let special = {
                    let s1 = self.s.m.cmp(c, pred::EQ, e1, k255);
                    let s2 = self.s.m.cmp(c, pred::EQ, e2, k255);
                    let a12 = self.s.m.bin(c, Bin::Or, s1, s2);
                    self.s.m.bin(c, Bin::Or, a12, any_zero)
                };
                let is_den = self.s.m.cmp(c, pred::EQ, ai, di);
                let diff = self.s.m.bin(c, Bin::Sub, e2, e1);
                let den_tiny = self.s.m.cmp(c, pred::IULE, e2, k23_exp);
                let den_huge = self.s.m.cmp(c, pred::IUGE, e2, k232_exp);
                let num_tiny_exp = self.s.m.cmp(c, pred::IULE, e1, k23_exp);
                let num_huge = self.s.m.cmp(c, pred::IUGE, e1, k232_exp);
                let num_gap = self.s.m.cmp(c, pred::SGE, diff, k96);
                let not_den_huge = {
                    let t = self.s.m.bool(true);
                    self.s.m.bin(c, Bin::Xor, den_huge, t)
                };
                let num_gap = self.s.m.bin(c, Bin::And, num_gap, not_den_huge);
                let num_tiny = self.s.m.bin(c, Bin::Or, num_tiny_exp, num_gap);
                let up_raw = self.s.m.select(c, is_den, den_tiny, num_tiny);
                let dn_raw = self.s.m.select(c, is_den, den_huge, num_huge);
                let not_special = {
                    let t = self.s.m.bool(true);
                    self.s.m.bin(c, Bin::Xor, special, t)
                };
                let up = self.s.m.bin(c, Bin::And, up_raw, not_special);
                let dn = self.s.m.bin(c, Bin::And, dn_raw, not_special);
                let uncancelled = {
                    let x_tiny = self.s.m.bin(c, Bin::Xor, den_tiny, num_tiny);
                    let x_huge = self.s.m.bin(c, Bin::Xor, den_huge, num_huge);
                    self.s.m.bin(c, Bin::Or, x_tiny, x_huge)
                };
                let flag_raw = self.s.m.select(c, is_den, uncancelled, up_raw);
                let flag = self.s.m.bin(c, Bin::And, flag_raw, not_special);
                let k_up = self.s.m.f32(0x5F80_0000);
                let k_dn = self.s.m.f32(0x1F80_0000);
                let a_up = self.s.m.bin(c, Bin::Mul, a, k_up);
                let a_dn = self.s.m.bin(c, Bin::Mul, a, k_dn);
                let r = self.s.m.select(c, dn, a_dn, a);
                let r = self.s.m.select(c, up, a_up, r);
                let qnan = self.s.m.f32(0x7FC0_0000);
                let r = self.s.m.select(c, any_zero, qnan, r);
                float(self, r)?;
                if let Some(Operand::Scalar { code, .. }) = v.carry
                    && let Some(w) = inst.write(Reg::S(code))
                {
                    self.lane_bits.insert(w, flag);
                }
                for &(_, w) in &inst.writes {
                    self.vals
                        .entry(w)
                        .or_insert(Val::Missing("a lane mask read as data"));
                }
                Ok(())
            }
            "v_div_fmas_f32" => {
                let (a, bb, cc) = (f(self, 0)?, f(self, 1)?, f(self, 2)?);
                let m = inst
                    .read(Reg::S(crate::ir::VCC))
                    .and_then(|v| self.x.st.masks.get(&v).copied())
                    .ok_or("v_div_fmas_f32 without VCC")?;
                let vcc = self.boolean(m, c)?;
                let tmp = self.fmad(a, bb, cc);
                let ci = self.as_int(cc);
                let (k23, k255, k128) = (self.s.m.i32(23), self.s.m.i32(0xFF), self.s.m.i32(128));
                let sh = self.s.m.bin(c, Bin::LShr, ci, k23);
                let exp = self.s.m.bin(c, Bin::And, sh, k255);
                let exp_ge = self.s.m.cmp(c, pred::IUGE, exp, k128);
                let (k_up, k_dn) = (self.s.m.f32(0x5F80_0000), self.s.m.f32(0x1F80_0000));
                let scale = self.s.m.select(c, exp_ge, k_up, k_dn);
                let scaled = self.s.m.bin(c, Bin::Mul, tmp, scale);
                let r = self.s.m.select(c, vcc, scaled, tmp);
                float(self, r)
            }
            "v_div_fixup_f32" => {
                let (q, den, num) = (f(self, 0)?, f(self, 1)?, f(self, 2)?);
                let (qi, di, ni) = (self.as_int(q), self.as_int(den), self.as_int(num));
                let (ksign, kabs, kinf) = (
                    self.s.m.i32(0x8000_0000),
                    self.s.m.i32(0x7FFF_FFFF),
                    self.s.m.i32(0x7F80_0000),
                );
                let (kqnan_bit, kneg_qnan, zero) = (
                    self.s.m.i32(0x0040_0000),
                    self.s.m.i32(0xFFC0_0000),
                    self.s.m.i32(0),
                );
                let sign = {
                    let x = self.s.m.bin(c, Bin::Xor, ni, di);
                    self.s.m.bin(c, Bin::And, x, ksign)
                };
                let q_abs = self.s.m.bin(c, Bin::And, qi, kabs);
                let d_abs = self.s.m.bin(c, Bin::And, di, kabs);
                let n_abs = self.s.m.bin(c, Bin::And, ni, kabs);
                let q_nan = self.s.m.cmp(c, pred::IUGT, q_abs, kinf);
                let d_nan = self.s.m.cmp(c, pred::IUGT, d_abs, kinf);
                let n_nan = self.s.m.cmp(c, pred::IUGT, n_abs, kinf);
                let d_inf = self.s.m.cmp(c, pred::EQ, d_abs, kinf);
                let n_inf = self.s.m.cmp(c, pred::EQ, n_abs, kinf);
                let d_zero = self.s.m.cmp(c, pred::EQ, d_abs, zero);
                let n_zero = self.s.m.cmp(c, pred::EQ, n_abs, zero);
                let both_inf = self.s.m.bin(c, Bin::And, n_inf, d_inf);
                let both_zero = self.s.m.bin(c, Bin::And, n_zero, d_zero);
                let invalid = self.s.m.bin(c, Bin::Or, both_inf, both_zero);
                let make_inf = {
                    let t = self.s.m.bin(c, Bin::Or, n_inf, d_zero);
                    self.s.m.bin(c, Bin::Or, t, q_nan)
                };
                let make_zero = self.s.m.bin(c, Bin::Or, d_inf, n_zero);
                let signed_q = self.s.m.bin(c, Bin::Or, sign, q_abs);
                let signed_inf = self.s.m.bin(c, Bin::Or, sign, kinf);
                let n_qnan = self.s.m.bin(c, Bin::Or, ni, kqnan_bit);
                let d_qnan = self.s.m.bin(c, Bin::Or, di, kqnan_bit);
                let r = self.s.m.select(c, make_zero, sign, signed_q);
                let r = self.s.m.select(c, make_inf, signed_inf, r);
                let r = self.s.m.select(c, invalid, kneg_qnan, r);
                let r = self.s.m.select(c, d_nan, d_qnan, r);
                let r = self.s.m.select(c, n_nan, n_qnan, r);
                let rf = self.as_float(r);
                float(self, rf)
            }
            "v_fmac_f32" | "v_mac_f32" | "v_fmac_legacy_f32" => {
                let (a, bb) = (f(self, 0)?, f(self, 1)?);
                let Some(Operand::Vector { reg, .. }) = v.dst else {
                    return Err("an accumulator");
                };
                let acc = inst.read(Reg::V(reg)).ok_or("an accumulator")?;
                let acc = match self.floats.get(&acc) {
                    Some(&x) => x,
                    None => {
                        let x = self.i32_of(acc)?;
                        self.as_float(x)
                    }
                };
                let r = match name {
                    "v_fmac_f32" => self.fmad(a, bb, acc),
                    "v_mac_f32" => {
                        let m = self.s.m.bin(c, Bin::Mul, a, bb);
                        self.s.m.bin(c, Bin::Add, m, acc)
                    }
                    _ => self.fma_legacy(a, bb, acc),
                };
                float(self, r)
            }
            "v_fma_legacy_f32" => {
                let (a, bb, cc) = (f(self, 0)?, f(self, 1)?, f(self, 2)?);
                let r = self.fma_legacy(a, bb, cc);
                float(self, r)
            }
            "v_mad_f32" | "v_madak_f32" | "v_madmk_f32" => {
                let (a, bb, cc) = (f(self, 0)?, f(self, 1)?, f(self, 2)?);
                let m = self.s.m.bin(c, Bin::Mul, a, bb);
                let r = self.s.m.bin(c, Bin::Add, m, cc);
                float(self, r)
            }
            "v_max3_f32" | "v_min3_f32" | "v_med3_f32" => {
                let (a, bb, cc) = (f(self, 0)?, f(self, 1)?, f(self, 2)?);
                let r = match name {
                    "v_max3_f32" => {
                        let t = self.max_min(true, a, bb);
                        self.max_min(true, t, cc)
                    }
                    "v_min3_f32" => {
                        let t = self.max_min(false, a, bb);
                        self.max_min(false, t, cc)
                    }
                    _ => self.med3(a, bb, cc),
                };
                float(self, r)
            }
            "v_rcp_f32" | "v_rcp_iflag_f32" => {
                // shortcut: the hardware reciprocal is approximate (1 ULP); this divides.
                let a = f(self, 0)?;
                let one = self.s.m.f32(1.0f32.to_bits());
                let r = self.s.m.bin(c, Bin::SDiv, one, a);
                float(self, r)
            }
            "v_rsq_f32" | "v_sqrt_f32" | "v_log_f32" | "v_exp_f32" | "v_fract_f32"
            | "v_trunc_f32" | "v_floor_f32" | "v_ceil_f32" | "v_rndne_f32" | "v_sin_f32"
            | "v_cos_f32" => {
                let a = f(self, 0)?;
                let op = match name {
                    "v_rsq_f32" => 25,
                    "v_sqrt_f32" => 24,
                    "v_log_f32" => 23,
                    "v_exp_f32" => 21,
                    "v_fract_f32" => 22,
                    "v_trunc_f32" => 29,
                    "v_floor_f32" => 27,
                    "v_ceil_f32" => 28,
                    "v_rndne_f32" => 26,
                    "v_sin_f32" => 13,
                    _ => 12,
                };
                let r = match op {
                    12 | 13 => self.trig_turns(a, op == 12),
                    22 => self.fract(a, 1.0, f32::from_bits(0x3F7F_FFFF)),
                    _ => self.unary(op, a),
                };
                float(self, r)
            }
            "v_cvt_i32_f32" | "v_cvt_u32_f32" | "v_cvt_flr_i32_f32" | "v_cvt_rpi_i32_f32" => {
                let mut a = f(self, 0)?;
                if name == "v_cvt_rpi_i32_f32" {
                    let h = self.s.m.f32(0.5f32.to_bits());
                    a = self.s.m.bin(c, Bin::Add, a, h);
                }
                if name != "v_cvt_i32_f32" && name != "v_cvt_u32_f32" {
                    a = self.unary(27, a);
                }
                // D3D's conversions saturate and give 0 for NaN, as the
                // hardware does (checked on the RX 6700 XT and WARP).
                let op = if name == "v_cvt_u32_f32" {
                    Cast::FpToUi
                } else {
                    Cast::FpToSi
                };
                let r = self.s.m.cast(c, op, a, self.s.t.i32);
                int(self, r)
            }
            "v_cvt_f32_i32" | "v_cvt_f32_u32" | "v_cvt_f32_ubyte0" | "v_cvt_f32_ubyte1"
            | "v_cvt_f32_ubyte2" | "v_cvt_f32_ubyte3" => {
                let mut a = s(self, 0)?;
                if let Some(n) = name.strip_prefix("v_cvt_f32_ubyte") {
                    let sh = self.s.m.i32(8 * u32::from(n.as_bytes()[0] - b'0'));
                    let ff = self.s.m.i32(0xff);
                    a = self.s.m.bin(c, Bin::LShr, a, sh);
                    a = self.s.m.bin(c, Bin::And, a, ff);
                }
                let op = if name == "v_cvt_f32_i32" {
                    Cast::SiToFp
                } else {
                    Cast::UiToFp
                };
                let r = self.s.m.cast(c, op, a, self.s.t.f32);
                float(self, r)
            }
            "v_fma_mix_f32" | "v_fma_mixlo_f16" | "v_fma_mixhi_f16" => {
                // Each source is f32, or with op_sel_hi the low (op_sel: high)
                // f16 half; neg_hi is the absolute value here.
                let mut xs = Vec::new();
                for i in 0..3 {
                    let op = v.src.get(i).ok_or("a missing operand")?;
                    let mut x = if v.opsel_hi >> i & 1 != 0 {
                        let x = self.f16_operand(inst, op)?;
                        let h = if v.opsel >> i & 1 != 0 {
                            let k16 = self.s.m.i32(16);
                            self.s.m.bin(c, Bin::LShr, x, k16)
                        } else {
                            x
                        };
                        self.s
                            .op(c, "legacyF16ToF32", Attr::ReadNone, self.s.t.f32, 131, &[h])
                    } else {
                        let x = self.operand(inst, op)?;
                        self.as_float(x)
                    };
                    if v.neg_hi >> i & 1 != 0 {
                        x = self.unary(6, x);
                    }
                    if v.neg >> i & 1 != 0 {
                        let z = self.s.m.f32(0x8000_0000);
                        x = self.s.m.bin(c, Bin::Sub, z, x);
                    }
                    xs.push(x);
                }
                let mut r = self.fmad(xs[0], xs[1], xs[2]);
                if name == "v_fma_mix_f32" {
                    return float(self, r);
                }
                if v.clamp {
                    r = self.unary(7, r);
                }
                // The f16 result goes to one half; the other half stays.
                let Some(Operand::Vector { reg, .. }) = v.dst else {
                    return Err("a mixed-precision result");
                };
                let old = match inst.read(Reg::V(reg)).map(|o| self.i32_of(o)) {
                    Some(Ok(o)) => o,
                    _ => self.s.m.i32(0),
                };
                let h = self
                    .s
                    .op(c, "legacyF32ToF16", Attr::ReadNone, self.s.t.i32, 130, &[r]);
                let (keep, h) = if name == "v_fma_mixhi_f16" {
                    let k16 = self.s.m.i32(16);
                    (0xFFFF, self.s.m.bin(c, Bin::Shl, h, k16))
                } else {
                    (0xFFFF_0000, h)
                };
                let keep = self.s.m.i32(keep);
                let old = self.s.m.bin(c, Bin::And, old, keep);
                let x = self.s.m.bin(c, Bin::Or, old, h);
                self.write(inst, Reg::V(reg), x, guard);
                Ok(())
            }
            "v_cvt_pkrtz_f16_f32" => {
                let (a, bb) = (f(self, 0)?, f(self, 1)?);
                let lo = self.f16_toward_zero(a);
                let hi = self.f16_toward_zero(bb);
                let sh = self.s.m.i32(16);
                let hi = self.s.m.bin(c, Bin::Shl, hi, sh);
                let r = self.s.m.bin(c, Bin::Or, lo, hi);
                int(self, r)
            }
            "v_ldexp_f32" => {
                // src0 * 2^src1 as three factors 2^e, each exponent in the
                // normal range, so the products are exact until the result
                // leaves it. Past |src1| = 375 every finite nonzero src0 has
                // over- or underflowed, so src1 is clamped there; then
                // e1 = floor(src1 * 21846 / 65536) (about a third) and the
                // rest in two halves.
                // shortcut: a result that turns denormal before the last
                // factor is rounded twice where denormals are kept.
                let (a, e) = (f(self, 0)?, s(self, 1)?);
                let (lo, hi) = (self.s.m.i32((-375i32).cast_unsigned()), self.s.m.i32(375));
                let below = self.s.m.cmp(c, pred::SLT, e, lo);
                let e = self.s.m.select(c, below, lo, e);
                let above = self.s.m.cmp(c, pred::SGT, e, hi);
                let e = self.s.m.select(c, above, hi, e);
                let (third, k16, one) = (self.s.m.i32(21846), self.s.m.i32(16), self.s.m.i32(1));
                let t = self.s.m.bin(c, Bin::Mul, e, third);
                let e1 = self.s.m.bin(c, Bin::AShr, t, k16);
                let rest = self.s.m.bin(c, Bin::Sub, e, e1);
                let e2 = self.s.m.bin(c, Bin::AShr, rest, one);
                let e3 = self.s.m.bin(c, Bin::Sub, rest, e2);
                let mut r = a;
                for x in [e1, e2, e3] {
                    let (bias, k23) = (self.s.m.i32(127), self.s.m.i32(23));
                    let x = self.s.m.bin(c, Bin::Add, x, bias);
                    let x = self.s.m.bin(c, Bin::Shl, x, k23);
                    let p = self.as_float(x);
                    r = self.s.m.bin(c, Bin::Mul, r, p);
                }
                float(self, r)
            }
            "v_cvt_f16_f32" if v.opsel == 0 => {
                // Rounded to nearest even (the MODE default) into the low half.
                let a = f(self, 0)?;
                let r = self
                    .s
                    .op(c, "legacyF32ToF16", Attr::ReadNone, self.s.t.i32, 130, &[a]);
                let Some(Operand::Vector { reg, .. }) = v.dst else {
                    return Err("a 16-bit result outside a VGPR");
                };
                let r = self.low_half(inst, reg, r);
                int(self, r)
            }
            "v_pack_b32_f16" => {
                // Two f16 sources (op_sel: their high halves; abs and neg on
                // the sign bit) into the low and high half.
                let mut halves = Vec::new();
                for k in 0..2 {
                    let op = v.src.get(k).ok_or("a missing operand")?;
                    let mut x = self.f16_operand(inst, op)?;
                    if v.opsel >> k & 1 != 0 {
                        let k16 = self.s.m.i32(16);
                        x = self.s.m.bin(c, Bin::LShr, x, k16);
                    }
                    let mask = if v.abs >> k & 1 != 0 { 0x7FFF } else { 0xFFFF };
                    let mask = self.s.m.i32(mask);
                    x = self.s.m.bin(c, Bin::And, x, mask);
                    if v.neg >> k & 1 != 0 {
                        let sign = self.s.m.i32(0x8000);
                        x = self.s.m.bin(c, Bin::Xor, x, sign);
                    }
                    halves.push(x);
                }
                let k16 = self.s.m.i32(16);
                let hi = self.s.m.bin(c, Bin::Shl, halves[1], k16);
                let r = self.s.m.bin(c, Bin::Or, halves[0], hi);
                int(self, r)
            }
            "v_cvt_f32_f16" if v.opsel == 0 => {
                let a = s(self, 0)?;
                let r = self
                    .s
                    .op(c, "legacyF16ToF32", Attr::ReadNone, self.s.t.f32, 131, &[a]);
                float(self, r)
            }
            "v_cndmask_b32" => {
                let cond = self.select_condition(inst, v.src.get(2))?;
                if v.abs != 0 || v.neg != 0 {
                    let (a, bb) = (f(self, 0)?, f(self, 1)?);
                    let r = self.s.m.select(c, cond, bb, a);
                    return float(self, r);
                }
                let (a, bb) = (s(self, 0)?, s(self, 1)?);
                let r = self.s.m.select(c, cond, bb, a);
                int(self, r)
            }
            "v_add_co_u32" | "v_sub_co_u32" | "v_subrev_co_u32" | "v_add_co_ci_u32"
            | "v_sub_co_ci_u32" | "v_subrev_co_ci_u32" => {
                // The carry (borrow) out is a lane mask: this lane's bit.
                let (mut a, mut bb) = (s(self, 0)?, s(self, 1)?);
                if name.contains("subrev") {
                    std::mem::swap(&mut a, &mut bb);
                }
                let cin = if name.ends_with("_ci_u32") {
                    let m = match v.src.get(2) {
                        Some(&Operand::Scalar { code, .. }) => inst.read(Reg::S(code)),
                        _ => None,
                    };
                    let m = m
                        .and_then(|m| self.x.st.masks.get(&m).copied())
                        .ok_or("a carry-in from an untracked mask")?;
                    Some(self.boolean(m, c)?)
                } else {
                    None
                };
                let (r, carry) = self.add_sub_carry(c, name.starts_with("v_add"), a, bb, cin);
                int(self, r)?;
                if let Some(Operand::Scalar { code, .. }) = v.carry
                    && let Some(w) = inst.write(Reg::S(code))
                {
                    self.lane_bits.insert(w, carry);
                }
                for &(_, w) in &inst.writes {
                    self.vals
                        .entry(w)
                        .or_insert(Val::Missing("a lane mask read as data"));
                }
                Ok(())
            }
            // Each lane reads a lane of its row of 16, or of the other row of
            // its half of 32, picked by its nibble of {src2, src1}.
            "v_permlane16_b32" | "v_permlanex16_b32" => {
                let (a, s1, s2) = (s(self, 0)?, s(self, 1)?, s(self, 2)?);
                self.cross_lane(false)?;
                if self.split || self.console_cf {
                    return Err("a lane permutation in a program split in halves");
                }
                let l = self.lane_index();
                let k = |l: &mut Self, x: u32| l.s.m.i32(x);
                let (k7, k8, k2, k15, top, zero) =
                    (k(self, 7), k(self, 8), k(self, 2), k(self, 15), k(self, !15), k(self, 0));
                let n = self.s.m.bin(c, Bin::And, l, k7);
                let n = self.s.m.bin(c, Bin::Shl, n, k2);
                let half = self.s.m.bin(c, Bin::And, l, k8);
                let high = self.s.m.cmp(c, pred::NE, half, zero);
                let word = self.s.m.select(c, high, s2, s1);
                let pick = self.s.m.bin(c, Bin::LShr, word, n);
                let pick = self.s.m.bin(c, Bin::And, pick, k15);
                let mut row = self.s.m.bin(c, Bin::And, l, top);
                if name == "v_permlanex16_b32" {
                    let k16 = self.s.m.i32(16);
                    row = self.s.m.bin(c, Bin::Xor, row, k16);
                }
                let lane = self.s.m.bin(c, Bin::Or, row, pick);
                let r = self.read_lane(a, lane);
                int(self, r)
            }
            "v_readlane_b32"
                if matches!(v.src.first(), Some(&Operand::Vector { reg, .. })
                    if inst.read(Reg::V(reg)).is_some_and(|r| self.one_lane.contains(&r))) =>
            {
                // Made from that lane's value already.
                let r = s(self, 0)?;
                int(self, r)
            }
            "v_readlane_b32" => {
                let (a, lane) = (s(self, 0)?, s(self, 1)?);
                // A computed lane in a pixel or vertex program is a host lane
                // (from a mask or `v_mbcnt`); a constant one is the console's.
                if self.mask_data == MaskData::Host && !matches!(lane, V::Const(_)) {
                    self.s.flags |= flags::WAVE_OPS;
                } else {
                    self.cross_lane(false)?;
                }
                let r = if self.console_cf {
                    self.console_read_lane(a, lane)
                } else {
                    self.read_lane(a, lane)
                };
                int(self, r)
            }
            "v_writelane_b32" => {
                // The lane given takes the scalar; the others keep their value.
                let (a, lane) = (s(self, 0)?, s(self, 1)?);
                let Some(Operand::Vector { reg, .. }) = v.dst else {
                    return Err("a lane write");
                };
                let old = match inst.read(Reg::V(reg)) {
                    Some(o) => self.i32_of(o)?,
                    None => self.s.m.i32(0),
                };
                self.cross_lane(false)?;
                let me = if self.console_cf {
                    self.console_lane().0
                } else {
                    self.lane_index()
                };
                let is = self.s.m.cmp(c, pred::EQ, me, lane);
                let r = self.s.m.select(c, is, a, old);
                int(self, r)
            }
            "v_readfirstlane_b32"
                if inst
                    .writes
                    .first()
                    .is_some_and(|w| self.x.res.waterfall.reads.contains(&w.1)) =>
            {
                // A waterfall loop's read of a descriptor dword: the lane's own.
                let a = s(self, 0)?;
                int(self, a)
            }
            "v_readfirstlane_b32" if self.console_cf => {
                // The console's first lane in EXEC (lane 0 when none).
                let a = s(self, 0)?;
                let word = |l: &mut Self, r: u8| -> Result<V, Why> {
                    match l.get(inst.read(Reg::S(r)).ok_or("EXEC")?)? {
                        Val::I32(x) => Ok(x),
                        _ => Err(LANE_MASK),
                    }
                };
                let lo = word(self, EXEC)?;
                let hi = if self.x.native.wave64 {
                    word(self, EXEC + 1)?
                } else {
                    self.s.m.i32(0)
                };
                let i32t = self.s.t.i32;
                let flo = self
                    .s
                    .op(c, "unaryBits.i32", Attr::ReadNone, i32t, 32, &[lo]);
                let fhi = self
                    .s
                    .op(c, "unaryBits.i32", Attr::ReadNone, i32t, 32, &[hi]);
                let (z, k32) = (self.s.m.i32(0), self.s.m.i32(32));
                let fhi = self.s.m.bin(c, Bin::Add, fhi, k32);
                let none_hi = self.s.m.cmp(c, pred::EQ, hi, z);
                let fhi = self.s.m.select(c, none_hi, z, fhi);
                let some_lo = self.s.m.cmp(c, pred::NE, lo, z);
                let first = self.s.m.select(c, some_lo, flo, fhi);
                let r = self.console_read_lane(a, first);
                int(self, r)
            }
            "v_readfirstlane_b32" => {
                let a = s(self, 0)?;
                self.s.flags |= flags::WAVE_OPS;
                let r = self.s.op(
                    c,
                    "waveReadLaneFirst.i32",
                    Attr::NoUnwind,
                    self.s.t.i32,
                    118,
                    &[a],
                );
                let r = self.helper_reads_own(a, r);
                int(self, r)
            }
            "v_add_f64" | "v_mul_f64" | "v_fma_f64" => {
                if v.omod != 0 || v.clamp {
                    return Err("f64 output modifiers");
                }
                let a = self.src_f64(inst, 0)?;
                let b = self.src_f64(inst, 1)?;
                let r = match name {
                    "v_add_f64" => self.s.m.bin(c, Bin::Add, a, b),
                    "v_mul_f64" => self.s.m.bin(c, Bin::Mul, a, b),
                    _ => {
                        let d = self.src_f64(inst, 2)?;
                        self.s.flags |= flags::DOUBLE_EXT;
                        let f64t = self.s.m.double();
                        self.s
                            .op(c, "fma.f64", Attr::ReadNone, f64t, 47, &[a, b, d])
                    }
                };
                self.write_f64(inst, r, guard)
            }
            "v_cvt_f32_f64" => {
                let a = self.src_f64(inst, 0)?;
                let r = self.s.m.cast(c, Cast::FpTrunc, a, self.s.t.f32);
                float(self, r)
            }
            "v_cvt_i32_f64" | "v_cvt_u32_f64" => {
                if v.omod != 0 || v.clamp {
                    return Err("modifiers on integer conversion");
                }
                let a = self.src_f64(inst, 0)?;
                let cast = if name == "v_cvt_i32_f64" {
                    Cast::FpToSi
                } else {
                    Cast::FpToUi
                };
                let r = self.s.m.cast(c, cast, a, self.s.t.i32);
                int(self, r)
            }
            "v_cvt_f64_f32" | "v_cvt_f64_i32" | "v_cvt_f64_u32" => {
                if v.omod != 0 || v.clamp {
                    return Err("f64 output modifiers");
                }
                self.s.flags |= flags::DOUBLES;
                let f64t = self.s.m.double();
                let r = match name {
                    "v_cvt_f64_f32" => {
                        let a = f(self, 0)?;
                        self.s.m.cast(c, Cast::FpExt, a, f64t)
                    }
                    "v_cvt_f64_i32" => {
                        let a = s(self, 0)?;
                        self.s.m.cast(c, Cast::SiToFp, a, f64t)
                    }
                    _ => {
                        let a = s(self, 0)?;
                        self.s.m.cast(c, Cast::UiToFp, a, f64t)
                    }
                };
                self.write_f64(inst, r, guard)
            }
            n if n.contains("_f64") => Err("floating-point arithmetic in f64 is not implemented by the DXIL emitter"),
            _ => {
                let r = self.integer(inst, name)?;
                int(self, r)
            }
        }
    }

    /// A 64-bit floating-point source, constructed from its bit pattern via `dx.op.makeDouble`.
    fn src_f64(&mut self, inst: &Inst, k: usize) -> Result<V, Why> {
        let bits = self.fp_bits(inst, k, 64)?;
        let (lo, hi) = self.split64(bits);
        self.s.flags |= flags::DOUBLES;
        let f64t = self.s.m.double();
        Ok(self
            .s
            .op(self.cur, "makeDouble.f64", Attr::ReadNone, f64t, 101, &[lo, hi]))
    }

    /// Writes a 64-bit floating-point result into a VGPR pair via `dx.op.splitDouble`.
    fn write_f64(&mut self, inst: &Inst, val: V, guard: Option<V>) -> Result<(), Why> {
        let Fields::Valu(v) = &inst.fields else {
            unreachable!()
        };
        let Some(Operand::Vector { reg, .. }) = v.dst else {
            return Err("an f64 destination");
        };
        self.s.flags |= flags::DOUBLES;
        let i32t = self.s.t.i32;
        let pair = self.s.m.named_struct("dx.types.splitdouble", vec![i32t, i32t]);
        let split = self
            .s
            .op(self.cur, "splitDouble.f64", Attr::ReadNone, pair, 102, &[val]);
        let lo = self.s.m.extract(self.cur, split, 0);
        let hi = self.s.m.extract(self.cur, split, 1);
        self.write(inst, Reg::V(reg), lo, guard);
        self.write(inst, Reg::V(reg + 1), hi, guard);
        Ok(())
    }

    /// A 64-bit source (a register pair; inline constants sign-extend).
    fn src64(&mut self, inst: &Inst, k: usize) -> Result<V, Why> {
        let Fields::Valu(v) = &inst.fields else {
            unreachable!()
        };
        if v.abs >> k & 1 != 0 || v.neg >> k & 1 != 0 || v.sdwa.is_some() || v.dpp.is_some() {
            return Err("64-bit operand modifiers");
        }
        self.raw_src64(inst, k, false)
    }

    fn raw_src64(&mut self, inst: &Inst, k: usize, float: bool) -> Result<V, Why> {
        let Fields::Valu(v) = &inst.fields else { unreachable!() };
        if v.sdwa.is_some() || v.dpp.is_some() {
            return Err("64-bit SDWA or DPP operands");
        }
        let c = self.cur;
        let i64t = self.s.m.int(64);
        self.s.flags |= flags::INT64;
        let op = *v.src.get(k).ok_or("a missing operand")?;
        let pair = |l: &mut Self, r: Reg, r1: Reg| -> Result<V, Why> {
            let lo = l.i32_of(inst.read(r).ok_or("an unread operand")?)?;
            let hi = l.i32_of(inst.read(r1).ok_or("an unread operand")?)?;
            Ok(l.join64(lo, hi))
        };
        match op {
            Operand::Vector { reg, .. } => pair(self, Reg::V(reg), Reg::V(reg + 1)),
            Operand::Scalar { code, .. } if code != NULL => {
                pair(self, Reg::S(code), Reg::S(code + 1))
            }
            Operand::Const(k @ 240..=248) if float => {
                let f = if k == 248 {
                    0.5 * std::f64::consts::FRAC_1_PI
                } else {
                    let b = crate::resource::inline(k).ok_or("a special operand")?;
                    f64::from(f32::from_bits(b))
                };
                Ok(self.s.m.int_const(i64t, f.to_bits() as i64))
            }
            Operand::Literal(x) if float => Ok(self.s.m.int_const(i64t, i64::from(x) << 32)),
            Operand::Literal(x) if inst.name() == "v_mad_i64_i32" => {
                Ok(self.s.m.int_const(i64t, i64::from(x as i32)))
            }
            Operand::Const(k @ 128..=208) => {
                let x = crate::resource::inline(k).ok_or("a special operand")?;
                Ok(self.s.m.int_const(i64t, i64::from(x as i32)))
            }
            _ => {
                // Literals and the null register: zero-extended.
                let x = self.operand(inst, &op)?;
                Ok(self.s.m.cast(c, Cast::ZExt, x, i64t))
            }
        }
    }

    /// IEEE source bits, with sign modifiers applied as bit operations so
    /// that a signalling NaN, a denormal or a negative zero is preserved.
    fn fp_bits(&mut self, inst: &Inst, k: usize, bits: u32) -> Result<V, Why> {
        let Fields::Valu(v) = &inst.fields else { unreachable!() };
        let mut x = match bits {
            16 => self.half_src(inst, k)?,
            32 => match (self.src0, k) {
                (Some(x), 0) => x,
                _ => {
                    let op = v.src.get(k).ok_or("a missing operand")?;
                    let x = self.operand(inst, op)?;
                    match Self::sdwa_src(inst, k) {
                        Some((sel, sext)) => self.select_bits(x, sel, sext),
                        None => x,
                    }
                }
            },
            _ => self.raw_src64(inst, k, true)?,
        };
        let ty = self.s.m.int(bits.max(32));
        let sign = 1u64 << (bits - 1);
        if v.abs >> k & 1 != 0 {
            let mask = self.s.m.int_const(ty, (sign - 1) as i64);
            x = self.s.m.bin(self.cur, Bin::And, x, mask);
        }
        if v.neg >> k & 1 != 0 {
            let sign = self.s.m.int_const(ty, sign as i64);
            x = self.s.m.bin(self.cur, Bin::Xor, x, sign);
        }
        Ok(x)
    }

    /// Decompose the IEEE encoding directly. AMD's RDNA 2 ISA specifies
    /// zero exponent for infinities and NaNs and passes their mantissa
    /// through. LLVM's AMDGPU frexp fold also normalizes subnormal inputs.
    fn frexp(&mut self, inst: &Inst, guard: Option<V>) -> Result<(), Why> {
        let Fields::Valu(v) = &inst.fields else { unreachable!() };
        if v.clamp || v.omod != 0 {
            return Err("floating output modifiers on frexp");
        }
        let name = inst.name();
        let (bits, fraction, bias) = if name.ends_with("f64") { (64, 52, 1023) }
            else if name.ends_with("f32") { (32, 23, 127) } else { (16, 10, 15) };
        let c = self.cur;
        let ty = self.s.m.int(bits.max(32));
        let x = self.fp_bits(inst, 0, bits)?;
        let k = |l: &mut Self, n| l.s.m.int_const(ty, n);
        let (z, frac, shift, emax, abs) = (k(self, 0), k(self, (1 << fraction) - 1),
            k(self, fraction), k(self, 2 * bias + 1), k(self, ((1u64 << (bits - 1)) - 1) as i64));
        let mant = self.s.m.bin(c, Bin::And, x, frac);
        let exp = self.s.m.bin(c, Bin::LShr, x, shift);
        let exp = self.s.m.bin(c, Bin::And, exp, emax);
        let subnormal = self.s.m.cmp(c, pred::EQ, exp, z);
        let abs = self.s.m.bin(c, Bin::And, x, abs);
        let nonzero = self.s.m.cmp(c, pred::NE, abs, z);
        let finite = self.s.m.cmp(c, pred::NE, exp, emax);
        let compute = self.s.m.bin(c, Bin::And, nonzero, finite);
        // Locate the highest set fraction bit with DXIL's i32 first-bit-high.
        let fb = |l: &mut Self, a| l.s.op(c, "unaryBits.i32", Attr::ReadNone, l.s.t.i32, 33, &[a]);
        let leading = if bits == 64 {
            let (lo, hi) = self.split64(mant);
            let (a, b) = (fb(self, lo), fb(self, hi));
            let (size, zero) = (self.s.m.i32(32), self.s.m.i32(0));
            let a = self.s.m.bin(c, Bin::Add, a, size);
            let low = self.s.m.cmp(c, pred::EQ, hi, zero);
            self.s.m.select(c, low, a, b)
        } else {
            fb(self, mant)
        };
        // A zero mantissa can produce -1. Mask the normalization shift
        // before emitting even the unselected path, so it is always defined.
        let offset = self.s.m.i32(bits.max(32) - 1 - fraction as u32);
        let left = self.s.m.bin(c, Bin::Sub, leading, offset);
        let limit = self.s.m.i32(bits.max(32) - 1);
        let left = self.s.m.bin(c, Bin::And, left, limit);
        let left = if bits == 64 { self.extend64(left, false) } else { left };
        let Some(Operand::Vector { reg, .. }) = v.dst else { return Err("a frexp result") };
        if name.starts_with("v_frexp_exp_") {
            let one = k(self, 1);
            let e_sub = self.s.m.bin(c, Bin::Sub, one, left);
            let e = self.s.m.select(c, subnormal, e_sub, exp);
            let bias = k(self, bias - 1);
            let e = self.s.m.bin(c, Bin::Sub, e, bias);
            let e = self.s.m.select(c, compute, e, z);
            if bits == 16 {
                self.write_half(inst, e, guard)
            } else {
                let e = if bits == 64 { self.s.m.cast(c, Cast::Trunc, e, self.s.t.i32) } else { e };
                self.write_i(inst, e, guard)
            }
        } else {
            let norm = self.s.m.bin(c, Bin::Shl, mant, left);
            let mant = self.s.m.select(c, subnormal, norm, mant);
            let mant = self.s.m.bin(c, Bin::And, mant, frac);
            let sign = k(self, (1u64 << (bits - 1)) as i64);
            let sign = self.s.m.bin(c, Bin::And, x, sign);
            let half = k(self, (bias - 1) << fraction);
            let value = self.s.m.bin(c, Bin::Or, sign, half);
            let value = self.s.m.bin(c, Bin::Or, value, mant);
            let value = self.s.m.select(c, compute, value, x);
            match bits {
                16 => self.write_half(inst, value, guard),
                32 => self.write_i(inst, value, guard),
                _ => {
                    let (lo, hi) = self.split64(value);
                    self.write(inst, Reg::V(reg), lo, guard);
                    self.write(inst, Reg::V(reg + 1), hi, guard);
                    Ok(())
                }
            }
        }
    }

    fn join64(&mut self, lo: V, hi: V) -> V {
        let c = self.cur;
        let ty = self.s.m.int(64);
        let lo = self.s.m.cast(c, Cast::ZExt, lo, ty);
        let hi = self.s.m.cast(c, Cast::ZExt, hi, ty);
        let k32 = self.s.m.int_const(ty, 32);
        let hi = self.s.m.bin(c, Bin::Shl, hi, k32);
        self.s.m.bin(c, Bin::Or, lo, hi)
    }

    fn extend64(&mut self, x: V, signed: bool) -> V {
        self.s.flags |= flags::INT64;
        let ty = self.s.m.int(64);
        self.s.m.cast(self.cur, if signed { Cast::SExt } else { Cast::ZExt }, x, ty)
    }

    /// The low and high dwords of an i64.
    fn split64(&mut self, x: V) -> (V, V) {
        let c = self.cur;
        let i64t = self.s.m.int(64);
        let k32 = self.s.m.int_const(i64t, 32);
        let lo = self.s.m.cast(c, Cast::Trunc, x, self.s.t.i32);
        let hi = self.s.m.bin(c, Bin::LShr, x, k32);
        let hi = self.s.m.cast(c, Cast::Trunc, hi, self.s.t.i32);
        (lo, hi)
    }

    /// Source `k` of a 16-bit operation in the low 16 bits (the rest 0):
    /// op_sel (or an SDWA word select) picks the high half; inline float
    /// constants are f16 values here.
    fn half_src(&mut self, inst: &Inst, k: usize) -> Result<V, Why> {
        let Fields::Valu(v) = &inst.fields else {
            unreachable!()
        };
        let op = *v.src.get(k).ok_or("a missing operand")?;
        let mut high = v.opsel >> k & 1 != 0;
        if let Some(sd) = v.sdwa {
            match sd.src_sel.get(k).copied().unwrap_or(6) {
                4 => {}
                5 => high = true,
                6 => {}
                _ => return Err("a byte select on a 16-bit operation"),
            }
        }
        let x = match (op, self.src0) {
            (_, Some(x)) if k == 0 => x,
            (Operand::Const(c @ 240..=248), _) => self.s.m.i32(inline_f16(c)),
            _ => self.operand(inst, &op)?,
        };
        let c = self.cur;
        let x = if high {
            let k16 = self.s.m.i32(16);
            self.s.m.bin(c, Bin::LShr, x, k16)
        } else {
            x
        };
        let m = self.s.m.i32(0xFFFF);
        Ok(self.s.m.bin(c, Bin::And, x, m))
    }

    /// An f16 source as an f32 (exact), with its abs/neg modifiers.
    fn half_fsrc(&mut self, inst: &Inst, k: usize) -> Result<V, Why> {
        let Fields::Valu(v) = &inst.fields else {
            unreachable!()
        };
        let h = self.half_src(inst, k)?;
        let f32t = self.s.t.f32;
        let mut f = self
            .s
            .op(self.cur, "legacyF16ToF32", Attr::ReadNone, f32t, 131, &[h]);
        if v.abs >> k & 1 != 0 {
            f = self.unary(6, f);
        }
        if v.neg >> k & 1 != 0 {
            let z = self.s.m.f32(0x8000_0000);
            f = self.s.m.bin(self.cur, Bin::Sub, z, f);
        }
        Ok(f)
    }

    /// Writes a 16-bit result (low bits of `x`) into the destination's low
    /// half, or its high half with op_sel bit 3 (or an SDWA WORD_1 select);
    /// the other half is kept (GFX10).
    fn write_half(&mut self, inst: &Inst, x: V, guard: Option<V>) -> Result<(), Why> {
        let Fields::Valu(v) = &inst.fields else {
            unreachable!()
        };
        let Some(Operand::Vector { reg, .. }) = v.dst else {
            return Err("a 16-bit result outside a VGPR");
        };
        let mut high = v.opsel >> 3 & 1 != 0;
        // SDWA: the word written, the other one kept (preserve) or zero
        // (pad; a sign extension above the high word adds nothing).
        let mut keep = true;
        if let Some(sd) = v.sdwa {
            match (sd.dst_sel, sd.dst_unused) {
                (4, 2) => {}
                (5, 2) => high = true,
                (4 | 6, _) => keep = false,
                (5, _) => (high, keep) = (true, false),
                _ => return Err("an SDWA destination of a 16-bit operation"),
            }
        }
        let c = self.cur;
        let old = match inst.read(Reg::V(reg)).map(|o| self.i32_of(o)) {
            Some(Ok(o)) if keep => o,
            _ => self.s.m.i32(0),
        };
        let (k16, low) = (self.s.m.i32(16), self.s.m.i32(0xFFFF));
        let x = self.s.m.bin(c, Bin::And, x, low);
        let r = if high {
            let o = self.s.m.bin(c, Bin::And, old, low);
            let x = self.s.m.bin(c, Bin::Shl, x, k16);
            self.s.m.bin(c, Bin::Or, o, x)
        } else {
            let keep = self.s.m.i32(0xFFFF_0000);
            let o = self.s.m.bin(c, Bin::And, old, keep);
            self.s.m.bin(c, Bin::Or, o, x)
        };
        self.write_placed(inst, Reg::V(reg), r, guard);
        Ok(())
    }

    /// 16-bit operations, computed in f32 or i32: f16 sums, differences and
    /// products are exact there and round once to f16 (f32 has the 2 x 11 +
    /// 2 bits that make double rounding harmless); `None` if `name` is none.
    fn valu16(&mut self, inst: &Inst, name: &str, guard: Option<V>) -> Option<Result<(), Why>> {
        const FLOAT2: &[&str] = &[
            "v_add_f16",
            "v_sub_f16",
            "v_subrev_f16",
            "v_mul_f16",
            "v_max_f16",
            "v_min_f16",
        ];
        const FLOAT3: &[&str] = &[
            "v_fma_f16",
            "v_mad_f16",
            "v_fmac_f16",
            "v_mac_f16",
            "v_max3_f16",
            "v_min3_f16",
            "v_med3_f16",
        ];
        const FLOAT1: &[&str] = &[
            "v_rcp_f16",
            "v_sqrt_f16",
            "v_rsq_f16",
            "v_log_f16",
            "v_exp_f16",
            "v_fract_f16",
            "v_trunc_f16",
            "v_floor_f16",
            "v_ceil_f16",
            "v_rndne_f16",
            "v_sin_f16",
            "v_cos_f16",
            "v_cvt_f16_u16",
            "v_cvt_f16_i16",
        ];
        const INT: &[&str] = &[
            "v_add_nc_u16",
            "v_mad_i16",
            "v_mad_u16",
            "v_min3_i16",
            "v_min3_u16",
            "v_max3_i16",
            "v_max3_u16",
            "v_med3_i16",
            "v_med3_u16",
            "v_sub_nc_u16",
            "v_add_nc_i16",
            "v_sub_nc_i16",
            "v_subrev_nc_u16",
            "v_mul_lo_u16",
            "v_lshlrev_b16",
            "v_lshrrev_b16",
            "v_ashrrev_i16",
            "v_max_u16",
            "v_max_i16",
            "v_min_u16",
            "v_min_i16",
            "v_cvt_u16_f16",
            "v_cvt_i16_f16",
            "v_cvt_norm_i16_f16",
            "v_cvt_norm_u16_f16",
        ];
        let all = [FLOAT1, FLOAT2, FLOAT3, INT];
        if !all.iter().any(|l| l.contains(&name))
            && !matches!(name, "v_cvt_f16_f32" | "v_cvt_f32_f16")
        {
            return None;
        }
        Some(self.valu16_op(inst, name, guard, FLOAT1, FLOAT2, FLOAT3))
    }

    #[allow(clippy::too_many_arguments)]
    fn valu16_op(
        &mut self,
        inst: &Inst,
        name: &str,
        guard: Option<V>,
        float1: &[&str],
        float2: &[&str],
        float3: &[&str],
    ) -> Result<(), Why> {
        let Fields::Valu(v) = &inst.fields else {
            unreachable!()
        };
        let c = self.cur;
        let i32t = self.s.t.i32;
        let f32t = self.s.t.f32;
        if name == "v_cvt_f32_f16" {
            let f = self.half_fsrc(inst, 0)?;
            return self.write_f(inst, f, guard);
        }
        let float_result = |l: &mut Self, mut f: V| -> Result<(), Why> {
            if v.omod != 0 {
                return Err("an output modifier on a 16-bit operation");
            }
            if v.clamp {
                f = l.unary(7, f);
            }
            let h =
                l.s.op(l.cur, "legacyF32ToF16", Attr::ReadNone, i32t, 130, &[f]);
            l.write_half(inst, h, guard)
        };
        if name == "v_cvt_f16_f32" {
            let f = self.fsrc(inst, 0)?;
            return float_result(self, f);
        }
        if float2.contains(&name) {
            let (a, b) = (self.half_fsrc(inst, 0)?, self.half_fsrc(inst, 1)?);
            let r = match name {
                "v_add_f16" => self.s.m.bin(c, Bin::Add, a, b),
                "v_sub_f16" => self.s.m.bin(c, Bin::Sub, a, b),
                "v_subrev_f16" => self.s.m.bin(c, Bin::Sub, b, a),
                "v_mul_f16" => self.s.m.bin(c, Bin::Mul, a, b),
                "v_max_f16" => self.max_min(true, a, b),
                _ => self.max_min(false, a, b),
            };
            return float_result(self, r);
        }
        if float3.contains(&name) {
            let (a, b) = (self.half_fsrc(inst, 0)?, self.half_fsrc(inst, 1)?);
            let acc = if matches!(name, "v_fmac_f16" | "v_mac_f16") {
                // The accumulator is the destination's half that is written.
                let Some(Operand::Vector { reg, .. }) = v.dst else {
                    return Err("an accumulator");
                };
                let x = self.i32_of(inst.read(Reg::V(reg)).ok_or("an accumulator")?)?;
                let x = if v.opsel >> 3 & 1 != 0 {
                    let k16 = self.s.m.i32(16);
                    self.s.m.bin(c, Bin::LShr, x, k16)
                } else {
                    x
                };
                let m = self.s.m.i32(0xFFFF);
                let x = self.s.m.bin(c, Bin::And, x, m);
                self.s
                    .op(c, "legacyF16ToF32", Attr::ReadNone, f32t, 131, &[x])
            } else {
                self.half_fsrc(inst, 2)?
            };
            let r = match name {
                // shortcut: fused f16 multiply-adds as FMad in f32 (the
                // product is exact; the sum rounds twice).
                "v_fma_f16" | "v_fmac_f16" | "v_mad_f16" | "v_mac_f16" => self.fmad(a, b, acc),
                "v_max3_f16" => {
                    let t = self.max_min(true, a, b);
                    self.max_min(true, t, acc)
                }
                "v_min3_f16" => {
                    let t = self.max_min(false, a, b);
                    self.max_min(false, t, acc)
                }
                _ => self.med3(a, b, acc),
            };
            return float_result(self, r);
        }
        if float1.contains(&name) {
            let r = match name {
                "v_cvt_f16_u16" | "v_cvt_f16_i16" => {
                    let mut x = self.half_src(inst, 0)?;
                    let op = if name == "v_cvt_f16_i16" {
                        let k16 = self.s.m.i32(16);
                        let t = self.s.m.bin(c, Bin::Shl, x, k16);
                        x = self.s.m.bin(c, Bin::AShr, t, k16);
                        Cast::SiToFp
                    } else {
                        Cast::UiToFp
                    };
                    self.s.m.cast(c, op, x, f32t)
                }
                "v_rcp_f16" => {
                    let a = self.half_fsrc(inst, 0)?;
                    let one = self.s.m.f32(1.0f32.to_bits());
                    self.s.m.bin(c, Bin::SDiv, one, a)
                }
                _ => {
                    let a = self.half_fsrc(inst, 0)?;
                    let op = match name {
                        "v_sqrt_f16" => 24,
                        "v_rsq_f16" => 25,
                        "v_log_f16" => 23,
                        "v_exp_f16" => 21,
                        "v_fract_f16" => 22,
                        "v_trunc_f16" => 29,
                        "v_floor_f16" => 27,
                        "v_ceil_f16" => 28,
                        "v_rndne_f16" => 26,
                        "v_sin_f16" => 13,
                        _ => 12,
                    };
                    match op {
                        12 | 13 => self.trig_turns(a, op == 12),
                        // Any f16 in (-2^-11, 0) has fract >= 1 - 2^-12; clamp to 0x3bff (1 - 2^-11).
                        22 => self.fract(a, 1.0 - 1.0 / 4096.0, 1.0 - 1.0 / 2048.0),
                        _ => self.unary(op, a),
                    }
                }
            };
            return float_result(self, r);
        }
        if matches!(name, "v_cvt_norm_i16_f16" | "v_cvt_norm_u16_f16") {
            if v.clamp || v.omod != 0 {
                return Err("modifiers on a 16-bit integer operation");
            }
            let r = self.cvt_norm16(inst, name == "v_cvt_norm_i16_f16")?;
            return self.write_half(inst, r, guard);
        }
        // 16-bit integers: computed on the low halves, the low 16 bits kept.
        let saturates = matches!(name, "v_mad_i16" | "v_mad_u16");
        if v.clamp && !saturates || v.omod != 0 || v.abs != 0 || v.neg != 0 {
            return Err("modifiers on a 16-bit integer operation");
        }
        let r = match name {
            "v_cvt_u16_f16" | "v_cvt_i16_f16" => {
                // Toward zero, saturating at the 16-bit range (infinities
                // included); NaN gives 0. Clamped in f32 first, where every
                // f16 is exact, so the conversion never leaves i32's range.
                let a = self.half_fsrc(inst, 0)?;
                let (lo, hi) = if name == "v_cvt_u16_f16" {
                    (0.0f32, 65535.0f32)
                } else {
                    (-32768.0, 32767.0)
                };
                let (lo, hi) = (self.s.m.f32(lo.to_bits()), self.s.m.f32(hi.to_bits()));
                let t = self.binary(35, a, lo);
                let t = self.binary(36, t, hi);
                let r = self.s.m.cast(c, Cast::FpToSi, t, i32t);
                let nan = self.s.m.cmp(c, pred::UNO, a, a);
                let z = self.s.m.i32(0);
                self.s.m.select(c, nan, z, r)
            }
            _ => {
                let signed = name.ends_with("_i16");
                let read = |l: &mut Self, i| -> Result<V, Why> {
                    let x = l.half_src(inst, i)?;
                    Ok(if signed { l.sign_extend(x, 16) } else { x })
                };
                let (a, b) = (read(self, 0)?, read(self, 1)?);
                let k15 = self.s.m.i32(15);
                match name {
                    "v_add_nc_u16" | "v_add_nc_i16" => self.s.m.bin(c, Bin::Add, a, b),
                    "v_sub_nc_u16" | "v_sub_nc_i16" => self.s.m.bin(c, Bin::Sub, a, b),
                    "v_subrev_nc_u16" => self.s.m.bin(c, Bin::Sub, b, a),
                    "v_mul_lo_u16" => self.s.m.bin(c, Bin::Mul, a, b),
                    "v_mad_i16" | "v_mad_u16" => {
                        let d = read(self, 2)?;
                        let m = self.s.m.bin(c, Bin::Mul, a, b);
                        self.s.m.bin(c, Bin::Add, m, d)
                    }
                    "v_min3_i16" | "v_min3_u16" | "v_max3_i16" | "v_max3_u16"
                    | "v_med3_i16" | "v_med3_u16" => {
                        let d = read(self, 2)?;
                        self.int3(&name[2..6], signed, a, b, d)
                    }
                    "v_lshlrev_b16" | "v_lshrrev_b16" | "v_ashrrev_i16" => {
                        // src0 is the shift (4 bits), src1 the value.
                        let sh = self.s.m.bin(c, Bin::And, a, k15);
                        let op = match name {
                            "v_lshlrev_b16" => Bin::Shl,
                            "v_lshrrev_b16" => Bin::LShr,
                            _ => Bin::AShr,
                        };
                        self.s.m.bin(c, op, b, sh)
                    }
                    _ => {
                        let p = match name {
                            "v_max_u16" => pred::IUGT,
                            "v_min_u16" => pred::IULT,
                            "v_max_i16" => pred::SGT,
                            _ => pred::SLT,
                        };
                        let t = self.s.m.cmp(c, p, a, b);
                        self.s.m.select(c, t, a, b)
                    }
                }
            }
        };
        let r = if v.clamp {
            let signed = name.ends_with("_i16");
            let (lo, hi) = if signed { (-32768, 32767) } else { (0, 65535) };
            self.clamp_int(r, 32, lo, hi, signed)
        } else {
            r
        };
        self.write_half(inst, r, guard)
    }

    /// `v_cvt_norm_i16_f16` / `v_cvt_norm_u16_f16`: the f16 source (with its
    /// abs/neg modifiers) clamped to [-1, 1] or [0, 1], times 32767 or 65535,
    /// rounded to nearest even once from the exact product; NaN gives 0.
    /// Worked on the f16's bits in integers, because in f32 the product
    /// (up to 26 significant bits) would round before the rounding to an
    /// integer.
    fn cvt_norm16(&mut self, inst: &Inst, signed: bool) -> Result<V, Why> {
        let Fields::Valu(v) = &inst.fields else {
            unreachable!()
        };
        let c = self.cur;
        let k = |l: &mut Self, x: u32| l.s.m.i32(x);
        let mut h = self.half_src(inst, 0)?;
        if v.abs & 1 != 0 {
            let m = k(self, 0x7FFF);
            h = self.s.m.bin(c, Bin::And, h, m);
        }
        if v.neg & 1 != 0 {
            let m = k(self, 0x8000);
            h = self.s.m.bin(c, Bin::Xor, h, m);
        }
        let scale = if signed { 0x7FFF } else { 0xFFFF };
        let (k0, k1, k10) = (k(self, 0), k(self, 1), k(self, 10));
        let mag_mask = k(self, 0x7FFF);
        let mag = self.s.m.bin(c, Bin::And, h, mag_mask);
        let k15 = k(self, 15);
        let neg = self.s.m.bin(c, Bin::LShr, h, k15);
        let neg = self.s.m.cmp(c, pred::NE, neg, k0);
        // |x| < 1: x = m * 2^-s with m < 2^11 and 11 <= s <= 24.
        let exp = self.s.m.bin(c, Bin::LShr, mag, k10);
        let man_mask = k(self, 0x3FF);
        let man = self.s.m.bin(c, Bin::And, mag, man_mask);
        let implicit = k(self, 0x400);
        let normal = self.s.m.bin(c, Bin::Or, man, implicit);
        let sub = self.s.m.cmp(c, pred::EQ, exp, k0);
        let m = self.s.m.select(c, sub, man, normal);
        let k25 = k(self, 25);
        let s_norm = self.s.m.bin(c, Bin::Sub, k25, exp);
        let k24 = k(self, 24);
        let sh = self.s.m.select(c, sub, k24, s_norm);
        let ks = k(self, scale);
        let p = self.s.m.bin(c, Bin::Mul, m, ks);
        let q = self.s.m.bin(c, Bin::LShr, p, sh);
        let one_s = self.s.m.bin(c, Bin::Shl, k1, sh);
        let low_mask = self.s.m.bin(c, Bin::Sub, one_s, k1);
        let rem = self.s.m.bin(c, Bin::And, p, low_mask);
        let half = self.s.m.bin(c, Bin::LShr, one_s, k1);
        let above = self.s.m.cmp(c, pred::IUGT, rem, half);
        let tie = self.s.m.cmp(c, pred::EQ, rem, half);
        let odd = self.s.m.bin(c, Bin::And, q, k1);
        let odd = self.s.m.cmp(c, pred::NE, odd, k0);
        let tie_up = self.s.m.bin(c, Bin::And, tie, odd);
        let up = self.s.m.bin(c, Bin::Or, above, tie_up);
        let r = self.s.m.select(c, up, k1, k0);
        let r = self.s.m.bin(c, Bin::Add, q, r);
        // |x| >= 1 (and infinity) saturates; NaN gives 0.
        let one_h = k(self, 0x3C00);
        let big = self.s.m.cmp(c, pred::IUGE, mag, one_h);
        let r = self.s.m.select(c, big, ks, r);
        let inf = k(self, 0x7C00);
        let nan = self.s.m.cmp(c, pred::IUGT, mag, inf);
        let r = self.s.m.select(c, nan, k0, r);
        let r = if signed {
            let n = self.s.m.bin(c, Bin::Sub, k0, r);
            self.s.m.select(c, neg, n, r)
        } else {
            self.s.m.select(c, neg, k0, r)
        };
        let m16 = k(self, 0xFFFF);
        Ok(self.s.m.bin(c, Bin::And, r, m16))
    }

    /// Half `high` of source `k` of a packed (VOP3P) operation, in the low
    /// 16 bits: op_sel picks the half the low result reads, op_sel_hi the
    /// half the high result reads. Packed integer instructions see the f32
    /// bits of an inline float constant.
    fn packed_src(&mut self, inst: &Inst, k: usize, high: bool, float: bool) -> Result<V, Why> {
        let Fields::Valu(v) = &inst.fields else {
            unreachable!()
        };
        let op = *v.src.get(k).ok_or("a missing operand")?;
        let sel = if high { v.opsel_hi } else { v.opsel } >> k & 1 != 0;
        let x = if float {
            self.f16_operand(inst, &op)?
        } else {
            self.operand(inst, &op)?
        };
        let c = self.cur;
        let x = if sel {
            let k16 = self.s.m.i32(16);
            self.s.m.bin(c, Bin::LShr, x, k16)
        } else {
            x
        };
        let m = self.s.m.i32(0xFFFF);
        Ok(self.s.m.bin(c, Bin::And, x, m))
    }

    /// Packed 16-bit operations (`v_pk_*`): each half computed as the
    /// 16-bit operation does, f16 through f32 as in [`Self::valu16`].
    /// `v_pk_fmac_f16`: each half of the destination plus the product of the
    /// sources' halves.
    // shortcut: FMad in f32, as v_fma_f16 (a second rounding).
    fn packed_fmac(&mut self, inst: &Inst, guard: Option<V>) -> Result<(), Why> {
        let Fields::Valu(v) = &inst.fields else {
            unreachable!()
        };
        let Some(Operand::Vector { reg, .. }) = v.dst else {
            return Err("a packed half FMA's destination");
        };
        if v.src.iter().any(|o| matches!(o, Operand::Const(_) | Operand::Literal(_)))
            || v.sdwa.is_some()
            || v.dpp.is_some()
        {
            return Err("a packed half FMA of constants");
        }
        let c = self.cur;
        let (i32t, f32t) = (self.s.t.i32, self.s.t.f32);
        let old = inst.read(Reg::V(reg)).ok_or("an unread destination")?;
        let words = [self.operand(inst, &v.src[0])?, self.operand(inst, &v.src[1])?, self.i32_of(old)?];
        let (k16, low) = (self.s.m.i32(16), self.s.m.i32(0xFFFF));
        let mut halves = Vec::new();
        for high in [false, true] {
            let xs: Vec<V> = words
                .iter()
                .map(|&w| {
                    let w = if high { self.s.m.bin(c, Bin::LShr, w, k16) } else { w };
                    let w = self.s.m.bin(c, Bin::And, w, low);
                    self.s.op(c, "legacyF16ToF32", Attr::ReadNone, f32t, 131, &[w])
                })
                .collect();
            let f = self.fmad(xs[0], xs[1], xs[2]);
            halves.push(self.s.op(c, "legacyF32ToF16", Attr::ReadNone, i32t, 130, &[f]));
        }
        let hi = self.s.m.bin(c, Bin::Shl, halves[1], k16);
        let lo = self.s.m.bin(c, Bin::And, halves[0], low);
        let x = self.s.m.bin(c, Bin::Or, lo, hi);
        self.write_i(inst, x, guard)
    }

    fn packed16(&mut self, inst: &Inst, op: &str, guard: Option<V>) -> Result<(), Why> {
        let Fields::Valu(v) = &inst.fields else {
            unreachable!()
        };
        let float = op.ends_with("_f16");
        let n = match op {
            "fma_f16" | "mad_u16" | "mad_i16" => 3,
            "add_f16" | "mul_f16" | "min_f16" | "max_f16" | "add_u16" | "sub_u16" | "add_i16"
            | "sub_i16" | "mul_lo_u16" | "lshlrev_b16" | "lshrrev_b16" | "ashrrev_i16"
            | "max_u16" | "min_u16" | "max_i16" | "min_i16" => 2,
            _ => return Err("a packed 16-bit operation"),
        };
        if v.omod != 0 || !float && (v.clamp || v.neg != 0 || v.neg_hi != 0) {
            return Err("modifiers on a packed integer operation");
        }
        let c = self.cur;
        let (i32t, f32t) = (self.s.t.i32, self.s.t.f32);
        let mut halves = Vec::new();
        for high in [false, true] {
            let mut xs = Vec::new();
            for k in 0..n {
                let h = self.packed_src(inst, k, high, float)?;
                if float {
                    let mut f = self
                        .s
                        .op(c, "legacyF16ToF32", Attr::ReadNone, f32t, 131, &[h]);
                    let neg = if high { v.neg_hi } else { v.neg };
                    if neg >> k & 1 != 0 {
                        let z = self.s.m.f32(0x8000_0000);
                        f = self.s.m.bin(c, Bin::Sub, z, f);
                    }
                    xs.push(f);
                } else {
                    xs.push(h);
                }
            }
            let r = if float {
                let mut f = match op {
                    "add_f16" => self.s.m.bin(c, Bin::Add, xs[0], xs[1]),
                    "mul_f16" => self.s.m.bin(c, Bin::Mul, xs[0], xs[1]),
                    "min_f16" => self.max_min(false, xs[0], xs[1]),
                    "max_f16" => self.max_min(true, xs[0], xs[1]),
                    // shortcut: FMad in f32, as v_fma_f16.
                    _ => self.fmad(xs[0], xs[1], xs[2]),
                };
                if v.clamp {
                    f = self.unary(7, f);
                }
                self.s
                    .op(c, "legacyF32ToF16", Attr::ReadNone, i32t, 130, &[f])
            } else {
                let k16 = self.s.m.i32(16);
                let sext = |l: &mut Self, x: V| {
                    let t = l.s.m.bin(c, Bin::Shl, x, k16);
                    l.s.m.bin(c, Bin::AShr, t, k16)
                };
                if op.ends_with("_i16") {
                    for x in &mut xs {
                        *x = sext(self, *x);
                    }
                }
                let k15 = self.s.m.i32(15);
                match op {
                    "add_u16" | "add_i16" => self.s.m.bin(c, Bin::Add, xs[0], xs[1]),
                    "sub_u16" | "sub_i16" => self.s.m.bin(c, Bin::Sub, xs[0], xs[1]),
                    "mul_lo_u16" => self.s.m.bin(c, Bin::Mul, xs[0], xs[1]),
                    "mad_u16" | "mad_i16" => {
                        let m = self.s.m.bin(c, Bin::Mul, xs[0], xs[1]);
                        self.s.m.bin(c, Bin::Add, m, xs[2])
                    }
                    "lshlrev_b16" | "lshrrev_b16" | "ashrrev_i16" => {
                        let sh = self.s.m.bin(c, Bin::And, xs[0], k15);
                        let o = match op {
                            "lshlrev_b16" => Bin::Shl,
                            "lshrrev_b16" => Bin::LShr,
                            _ => Bin::AShr,
                        };
                        let x = if op == "ashrrev_i16" {
                            sext(self, xs[1])
                        } else {
                            xs[1]
                        };
                        self.s.m.bin(c, o, x, sh)
                    }
                    _ => {
                        let p = match op {
                            "max_u16" => pred::IUGT,
                            "min_u16" => pred::IULT,
                            "max_i16" => pred::SGT,
                            _ => pred::SLT,
                        };
                        let t = self.s.m.cmp(c, p, xs[0], xs[1]);
                        self.s.m.select(c, t, xs[0], xs[1])
                    }
                }
            };
            let m = self.s.m.i32(0xFFFF);
            halves.push(self.s.m.bin(c, Bin::And, r, m));
        }
        let k16 = self.s.m.i32(16);
        let hi = self.s.m.bin(c, Bin::Shl, halves[1], k16);
        let r = self.s.m.bin(c, Bin::Or, halves[0], hi);
        let Some(Operand::Vector { reg, .. }) = v.dst else {
            return Err("a packed result outside a VGPR");
        };
        self.write(inst, Reg::V(reg), r, guard);
        Ok(())
    }

    /// The sign or carry bit above the 64-bit multiply-add is returned in
    /// SDST. LLVM's AMDGPURegisterBankInfo describes the signed form as the
    /// low bit of sign(product) + sign(accumulator) + unsigned carry-out.
    fn wide_mad(&mut self, inst: &Inst, guard: Option<V>) -> Result<(), Why> {
        let Fields::Valu(v) = &inst.fields else { unreachable!() };
        if v.clamp {
            return Err("clamped 64-bit multiply-add carry semantics");
        }
        let Some(Operand::Vector { reg, .. }) = v.dst else { return Err("a wide multiply-add result") };
        let signed = inst.name() == "v_mad_i64_i32";
        let (a, b) = (self.src(inst, 0)?, self.src(inst, 1)?);
        let (a, b) = (self.extend64(a, signed), self.extend64(b, signed));
        let acc = self.src64(inst, 2)?;
        let c = self.cur;
        let prod = self.s.m.bin(c, Bin::Mul, a, b);
        let sum = self.s.m.bin(c, Bin::Add, prod, acc);
        let mut carry = self.s.m.cmp(c, pred::IULT, sum, prod);
        if signed {
            let t = self.s.m.int(64);
            let z = self.s.m.int_const(t, 0);
            let pneg = self.s.m.cmp(c, pred::SLT, prod, z);
            let aneg = self.s.m.cmp(c, pred::SLT, acc, z);
            let signs = self.s.m.bin(c, Bin::Xor, pneg, aneg);
            carry = self.s.m.bin(c, Bin::Xor, carry, signs);
        }
        if let Some(Operand::Scalar { code, .. }) = v.carry
            && let Some(w) = inst.write(Reg::S(code))
        {
            self.lane_bits.insert(w, carry);
        }
        let (lo, hi) = self.split64(sum);
        self.write(inst, Reg::V(reg), lo, guard);
        self.write(inst, Reg::V(reg + 1), hi, guard);
        for &(_, w) in &inst.writes {
            self.vals
                .entry(w)
                .or_insert(Val::Missing("a lane mask read as data"));
        }
        Ok(())
    }

    fn int_field(&mut self, x: V, shift: u32, width: u32) -> V {
        if width == 32 {
            return x;
        }
        let sh = self.s.m.i32(shift);
        let mask = self.s.m.i32((1 << width) - 1);
        let x = self.s.m.bin(self.cur, Bin::LShr, x, sh);
        self.s.m.bin(self.cur, Bin::And, x, mask)
    }

    fn int3(&mut self, op: &str, signed: bool, a: V, b: V, d: V) -> V {
        let (lt, gt) = if signed { (pred::SLT, pred::SGT) } else { (pred::IULT, pred::IUGT) };
        match op {
            "min3" => {
                let t = self.int_pick(lt, a, b);
                self.int_pick(lt, t, d)
            }
            "max3" => {
                let t = self.int_pick(gt, a, b);
                self.int_pick(gt, t, d)
            }
            _ => {
                let lo = self.int_pick(lt, a, b);
                let hi = self.int_pick(gt, a, b);
                let t = self.int_pick(lt, hi, d);
                self.int_pick(gt, lo, t)
            }
        }
    }

    /// Integer dot products, including the optional output saturation in
    /// VOP3P (LLVM AMDGPUUsage's sdot/udot intrinsics).
    fn integer_dot(&mut self, inst: &Inst, guard: Option<V>) -> Result<(), Why> {
        let Fields::Valu(v) = &inst.fields else { unreachable!() };
        let name = inst.name();
        let implicit = name == "v_dot4c_i32_i8";
        let width = if name.starts_with("v_dot2") { 16 } else if name.starts_with("v_dot4") { 8 } else { 4 };
        let signed = name.contains("_i32");
        if v.neg != 0 || v.neg_hi != 0 || v.abs != 0 || v.omod != 0 || v.sdwa.is_some() {
            return Err("modifiers on an integer dot product");
        }
        let Some(Operand::Vector { reg, .. }) = v.dst else { return Err("an integer dot product result") };
        let c = self.cur;
        let a = self.src(inst, 0)?;
        let b = self.src(inst, 1)?;
        let d = if implicit {
            self.i32_of(inst.read(Reg::V(reg)).ok_or("a dot product accumulator")?)?
        } else {
            self.src(inst, 2)?
        };
        let mut sum = if v.clamp { self.extend64(d, signed) } else { d };
        for i in 0..32 / width {
            // dot2 has independent low/high half selects for each input.
            let (sa, sb) = if width == 16 {
                let sel = if i == 0 { v.opsel } else { v.opsel_hi };
                (u32::from(sel & 1) * 16, u32::from(sel >> 1 & 1) * 16)
            } else {
                (i * width, i * width)
            };
            let (mut a, mut b) = (self.int_field(a, sa, width), self.int_field(b, sb, width));
            if signed {
                a = self.sign_extend(a, width);
                b = self.sign_extend(b, width);
            }
            if v.clamp {
                a = self.extend64(a, signed);
                b = self.extend64(b, signed);
            }
            let p = self.s.m.bin(c, Bin::Mul, a, b);
            sum = self.s.m.bin(c, Bin::Add, sum, p);
        }
        if v.clamp {
            let (min, max) = if signed { (i32::MIN as i64, i32::MAX as i64) } else { (0, u32::MAX as i64) };
            sum = self.clamp_int(sum, 64, min, max, signed);
            sum = self.s.m.cast(c, Cast::Trunc, sum, self.s.t.i32);
        }
        self.write(inst, Reg::V(reg), sum, guard);
        Ok(())
    }

    /// Sum of the unsigned absolute differences of the packed elements.
    fn sad(&mut self, a: V, b: V, width: u32, masked: bool) -> V {
        let c = self.cur;
        let zero = self.s.m.i32(0);
        let mut sum = zero;
        for offset in (0..32).step_by(width as usize) {
            let x = self.int_field(a, offset, width);
            let y = self.int_field(b, offset, width);
            let ge = self.s.m.cmp(c, pred::IUGE, x, y);
            let xy = self.s.m.bin(c, Bin::Sub, x, y);
            let yx = self.s.m.bin(c, Bin::Sub, y, x);
            let mut diff = self.s.m.select(c, ge, xy, yx);
            if masked {
                let skip = self.s.m.cmp(c, pred::EQ, y, zero);
                diff = self.s.m.select(c, skip, zero, diff);
            }
            sum = self.s.m.bin(c, Bin::Add, sum, diff);
        }
        sum
    }

    fn integer(&mut self, inst: &Inst, name: &'static str) -> Result<V, Why> {
        let c = self.cur;
        let s = |l: &mut Self, k| l.src(inst, k);
        let k = |l: &mut Self, x: u32| l.s.m.i32(x);
        let m31 = |l: &mut Self, x: V| {
            let k31 = l.s.m.i32(31);
            l.s.m.bin(c, Bin::And, x, k31)
        };
        Ok(match name {
            "v_and_b32" | "v_or_b32" | "v_xor_b32" | "v_add_nc_u32" | "v_add_nc_i32"
            | "v_sub_nc_u32" | "v_sub_nc_i32" | "v_subrev_nc_u32" | "v_mul_lo_u32"
            | "v_mul_lo_i32" => {
                let (a, b) = (s(self, 0)?, s(self, 1)?);
                let (op, a, b) = match name {
                    "v_and_b32" => (Bin::And, a, b),
                    "v_or_b32" => (Bin::Or, a, b),
                    "v_xor_b32" => (Bin::Xor, a, b),
                    "v_add_nc_u32" | "v_add_nc_i32" => (Bin::Add, a, b),
                    "v_sub_nc_u32" | "v_sub_nc_i32" => (Bin::Sub, a, b),
                    "v_subrev_nc_u32" => (Bin::Sub, b, a),
                    _ => (Bin::Mul, a, b),
                };
                self.s.m.bin(c, op, a, b)
            }
            "v_xnor_b32" => {
                let (a, b) = (s(self, 0)?, s(self, 1)?);
                let x = self.s.m.bin(c, Bin::Xor, a, b);
                let ones = k(self, u32::MAX);
                self.s.m.bin(c, Bin::Xor, x, ones)
            }
            "v_not_b32" => {
                let a = s(self, 0)?;
                let ones = k(self, u32::MAX);
                self.s.m.bin(c, Bin::Xor, a, ones)
            }
            "v_lshlrev_b32" | "v_lshrrev_b32" | "v_ashrrev_i32" => {
                let (sh, a) = (s(self, 0)?, s(self, 1)?);
                let sh = m31(self, sh);
                let op = match name {
                    "v_lshlrev_b32" => Bin::Shl,
                    "v_lshrrev_b32" => Bin::LShr,
                    _ => Bin::AShr,
                };
                self.s.m.bin(c, op, a, sh)
            }
            "v_lshl_add_u32" | "v_lshl_or_b32" | "v_add_lshl_u32" | "v_and_or_b32"
            | "v_or3_b32" | "v_xad_u32" | "v_add3_u32" | "v_xor3_b32" => {
                let (a, b, d) = (s(self, 0)?, s(self, 1)?, s(self, 2)?);
                match name {
                    "v_lshl_add_u32" | "v_lshl_or_b32" => {
                        let b = m31(self, b);
                        let t = self.s.m.bin(c, Bin::Shl, a, b);
                        let op = if name == "v_lshl_add_u32" {
                            Bin::Add
                        } else {
                            Bin::Or
                        };
                        self.s.m.bin(c, op, t, d)
                    }
                    "v_add_lshl_u32" => {
                        let t = self.s.m.bin(c, Bin::Add, a, b);
                        let d = m31(self, d);
                        self.s.m.bin(c, Bin::Shl, t, d)
                    }
                    _ => {
                        let (o1, o2) = match name {
                            "v_and_or_b32" => (Bin::And, Bin::Or),
                            "v_or3_b32" => (Bin::Or, Bin::Or),
                            "v_xad_u32" => (Bin::Xor, Bin::Add),
                            "v_add3_u32" => (Bin::Add, Bin::Add),
                            _ => (Bin::Xor, Bin::Xor),
                        };
                        let t = self.s.m.bin(c, o1, a, b);
                        self.s.m.bin(c, o2, t, d)
                    }
                }
            }
            "v_mul_u32_u24" | "v_mul_i32_i24" | "v_mul_hi_i32_i24" | "v_mul_hi_u32_u24" => {
                let (a, b) = (s(self, 0)?, s(self, 1)?);
                let (sh, op) = (
                    k(self, 8),
                    if name.ends_with("i24") {
                        Bin::AShr
                    } else {
                        Bin::LShr
                    },
                );
                let a = self.s.m.bin(c, Bin::Shl, a, sh);
                let a = self.s.m.bin(c, op, a, sh);
                let b = self.s.m.bin(c, Bin::Shl, b, sh);
                let b = self.s.m.bin(c, op, b, sh);
                if name.starts_with("v_mul_hi") {
                    self.mul_hi(a, b, name.ends_with("i24"))
                } else {
                    self.s.m.bin(c, Bin::Mul, a, b)
                }
            }
            "v_sad_u8" | "v_sad_hi_u8" | "v_sad_u16" | "v_sad_u32" | "v_msad_u8" => {
                // RDNA 2 ISA, VOP3A: accumulation wraps in the dword.
                let (a, b, d) = (s(self, 0)?, s(self, 1)?, s(self, 2)?);
                let width = if name.ends_with("u8") { 8 } else if name.ends_with("u16") { 16 } else { 32 };
                let sum = self.sad(a, b, width, name == "v_msad_u8");
                let sum = if name == "v_sad_hi_u8" {
                    let k16 = k(self, 16);
                    self.s.m.bin(c, Bin::Shl, sum, k16)
                } else {
                    sum
                };
                self.s.m.bin(c, Bin::Add, sum, d)
            }
            "v_lerp_u8" => {
                // RDNA 2 ISA, VOP3A: low bit of each byte of S2 rounds a tie up.
                let (a, b, d) = (s(self, 0)?, s(self, 1)?, s(self, 2)?);
                let one = k(self, 1);
                let mut result = k(self, 0);
                for shift in [0, 8, 16, 24] {
                    let x = self.int_field(a, shift, 8);
                    let y = self.int_field(b, shift, 8);
                    let round = self.int_field(d, shift, 1);
                    let sum = self.s.m.bin(c, Bin::Add, x, y);
                    let sum = self.s.m.bin(c, Bin::Add, sum, round);
                    let avg = self.s.m.bin(c, Bin::LShr, sum, one);
                    let sh = k(self, shift);
                    let field = self.s.m.bin(c, Bin::Shl, avg, sh);
                    result = self.s.m.bin(c, Bin::Or, result, field);
                }
                result
            }
            "v_bfe_u32" | "v_bfe_i32" => {
                let (a, off, w) = (s(self, 0)?, s(self, 1)?, s(self, 2)?);
                let w = m31(self, w);
                self.bfe(a, off, w, name == "v_bfe_i32")
            }
            "v_bfi_b32" => {
                let (m, a, b) = (s(self, 0)?, s(self, 1)?, s(self, 2)?);
                let ones = k(self, u32::MAX);
                let nm = self.s.m.bin(c, Bin::Xor, m, ones);
                let x = self.s.m.bin(c, Bin::And, m, a);
                let y = self.s.m.bin(c, Bin::And, nm, b);
                self.s.m.bin(c, Bin::Or, x, y)
            }
            "v_max_i32" | "v_min_i32" | "v_max_u32" | "v_min_u32" => {
                let (a, b) = (s(self, 0)?, s(self, 1)?);
                let p = match name {
                    "v_max_i32" => pred::SGT,
                    "v_min_i32" => pred::SLT,
                    "v_max_u32" => pred::IUGT,
                    _ => pred::IULT,
                };
                let t = self.s.m.cmp(c, p, a, b);
                self.s.m.select(c, t, a, b)
            }
            "v_mul_hi_u32" | "v_mul_hi_i32" => {
                let (a, b) = (s(self, 0)?, s(self, 1)?);
                self.mul_hi(a, b, name == "v_mul_hi_i32")
            }
            "v_cvt_pk_u16_u32" | "v_cvt_pk_i16_i32" => {
                // Each source saturated to 16 bits: src0 into the low half,
                // src1 into the high one.
                let unsigned = name == "v_cvt_pk_u16_u32";
                let (a, b) = (s(self, 0)?, s(self, 1)?);
                let mut halves = [a, b];
                for x in &mut halves {
                    *x = if unsigned {
                        let max = k(self, 0xFFFF);
                        let over = self.s.m.cmp(c, pred::IUGT, *x, max);
                        self.s.m.select(c, over, max, *x)
                    } else {
                        let (lo, hi, m) = (k(self, 0xFFFF_8000), k(self, 0x7FFF), k(self, 0xFFFF));
                        let below = self.s.m.cmp(c, pred::SLT, *x, lo);
                        let t = self.s.m.select(c, below, lo, *x);
                        let above = self.s.m.cmp(c, pred::SGT, t, hi);
                        let t = self.s.m.select(c, above, hi, t);
                        self.s.m.bin(c, Bin::And, t, m)
                    };
                }
                let k16 = k(self, 16);
                let hi = self.s.m.bin(c, Bin::Shl, halves[1], k16);
                self.s.m.bin(c, Bin::Or, halves[0], hi)
            }
            "v_dot2_i32_i16" | "v_dot2_u32_u16" | "v_dot4_i32_i8" | "v_dot4_u32_u8"
            | "v_dot8_u32_u4" => {
                // The products of the packed fields of src0 and src1, summed
                // into src2, wrapping (clamp is refused with the other
                // integer output modifiers).
                let Fields::Valu(v) = &inst.fields else {
                    unreachable!()
                };
                if v.opsel != 0 || v.neg_hi != 0 {
                    return Err("dot product operand selects");
                }
                let (bits, signed) = match name {
                    "v_dot2_i32_i16" => (16, true),
                    "v_dot2_u32_u16" => (16, false),
                    "v_dot4_i32_i8" => (8, true),
                    "v_dot4_u32_u8" => (8, false),
                    _ => (4, false),
                };
                let (a, b) = (s(self, 0)?, s(self, 1)?);
                let mut sum = s(self, 2)?;
                let op = if signed { Bin::AShr } else { Bin::LShr };
                for i in 0..32 / bits {
                    // Field i to the top, then down again (extending it).
                    let (up, down) = (k(self, 32 - bits * (i + 1)), k(self, 32 - bits));
                    let x = self.s.m.bin(c, Bin::Shl, a, up);
                    let x = self.s.m.bin(c, op, x, down);
                    let y = self.s.m.bin(c, Bin::Shl, b, up);
                    let y = self.s.m.bin(c, op, y, down);
                    let p = self.s.m.bin(c, Bin::Mul, x, y);
                    sum = self.s.m.bin(c, Bin::Add, sum, p);
                }
                sum
            }
            "v_alignbit_b32" | "v_alignbyte_b32" => {
                // The low dword of {a, b} >> shift.
                let (a, b, sh) = (s(self, 0)?, s(self, 1)?, s(self, 2)?);
                let sh = if name == "v_alignbyte_b32" {
                    let three = k(self, 3);
                    let t = self.s.m.bin(c, Bin::And, sh, three);
                    let k3 = k(self, 3);
                    self.s.m.bin(c, Bin::Shl, t, k3)
                } else {
                    m31(self, sh)
                };
                let k32 = k(self, 32);
                let lo = self.s.m.bin(c, Bin::LShr, b, sh);
                let left = self.s.m.bin(c, Bin::Sub, k32, sh);
                let left = m31(self, left);
                let hi = self.s.m.bin(c, Bin::Shl, a, left);
                let zero = k(self, 0);
                let none = self.s.m.cmp(c, pred::EQ, sh, zero);
                let hi = self.s.m.select(c, none, zero, hi);
                self.s.m.bin(c, Bin::Or, lo, hi)
            }
            "v_bfrev_b32" => {
                let a = s(self, 0)?;
                self.s
                    .op(c, "unary.i32", Attr::ReadNone, self.s.t.i32, 30, &[a])
            }
            "v_bcnt_u32_b32" => {
                let (a, b) = (s(self, 0)?, s(self, 1)?);
                let n = self
                    .s
                    .op(c, "unaryBits.i32", Attr::ReadNone, self.s.t.i32, 31, &[a]);
                self.s.m.bin(c, Bin::Add, n, b)
            }
            "v_mad_u32_u24" | "v_mad_i32_i24" => {
                let (a, b, d) = (s(self, 0)?, s(self, 1)?, s(self, 2)?);
                let (sh, op) = (
                    k(self, 8),
                    if name == "v_mad_i32_i24" {
                        Bin::AShr
                    } else {
                        Bin::LShr
                    },
                );
                let a = self.s.m.bin(c, Bin::Shl, a, sh);
                let a = self.s.m.bin(c, op, a, sh);
                let b = self.s.m.bin(c, Bin::Shl, b, sh);
                let b = self.s.m.bin(c, op, b, sh);
                let m = self.s.m.bin(c, Bin::Mul, a, b);
                self.s.m.bin(c, Bin::Add, m, d)
            }
            "v_min3_i32" | "v_max3_i32" | "v_min3_u32" | "v_max3_u32" | "v_med3_i32"
            | "v_med3_u32" => {
                let (a, b, d) = (s(self, 0)?, s(self, 1)?, s(self, 2)?);
                self.int3(&name[2..6], name.ends_with("i32"), a, b, d)
            }
            "v_perm_b32" => {
                // Byte i of the result is byte S of {src0, src1} (src1 the
                // low dword) for selector byte S below 8; 8-11 repeat the
                // sign of bytes 1 and 3 of src1, then of src0; 12 is 0x00,
                // higher 0xFF.
                let Fields::Valu(v) = &inst.fields else {
                    unreachable!()
                };
                let sel = match v.src.get(2) {
                    Some(&Operand::Literal(x)) => x,
                    Some(&Operand::Const(k)) => crate::resource::inline(k).ok_or(name)?,
                    _ => {
                        // A computed selector: each byte picked at run time.
                        let (a, b, sv) = (s(self, 0)?, s(self, 1)?, s(self, 2)?);
                        let mut r = k(self, 0);
                        for i in 0..4u32 {
                            let (sh, ff) = (k(self, 8 * i), k(self, 0xFF));
                            let sel = self.s.m.bin(c, Bin::LShr, sv, sh);
                            let sel = self.s.m.bin(c, Bin::And, sel, ff);
                            // Select a dword and a byte within it.
                            // Later selects handle codes 8 and above.
                            let four = k(self, 4);
                            let zero = k(self, 0);
                            let high = self.s.m.bin(c, Bin::And, sel, four);
                            let high = self.s.m.cmp(c, pred::NE, high, zero);
                            let word = self.s.m.select(c, high, a, b);
                            let three = k(self, 3);
                            let index = self.s.m.bin(c, Bin::And, sel, three);
                            let bits = self.s.m.bin(c, Bin::Shl, index, three);
                            let x = self.s.m.bin(c, Bin::LShr, word, bits);
                            let mut byte = self.s.m.bin(c, Bin::And, x, ff);
                            // 8-11: the sign of bytes 1, 3 of src1, then of src0.
                            for (n, src, bit) in [(8, b, 15), (9, b, 31), (10, a, 15), (11, a, 31)]
                            {
                                let left = k(self, 31 - bit);
                                let t = self.s.m.bin(c, Bin::Shl, src, left);
                                let k31 = k(self, 31);
                                let t = self.s.m.bin(c, Bin::AShr, t, k31);
                                let t = self.s.m.bin(c, Bin::And, t, ff);
                                let kn = k(self, n);
                                let is = self.s.m.cmp(c, pred::EQ, sel, kn);
                                byte = self.s.m.select(c, is, t, byte);
                            }
                            let (k12, z) = (k(self, 12), k(self, 0));
                            let is12 = self.s.m.cmp(c, pred::EQ, sel, k12);
                            byte = self.s.m.select(c, is12, z, byte);
                            let above = self.s.m.cmp(c, pred::IUGT, sel, k12);
                            byte = self.s.m.select(c, above, ff, byte);
                            let at = k(self, 8 * i);
                            let byte = self.s.m.bin(c, Bin::Shl, byte, at);
                            r = self.s.m.bin(c, Bin::Or, r, byte);
                        }
                        return Ok(r);
                    }
                };
                let (a, b) = (s(self, 0)?, s(self, 1)?);
                let mut r = k(self, 0);
                for i in 0..4 {
                    let pick = sel >> (8 * i) & 0xFF;
                    let byte = match pick {
                        0..=7 => {
                            let (src, n) = if pick < 4 { (b, pick) } else { (a, pick - 4) };
                            let sh = k(self, 8 * n);
                            let t = self.s.m.bin(c, Bin::LShr, src, sh);
                            let ff = k(self, 0xFF);
                            self.s.m.bin(c, Bin::And, t, ff)
                        }
                        8..=11 => {
                            let src = if pick < 10 { b } else { a };
                            let bit = if pick % 2 == 0 { 15 } else { 31 };
                            let left = k(self, 31 - bit);
                            let t = self.s.m.bin(c, Bin::Shl, src, left);
                            let k31 = k(self, 31);
                            let t = self.s.m.bin(c, Bin::AShr, t, k31);
                            let ff = k(self, 0xFF);
                            self.s.m.bin(c, Bin::And, t, ff)
                        }
                        12 => continue,
                        _ => k(self, 0xFF),
                    };
                    let at = k(self, 8 * i);
                    let byte = self.s.m.bin(c, Bin::Shl, byte, at);
                    r = self.s.m.bin(c, Bin::Or, r, byte);
                }
                r
            }
            "v_bfm_b32" => {
                // ((1 << src0[4:0]) - 1) << src1[4:0]
                let (a, b) = (s(self, 0)?, s(self, 1)?);
                let one = k(self, 1);
                let (a, b) = (m31(self, a), m31(self, b));
                let m = self.s.m.bin(c, Bin::Shl, one, a);
                let m = self.s.m.bin(c, Bin::Sub, m, one);
                self.s.m.bin(c, Bin::Shl, m, b)
            }
            "v_cvt_pk_u8_f32" => {
                // src0 rounded to nearest even and saturated to a byte, into
                // byte src1[1:0] of src2.
                let f = self.fsrc(inst, 0)?;
                let (b, d) = (s(self, 1)?, s(self, 2)?);
                let f = self.unary(26, f);
                let (zero, max) = (self.s.m.f32(0), self.s.m.f32(255.0f32.to_bits()));
                let f = self.binary(35, f, zero);
                let f = self.binary(36, f, max);
                let x = self.s.m.cast(c, Cast::FpToUi, f, self.s.t.i32);
                let (three, k3) = (k(self, 3), k(self, 3));
                let sh = self.s.m.bin(c, Bin::And, b, three);
                let sh = self.s.m.bin(c, Bin::Shl, sh, k3);
                let x = self.s.m.bin(c, Bin::Shl, x, sh);
                let ff = k(self, 0xFF);
                let hole = self.s.m.bin(c, Bin::Shl, ff, sh);
                let ones = k(self, u32::MAX);
                let hole = self.s.m.bin(c, Bin::Xor, hole, ones);
                let d = self.s.m.bin(c, Bin::And, d, hole);
                self.s.m.bin(c, Bin::Or, d, x)
            }
            "v_ffbl_b32" => {
                // -1 for zero, as DXIL's first-bit-low.
                let a = s(self, 0)?;
                self.s
                    .op(c, "unaryBits.i32", Attr::ReadNone, self.s.t.i32, 32, &[a])
            }
            "v_mbcnt_lo_u32_b32" | "v_mbcnt_hi_u32_b32" => {
                // Bits of the mask's low (high) half below this lane, plus src1:
                // with an all-ones mask, the lane index.
                let b = s(self, 1)?;
                // Mesh lanes are the console's by construction; pixel and
                // vertex lanes are the host's, as their masks are.
                if self.mask_data == MaskData::Host {
                    // Host lanes: the mask must be one (a ballot) or all lanes.
                    let Fields::Valu(v) = &inst.fields else {
                        unreachable!()
                    };
                    if name == "v_mbcnt_lo_u32_b32" && !self.x.native.wave64 {
                        if matches!(v.src.first(), Some(&Operand::Const(193))) {
                            self.s.flags |= flags::WAVE_OPS;
                            let lane = self.lane_index();
                            return Ok(self.s.m.bin(c, Bin::Add, lane, b));
                        }
                        if let Some((lo, hi)) = v
                            .src
                            .first()
                            .map(|op| self.host_mask_pair(inst, op))
                            .transpose()?
                            .flatten()
                        {
                            self.s.flags |= flags::WAVE_OPS;
                            let lane = self.lane_index();
                            let m_lo = self.below_mask(c, lane);
                            let (k32, z) = (k(self, 32), k(self, 0));
                            let t = self.s.m.bin(c, Bin::Sub, lane, k32);
                            let low = self.s.m.cmp(c, pred::IULT, lane, k32);
                            let below_hi = self.s.m.select(c, low, z, t);
                            let m_hi = self.below_mask(c, below_hi);
                            let x_lo = self.s.m.bin(c, Bin::And, lo, m_lo);
                            let x_hi = self.s.m.bin(c, Bin::And, hi, m_hi);
                            let n = self.bcnt1_64(c, x_lo, x_hi);
                            return Ok(self.s.m.bin(c, Bin::Add, n, b));
                        }
                    }
                    let host = match v.src.first() {
                        Some(&Operand::Const(193)) => true,
                        Some(&Operand::Scalar { code, .. }) => {
                            inst.read(Reg::S(code)).is_some_and(|m| {
                                self.x.st.masks.contains_key(&m) || self.x.st.highs.contains_key(&m)
                            })
                        }
                        _ => false,
                    };
                    if !host {
                        return Err("v_mbcnt of a mask that is no ballot (console lane layout)");
                    }
                    self.s.flags |= flags::WAVE_OPS;
                } else if self.s.mesh.is_none() {
                    self.cross_lane(false)?;
                }
                let a = s(self, 0)?;
                let lane = if self.console_cf {
                    self.console_lane().0
                } else {
                    self.lane_index()
                };
                let below = if name == "v_mbcnt_lo_u32_b32" {
                    lane
                } else {
                    let (k32, z) = (k(self, 32), k(self, 0));
                    let t = self.s.m.bin(c, Bin::Sub, lane, k32);
                    let low = self.s.m.cmp(c, pred::IULT, lane, k32);
                    self.s.m.select(c, low, z, t)
                };
                let m = self.below_mask(c, below);
                let x = self.s.m.bin(c, Bin::And, a, m);
                let n = self
                    .s
                    .op(c, "unaryBits.i32", Attr::ReadNone, self.s.t.i32, 31, &[x]);
                self.s.m.bin(c, Bin::Add, n, b)
            }
            _ => return Err(name),
        })
    }

    /// AMD RDNA 2 ISA, VOPC: classify the input bits without converting to a
    /// host float. Conversion can quiet an sNaN or flush a denormal.
    fn fp_class(&mut self, inst: &Inst, format: &str) -> Result<V, Why> {
        let (bits, fraction, exp_mask) = match format {
            "f16" => (16, 10, 0x1F),
            "f32" => (32, 23, 0xFF),
            _ => (64, 52, 0x7FF),
        };
        let c = self.cur;
        let ty = self.s.m.int(bits.max(32));
        let x = self.fp_bits(inst, 0, bits)?;
        let mask = if bits == 16 { self.half_src(inst, 1)? } else { self.src(inst, 1)? };
        let (zero, sh, emax, frac, sign, quiet) = (
            self.s.m.int_const(ty, 0), self.s.m.int_const(ty, fraction),
            self.s.m.int_const(ty, exp_mask), self.s.m.int_const(ty, (1 << fraction) - 1),
            self.s.m.int_const(ty, (1u64 << (bits - 1)) as i64),
            self.s.m.int_const(ty, 1 << (fraction - 1)),
        );
        let e = self.s.m.bin(c, Bin::LShr, x, sh);
        let e = self.s.m.bin(c, Bin::And, e, emax);
        let m = self.s.m.bin(c, Bin::And, x, frac);
        let neg = self.s.m.bin(c, Bin::And, x, sign);
        let q = self.s.m.bin(c, Bin::And, x, quiet);
        let subnormal = self.s.m.cmp(c, pred::EQ, e, zero);
        let nonfinite = self.s.m.cmp(c, pred::EQ, e, emax);
        let no_frac = self.s.m.cmp(c, pred::EQ, m, zero);
        let has_frac = self.s.m.cmp(c, pred::NE, m, zero);
        let neg = self.s.m.cmp(c, pred::NE, neg, zero);
        let quiet = self.s.m.cmp(c, pred::NE, q, zero);
        // Positive classes 6-9, negative classes 11 minus those.
        let zero = self.s.m.i32(0);
        let (one, six, seven, eight, nine, eleven) = (self.s.m.i32(1), self.s.m.i32(6),
            self.s.m.i32(7), self.s.m.i32(8), self.s.m.i32(9), self.s.m.i32(11));
        let denorm_or_zero = self.s.m.select(c, no_frac, six, seven);
        let class = self.s.m.select(c, subnormal, denorm_or_zero, eight);
        let class = self.s.m.select(c, nonfinite, nine, class);
        let mirrored = self.s.m.bin(c, Bin::Sub, eleven, class);
        let class = self.s.m.select(c, neg, mirrored, class);
        let nan = self.s.m.bin(c, Bin::And, nonfinite, has_frac);
        let nan_class = self.s.m.select(c, quiet, one, zero);
        let class = self.s.m.select(c, nan, nan_class, class);
        let bit = self.s.m.bin(c, Bin::LShr, mask, class);
        let bit = self.s.m.bin(c, Bin::And, bit, one);
        Ok(self.s.m.cmp(c, pred::NE, bit, zero))
    }

    /// Compare double encodings in IEEE order using integer operations.
    /// Flipping the sign for nonnegative values and complementing negative
    /// ones makes their unsigned order match their numeric order.
    fn compare_f64(&mut self, inst: &Inst, op: &str) -> Result<V, Why> {
        if matches!(op, "f" | "tru") {
            return Ok(self.s.m.bool(op == "tru"));
        }
        let c = self.cur;
        let (a, b) = (self.fp_bits(inst, 0, 64)?, self.fp_bits(inst, 1, 64)?);
        let ty = self.s.m.int(64);
        let (z, sign, abs, inf, ones) = (self.s.m.int_const(ty, 0), self.s.m.int_const(ty, i64::MIN),
            self.s.m.int_const(ty, i64::MAX), self.s.m.int_const(ty, 0x7FF0_0000_0000_0000),
            self.s.m.int_const(ty, -1));
        let abs_a = self.s.m.bin(c, Bin::And, a, abs);
        let abs_b = self.s.m.bin(c, Bin::And, b, abs);
        let nan_a = self.s.m.cmp(c, pred::IUGT, abs_a, inf);
        let nan_b = self.s.m.cmp(c, pred::IUGT, abs_b, inf);
        let unordered = self.s.m.bin(c, Bin::Or, nan_a, nan_b);
        let yes = self.s.m.bool(true);
        let ordered = self.s.m.bin(c, Bin::Xor, unordered, yes);
        if op == "u" || op == "o" {
            return Ok(if op == "u" { unordered } else { ordered });
        }
        let key = |l: &mut Self, x, magnitude| {
            let zero = l.s.m.cmp(c, pred::EQ, magnitude, z);
            let x = l.s.m.select(c, zero, z, x); // both zeros compare equal
            let neg = l.s.m.cmp(c, pred::SLT, x, z);
            let xor = l.s.m.select(c, neg, ones, sign);
            l.s.m.bin(c, Bin::Xor, x, xor)
        };
        let (a, b) = (key(self, a, abs_a), key(self, b, abs_b));
        let (p, negated) = match op {
            "lt" | "nlt" => (pred::IULT, op == "nlt"),
            "eq" | "neq" => (pred::EQ, op == "neq"),
            "le" | "nle" => (pred::IULE, op == "nle"),
            "gt" | "ngt" => (pred::IUGT, op == "ngt"),
            "lg" | "nlg" => (pred::NE, op == "nlg"),
            "ge" | "nge" => (pred::IUGE, op == "nge"),
            _ => return Err("a double compare predicate"),
        };
        let test = self.s.m.cmp(c, p, a, b);
        let test = self.s.m.bin(c, Bin::And, ordered, test);
        Ok(if negated { self.s.m.bin(c, Bin::Xor, test, yes) } else { test })
    }

    fn compare(&mut self, inst: &Inst, rest: &str) -> Result<(), Why> {
        let (op, ty) = rest.rsplit_once('_').ok_or("a compare")?;
        let c = self.cur;
        let r = match ty {
            "f16" | "f32" | "f64" if op == "class" => self.fp_class(inst, ty)?,
            "f64" => self.compare_f64(inst, op)?,
            "f32" | "f16" => {
                let (a, b) = if ty == "f16" {
                    (self.half_fsrc(inst, 0)?, self.half_fsrc(inst, 1)?)
                } else {
                    (self.fsrc(inst, 0)?, self.fsrc(inst, 1)?)
                };
                // `fsrc` may emit two separate bitcasts for the same register,
                // which host CSE later merges into `fcmp ... %f, %f`.
                let same_src = a == b
                    || (self.src0.is_none()
                        && matches!(
                            &inst.fields,
                            Fields::Valu(v)
                                if v.src.len() >= 2
                                    && v.src[0] == v.src[1]
                                    && (v.abs & 1) == ((v.abs >> 1) & 1)
                                    && (v.neg & 1) == ((v.neg >> 1) & 1)
                                    && (v.opsel & 1) == ((v.opsel >> 1) & 1)
                                    && v.sdwa.is_none()
                        ));
                // Some D3D12 drivers (observed on Intel Iris Xe) assume no NaNs
                // for plain `fcmp`, folding `fcmp ord` to true, `fcmp uno` to
                // false (even across distinct registers), and self-comparisons
                // like `fcmp oeq/une x, x` to constants. Use DXIL `IsNaN`
                // (`dx.op.isSpecialFloat.f32`, opcode 8) for ordered/unordered
                // and same-value NaN-sensitive predicates.
                let is_nan = |l: &mut Self, x: V| {
                    l.s.op(c, "isSpecialFloat.f32", Attr::ReadNone, l.s.t.i1, 8, &[x])
                };
                let any_nan = |l: &mut Self, x: V, y: V| {
                    let nx = is_nan(l, x);
                    if same_src {
                        nx
                    } else {
                        let ny = is_nan(l, y);
                        l.s.m.bin(c, Bin::Or, nx, ny)
                    }
                };
                match op {
                    "u" => any_nan(self, a, b),
                    "o" => {
                        let u = any_nan(self, a, b);
                        let yes = self.s.m.bool(true);
                        self.s.m.bin(c, Bin::Xor, u, yes)
                    }
                    "eq" | "le" | "ge" if same_src => {
                        let n = is_nan(self, a);
                        let yes = self.s.m.bool(true);
                        self.s.m.bin(c, Bin::Xor, n, yes)
                    }
                    "neq" | "nge" | "nle" if same_src => is_nan(self, a),
                    // Ordered and not equal, false for a NaN, as `olt | ogt`.
                    // `fcmp one` means the same, but the AMD D3D12 driver
                    // runs it as unordered (true for a NaN) where NVIDIA
                    // and WARP do not; olt and ogt agree on all three.
                    "lg" => {
                        let lt = self.s.m.cmp(c, pred::OLT, a, b);
                        let gt = self.s.m.cmp(c, pred::OGT, a, b);
                        self.s.m.bin(c, Bin::Or, lt, gt)
                    }
                    _ => {
                        let p = match op {
                            "f" => pred::FALSE,
                            "lt" => pred::OLT,
                            "eq" => pred::OEQ,
                            "le" => pred::OLE,
                            "gt" => pred::OGT,
                            "ge" => pred::OGE,
                            "nge" => pred::ULT,
                            "nlg" => pred::UEQ,
                            "ngt" => pred::ULE,
                            "nle" => pred::UGT,
                            "neq" => pred::UNE,
                            "nlt" => pred::UGE,
                            "tru" => pred::TRUE,
                            _ => return Err(inst.name()),
                        };
                        self.s.m.cmp(c, p, a, b)
                    }
                }
            }
            "u64" | "i64" if matches!(op, "f" | "t") => self.s.m.bool(op == "t"),
            "u64" | "i64" => {
                let (a, b) = (self.src64(inst, 0)?, self.src64(inst, 1)?);
                let signed = ty == "i64";
                let p = match op {
                    "eq" => pred::EQ,
                    "ne" | "lg" => pred::NE,
                    "lt" if signed => pred::SLT,
                    "le" if signed => pred::SLE,
                    "gt" if signed => pred::SGT,
                    "ge" if signed => pred::SGE,
                    "lt" => pred::IULT,
                    "le" => pred::IULE,
                    "gt" => pred::IUGT,
                    "ge" => pred::IUGE,
                    _ => return Err("a 64-bit compare"),
                };
                self.s.m.cmp(c, p, a, b)
            }
            "u32" | "i32" | "u16" | "i16" => {
                let (a, b) = if ty.ends_with("16") {
                    let (mut a, mut b) = (self.half_src(inst, 0)?, self.half_src(inst, 1)?);
                    if ty == "i16" {
                        let k16 = self.s.m.i32(16);
                        for x in [&mut a, &mut b] {
                            let t = self.s.m.bin(c, Bin::Shl, *x, k16);
                            *x = self.s.m.bin(c, Bin::AShr, t, k16);
                        }
                    }
                    (a, b)
                } else {
                    (self.src(inst, 0)?, self.src(inst, 1)?)
                };
                let signed = ty == "i32" || ty == "i16";
                let p = match op {
                    "eq" => pred::EQ,
                    "ne" | "lg" => pred::NE,
                    "lt" if signed => pred::SLT,
                    "le" if signed => pred::SLE,
                    "gt" if signed => pred::SGT,
                    "ge" if signed => pred::SGE,
                    "lt" => pred::IULT,
                    "le" => pred::IULE,
                    "gt" => pred::IUGT,
                    "ge" => pred::IUGE,
                    "f" | "t" => {
                        self.lane(inst, Some(op == "t"));
                        return Ok(());
                    }
                    _ => return Err("an integer compare"),
                };
                self.s.m.cmp(c, p, a, b)
            }
            _ => return Err(inst.name()),
        };
        self.lane_bits_for(inst, r);
        Ok(())
    }

    fn lane(&mut self, inst: &Inst, value: Option<bool>) {
        let b = self.s.m.bool(value == Some(true));
        self.lane_bits_for(inst, b);
    }

    fn lane_bits_for(&mut self, inst: &Inst, r: V) {
        let Fields::Valu(v) = &inst.fields else {
            return;
        };
        let code = if inst.name().starts_with("v_cmpx") {
            Some(EXEC)
        } else {
            match v.dst {
                Some(Operand::Scalar { code, .. }) => Some(code),
                _ => None,
            }
        };
        if let Some(lo) = code.and_then(|c| inst.write(Reg::S(c))) {
            self.lane_bits.insert(lo, r);
        }
        for &(_, w) in &inst.writes {
            self.vals
                .entry(w)
                .or_insert(Val::Missing("a lane mask read as data"));
        }
    }

    fn interp(&mut self, inst: &Inst, site: Site) -> Result<(), Why> {
        let Some(&interp) = self.interps.get(&site) else {
            return Err("the first half of an interpolation");
        };
        let name = inst.name();
        let (dst, src) = match &inst.fields {
            Fields::Vintrp { dst, src, .. } => (*dst, Some(*src)),
            Fields::Valu(v) => {
                if v.abs != 0 || v.neg != 0 {
                    return Err("interpolation operand modifiers");
                }
                (v.dst.ok_or("an interpolation")?, v.src.get(1).copied())
            }
            _ => unreachable!(),
        };
        let Operand::Vector { reg, .. } = dst else {
            return Err("an interpolation");
        };
        // A float operand of the instruction.
        let float = |l: &mut Self, op: Option<Operand>| -> Result<V, Why> {
            let x = l.operand(inst, &op.ok_or("an interpolation operand")?)?;
            Ok(l.as_float(x))
        };
        // What the result adds to the evaluated channel (a second half's
        // first half, when the halves evaluate on their own).
        let mut add = None;
        let (attribute, eval) = match *interp {
            Interp::Interpolated {
                attribute,
                barycentric,
                ..
            } => {
                let mode = interp_mode(barycentric).ok_or("pull-model interpolation")?;
                let eval = if self.per_vertex(attribute.row) {
                    // The element is read per vertex elsewhere, so it is
                    // constant: weigh its vertices with SV_Barycentrics.
                    let mut ij = [None; 2];
                    for (index, w) in ij.iter_mut().enumerate() {
                        let x = match self.barycentric(barycentric, index as u8) {
                            Val::I32(x) => x,
                            Val::Missing(w) => return Err(w),
                            Val::Bit(_) => unreachable!(),
                        };
                        *w = Some(self.as_float(x));
                    }
                    Eval::Weights {
                        p0: true,
                        i: ij[0],
                        j: ij[1],
                    }
                } else {
                    Eval::Mode(mode)
                };
                (attribute, eval)
            }
            // P0, the provoking vertex: a flat (nointerpolation) input.
            Interp::Vertex {
                attribute,
                vertex: 2,
            } => (attribute, Eval::Mode(1)),
            // P10 and P20: what the first and second halves weigh, P1 - P0
            // and P2 - P0, which is what the hardware holds for an
            // attribute the program also interpolates.
            Interp::Vertex { attribute, vertex } if self.interpolated(attribute.row) => {
                let one = Some(self.s.m.f32(1f32.to_bits()));
                let (i, j) = if vertex == 0 {
                    (one, None)
                } else {
                    (None, one)
                };
                (attribute, Eval::Weights { p0: false, i, j })
            }
            Interp::Vertex { .. } => return Err("a non-provoking vertex value"),
            // A half of an interpolation with computed i or j: P0 + i P10,
            // or the old dst (f16: the third operand) + j P20.
            Interp::Weighted { attribute } if name.starts_with("v_interp_p1") => {
                let i = Some(float(self, src)?);
                (
                    attribute,
                    Eval::Weights {
                        p0: true,
                        i,
                        j: None,
                    },
                )
            }
            Interp::Weighted { attribute } => {
                let old = match &inst.fields {
                    Fields::Valu(v) => v.src.get(2).copied(),
                    _ => Some(dst),
                };
                add = Some(float(self, old)?);
                let j = Some(float(self, src)?);
                (
                    attribute,
                    Eval::Weights {
                        p0: false,
                        i: None,
                        j,
                    },
                )
            }
        };
        let packing = self.half_inputs.get(&attribute.row).copied();
        let f16 = name.ends_with("_f16");
        // The evaluated channel plus `add`, as a float.
        let total = |l: &mut Self, row: u8, ch: u8| -> Result<V, Why> {
            let x = l.eval_channel(row, ch, eval)?;
            let x = l.as_float(x);
            Ok(match add {
                Some(a) => l.s.m.bin(l.cur, Bin::Add, a, x),
                None => x,
            })
        };
        let x = match packing {
            None if f16 || attribute.high => return Err("a 16-bit read of a 32-bit input"),
            None => {
                let x = total(self, attribute.row, attribute.channel)?;
                self.as_int(x)
            }
            Some((pack2, b)) => {
                // 16-bit values: one in the low half of each dword, or two
                // per dword (`packed_half`); the host's are 32-bit floats.
                let half = |l: &mut Self, h: u8| -> Result<Option<V>, Why> {
                    let src = if pack2 {
                        packed_half(&l.s.inputs, attribute.row, b, h)
                            .or_else(|| l.half_input_spills.get(&(attribute.row, h)).copied())
                    } else {
                        h.is_multiple_of(2).then_some((attribute.row, h / 2))
                    };
                    match src {
                        Some((row, ch)) => total(l, row, ch).map(Some),
                        None => Ok(None),
                    }
                };
                let to_f16 = |l: &mut Self, f: V| {
                    let i32t = l.s.t.i32;
                    l.s.op(l.cur, "legacyF32ToF16", Attr::ReadNone, i32t, 130, &[f])
                };
                let h = 2 * attribute.channel + u8::from(attribute.high);
                let c = self.cur;
                if name == "v_interp_p1ll_f16" {
                    // An f32 result from 16-bit vertex values.
                    let f = half(self, h)?.ok_or("a packed input half no element holds")?;
                    self.as_int(f)
                } else if f16 {
                    let f = half(self, h)?.ok_or("a packed input half no element holds")?;
                    let lo = to_f16(self, f);
                    self.low_half(inst, reg, lo)
                } else if matches!(eval, Eval::Mode(1)) {
                    // The whole dword of a flat input: both halves.
                    let h = 2 * attribute.channel;
                    let z = self.s.m.i32(0);
                    let lo = match half(self, h)? {
                        Some(f) => to_f16(self, f),
                        None => z,
                    };
                    let hi = match half(self, h + 1)? {
                        Some(f) => to_f16(self, f),
                        None => z,
                    };
                    let k16 = self.s.m.i32(16);
                    let hi = self.s.m.bin(c, Bin::Shl, hi, k16);
                    self.s.m.bin(c, Bin::Or, lo, hi)
                } else {
                    return Err("32-bit interpolation of a 16-bit input");
                }
            }
        };
        // A pixel interpolates in every lane; guards only hide inactive ones.
        self.write(inst, Reg::V(reg), x, None);
        Ok(())
    }

    /// Whether the program reads attribute row `row` per vertex (an
    /// interpolation with computed barycentrics, or P10 or P20), or the row
    /// must be constant for another of its elements: then its elements are
    /// declared constant and read with `attributeAtVertex`.
    fn per_vertex(&self, row: u8) -> bool {
        self.x.io.interps.iter().any(|(_, i)| match *i {
            Interp::Weighted { attribute } => attribute.row == row,
            Interp::Vertex { attribute, vertex } => {
                attribute.row == row && vertex != 2 && self.interpolated(row)
            }
            Interp::Interpolated { .. } => false,
        }) || self.constant_row(row)
    }

    /// Whether row `row` holds an element that must be constant (an
    /// integer, `SV_IsFrontFace`, a provoking-vertex read) next to an
    /// interpolated one. DXIL wants one interpolation mode per packed row,
    /// and the row keeps the console's register (the stages link by it), so
    /// its interpolated elements are weighed from their vertices instead.
    fn constant_row(&self, row: u8) -> bool {
        let interps = || {
            self.x
                .io
                .interps
                .iter()
                .filter(move |(_, i)| match *i {
                    Interp::Interpolated { attribute, .. }
                    | Interp::Vertex { attribute, .. }
                    | Interp::Weighted { attribute } => attribute.row == row,
                })
                .map(|(_, i)| i)
        };
        let interpolated = interps().any(|i| matches!(i, Interp::Interpolated { .. }));
        if !interpolated {
            return false;
        }
        let flat_read = interps().any(|i| matches!(i, Interp::Vertex { vertex: 2, .. }));
        // The signature rows the hardware row carries (a second one for
        // packed 16-bit pairs).
        let rows: Vec<u32> = match &self.x.native.io {
            StageIo::Ps { inputs, .. } => inputs
                .iter()
                .filter(|i| i.row == row && i.fp16)
                .map(|i| u32::from(i.row2))
                .chain([u32::from(row)])
                .collect(),
            _ => vec![u32::from(row)],
        };
        let integer = self
            .x
            .inputs
            .iter()
            .any(|e| e.row.is_some_and(|r| rows.contains(&r)) && e.comp != 9);
        flat_read || integer
    }

    /// Whether the program interpolates attribute row `row` (so the
    /// hardware holds P1 - P0 and P2 - P0 for it).
    fn interpolated(&self, row: u8) -> bool {
        self.x.io.interps.iter().any(|(_, i)| match *i {
            Interp::Interpolated { attribute, .. } | Interp::Weighted { attribute } => {
                attribute.row == row
            }
            Interp::Vertex { .. } => false,
        })
    }

    /// Channel `ch` of input row `row` evaluated as `eval` says, as raw bits.
    fn eval_channel(&mut self, row: u8, ch: u8, eval: Eval) -> Result<V, Why> {
        let (id, col) = self
            .find_input(row, ch)
            .ok_or("an input the signature lacks")?;
        let mode = match eval {
            Eval::Mode(mode) => mode,
            Eval::Weights { .. } => 1,
        };
        let e = &mut self.s.inputs[id];
        if e.interp != mode && e.usage != 0 {
            return Err("one input interpolated two ways");
        }
        e.interp = mode;
        let Eval::Weights { p0, i, j } = eval else {
            return match self.load_any_input(id, col) {
                Val::I32(x) => Ok(x),
                _ => Err("an input"),
            };
        };
        let v0 = self.vertex_input(id, col, 0);
        let c = self.cur;
        let mut acc = p0.then_some(v0);
        for (k, w) in [(1, i), (2, j)] {
            let Some(w) = w else { continue };
            let vk = self.vertex_input(id, col, k);
            let d = self.s.m.bin(c, Bin::Sub, vk, v0);
            acc = Some(match acc {
                Some(a) => self.fmad(w, d, a),
                None => self.s.m.bin(c, Bin::Mul, w, d),
            });
        }
        let acc = acc.ok_or("an interpolation")?;
        Ok(self.as_int(acc))
    }

    /// Vertex `k` (0 the provoking vertex, as `barycentric` orders them)
    /// of a constant input element's channel, as a float.
    fn vertex_input(&mut self, id: usize, col: u8, k: u8) -> V {
        let e = &mut self.s.inputs[id];
        e.usage |= 1 << col;
        let int = e.comp != 9;
        let (ty, suffix) = if int {
            (self.s.t.i32, ".i32")
        } else {
            (self.s.t.f32, ".f32")
        };
        self.s.flags |= flags::BARYCENTRICS;
        let entry = self.s.entry;
        let args = [
            self.s.m.i32(id as u32),
            self.s.m.i32(0),
            self.s.m.i8(col),
            self.s.m.i8(k),
        ];
        let v = self.s.op(
            entry,
            &format!("attributeAtVertex{suffix}"),
            Attr::ReadNone,
            ty,
            137,
            &args,
        );
        if int {
            self.s.m.cast(entry, Cast::Bitcast, v, self.s.t.f32)
        } else {
            v
        }
    }

    /// `x` (an f16 in the low bits) as the low half of VGPR `reg`: 16-bit
    /// results keep the register's high half (GFX10).
    fn low_half(&mut self, inst: &Inst, reg: u8, x: V) -> V {
        let c = self.cur;
        let old = match inst.read(Reg::V(reg)).map(|o| self.i32_of(o)) {
            Some(Ok(o)) => o,
            _ => self.s.m.i32(0),
        };
        let (high, low) = (self.s.m.i32(0xFFFF_0000), self.s.m.i32(0xFFFF));
        let old = self.s.m.bin(c, Bin::And, old, high);
        let x = self.s.m.bin(c, Bin::And, x, low);
        self.s.m.bin(c, Bin::Or, old, x)
    }

    fn image(&mut self, inst: &Inst, b: usize, i: usize) -> Result<(), Why> {
        let Fields::Image {
            data,
            addr,
            dmask,
            dim,
            unorm,
            a16,
            d16,
            tfe,
            lwe,
            ..
        } = &inst.fields
        else {
            unreachable!()
        };
        if *tfe || *lwe {
            return Err("an image access with tfe or lwe");
        }
        let name = inst.name();
        if name.starts_with("image_bvh") {
            // shortcut: rays hit nothing (no node after this one): nothing is
            // in shadow, and indirect light is the sky's.
            let guard = self.guard(b, i)?;
            let Operand::Vector { reg, .. } = *data else {
                return Err("a ray intersection's result");
            };
            if name == "image_bvh64_intersect_ray" && !*a16 && self.x.res.addresses {
                let mut regs = Vec::new();
                for a in addr {
                    if let Operand::Vector { reg, dwords } = *a {
                        regs.extend((0..dwords).map(|k| reg + k));
                    }
                }
                let words = regs
                    .iter()
                    .map(|&r| {
                        let v = inst.read(Reg::V(r)).ok_or("a ray")?;
                        self.i32_of(v)
                    })
                    .collect::<Result<Vec<V>, Why>>()?;
                let words: [V; 12] = words.try_into().map_err(|_| "a ray")?;
                let r = self.intersect(&words, guard);
                for (k, x) in r.into_iter().enumerate() {
                    self.write(inst, Reg::V(reg + k as u8), x, guard);
                }
                return Ok(());
            }
            let none = self.s.m.i32(u32::MAX);
            for k in 0..4 {
                self.write(inst, Reg::V(reg + k), none, guard);
            }
            return Ok(());
        }
        // A load or sample with no channel enabled returns the first.
        let dmask = if *dmask == 0 { &1 } else { dmask };
        let site = (b, i);
        let guard = self.guard(b, i)?;
        let c = self.cur;
        let (kind, coords) = match dim {
            0 => (1, 1),
            1 => (2, 2),
            2 => (4, 3),
            // Sampled (face coordinates and face, see `cube_direction`) or
            // sized only.
            3 if name.starts_with("image_sample")
                || name.starts_with("image_gather")
                || name == "image_get_resinfo" =>
            {
                (5, 3)
            }
            4 => (6, 2),
            5 => (7, 3),
            6 if name == "image_load" || name == "image_get_resinfo" => (3, 2),
            7 if name == "image_load" || name == "image_get_resinfo" => (8, 3),
            _ => return Err("an image dimension (cube loads or MSAA)"),
        };
        // Address VGPRs in order.
        let mut regs = Vec::new();
        for a in addr {
            if let Operand::Vector { reg, dwords } = *a {
                regs.extend((0..dwords).map(|k| reg + k));
            }
        }
        // Full 32-bit address VGPR at `reg_idx` (offset, bias, z-compare).
        let addr_u32 = |l: &mut Self, reg_idx: usize| -> Result<V, Why> {
            let r = *regs.get(reg_idx).ok_or("an image address")?;
            let v = inst.read(Reg::V(r)).ok_or("an image address")?;
            l.i32_of(v)
        };
        // With a16, two 16-bit components per VGPR starting at `base_reg`, low half first.
        let addr_packed = |l: &mut Self, base_reg: usize, k: usize| -> Result<V, Why> {
            let x = addr_u32(l, base_reg + if *a16 { k / 2 } else { k })?;
            if !*a16 {
                return Ok(x);
            }
            let c = l.cur;
            let (sh, mask) = (l.s.m.i32(16 * (k as u32 % 2)), l.s.m.i32(0xFFFF));
            let x = l.s.m.bin(c, Bin::LShr, x, sh);
            Ok(l.s.m.bin(c, Bin::And, x, mask))
        };
        let addr_val = |l: &mut Self, k: usize| -> Result<V, Why> { addr_packed(l, 0, k) };
        let src = self
            .descriptors
            .get(&(site, false))
            .copied()
            .ok_or("an untracked image")?;
        let store = name.starts_with("image_store");
        let uav = store
            || self
                .used_kind(src)
                .is_some_and(|u| u.kind == ResourceType::Uav);
        let props = (kind | if uav { 0x1000 } else { 0 }, 0x409);
        if let Some(op) = name.strip_prefix("image_atomic_") {
            // GLC returns the old value. DXIL implements subtraction by adding
            // the two's complement of the operand at the same width.
            let code = match op {
                "add" | "sub" => 0,
                "and" => 1,
                "or" => 2,
                "xor" => 3,
                "smin" => 4,
                "smax" => 5,
                "umin" => 6,
                "umax" => 7,
                "swap" => 8,
                "cmpswap" => u32::MAX,
                _ => return Err(name),
            };
            let Fields::Image { cache, .. } = &inst.fields else {
                unreachable!()
            };
            if *d16 {
                return Err("a 16-bit image atomic");
            }
            // Compare-exchange consumes a replacement followed by a comparand.
            let compare = code == u32::MAX;
            let wide = match (compare, *dmask) {
                (false, 1) | (true, 3) => false,
                (false, 3) | (true, 15) => true,
                _ => return Err("an image atomic data mask"),
            };
            let iu = self.s.m.undef(self.s.t.i32);
            let mut cs = vec![iu; 3];
            for (k, slot) in cs.iter_mut().enumerate().take(coords) {
                *slot = addr_val(self, k)?;
            }
            let Operand::Vector { reg, .. } = *data else {
                return Err("image atomic data");
            };
            let width = if wide { 2 } else { 1 };
            let vals = self.vgprs(inst, data, if compare { 2 * width } else { width })?;
            let (ty, component) = if wide {
                self.s.flags |= flags::INT64 | flags::ATOMIC_TYPED64 | flags::ATOMIC_HEAP64;
                (self.s.m.int(64), 0x107) // one uint64_t in an R32G32_UINT view
            } else {
                (self.s.t.i32, 0x105)
            };
            let original = if wide {
                self.join64(vals[0], vals[1])
            } else {
                vals[0]
            };
            let value = if op == "sub" {
                let zero = self.s.m.int_const(ty, 0);
                self.s.m.bin(c, Bin::Sub, zero, original)
            } else {
                original
            };
            let h = self.handle(site, src, false, (kind | 0x1000, component))?;
            let mut args = vec![h];
            let (intrinsic, opcode) = if compare {
                let comparand = if wide {
                    self.join64(vals[2], vals[3])
                } else {
                    vals[1]
                };
                args.extend(cs);
                args.extend([comparand, value]);
                let intrinsic = if wide {
                    "atomicCompareExchange.i64"
                } else {
                    "atomicCompareExchange.i32"
                };
                (intrinsic, 79)
            } else {
                args.push(self.s.m.i32(code));
                args.extend(cs);
                args.push(value);
                (
                    if wide {
                        "atomicBinOp.i64"
                    } else {
                        "atomicBinOp.i32"
                    },
                    78,
                )
            };
            let at = self.begin_guarded(guard);
            let r = self
                .s
                .op(self.cur, intrinsic, Attr::NoUnwind, ty, opcode, &args);
            if cache.glc {
                let old = self.end_guarded(at, &[(r, original)])[0];
                if wide {
                    let (lo, hi) = self.split64(old);
                    self.write(inst, Reg::V(reg), lo, None);
                    self.write(inst, Reg::V(reg + 1), hi, None);
                } else {
                    self.write(inst, Reg::V(reg), old, None);
                }
            } else {
                self.end_guarded(at, &[]);
            }
            return Ok(());
        }
        if store {
            if name != "image_store" {
                return Err("an image store to a mip level");
            }
            // shortcut: stores are taken to write all four channels of a
            // float format (the console converts by the view's format). The
            // data VGPRs hold the dmask's channels in order; the others are
            // written as 0.
            let iu = self.s.m.undef(self.s.t.i32);
            let mut cs = vec![iu; 3];
            for (k, slot) in cs.iter_mut().enumerate().take(coords) {
                *slot = addr_val(self, k)?;
            }
            let Operand::Vector { reg, .. } = *data else {
                return Err("image store data");
            };
            let mut vals = Vec::new();
            for ch in 0..4u8 {
                if dmask >> ch & 1 == 0 {
                    vals.push(self.s.m.f32(0));
                    continue;
                }
                let k = (dmask & ((1 << ch) - 1)).count_ones() as u8;
                // d16: two f16 channels per VGPR, low half first.
                let r = if *d16 { k / 2 } else { k };
                let v = inst.read(Reg::V(reg + r)).ok_or("image store data")?;
                vals.push(if *d16 {
                    let mut x = self.i32_of(v)?;
                    if k % 2 == 1 {
                        let k16 = self.s.m.i32(16);
                        x = self.s.m.bin(self.cur, Bin::LShr, x, k16);
                    }
                    let f32t = self.s.t.f32;
                    self.s
                        .op(self.cur, "legacyF16ToF32", Attr::ReadNone, f32t, 131, &[x])
                } else {
                    self.float_of(v)?
                });
            }
            let h = self.handle(site, src, false, props)?;
            let at = self.begin_guarded(guard);
            let mut args = vec![h];
            args.extend(cs);
            args.extend(vals);
            args.push(self.s.m.i8(0xF));
            let void = self.s.t.void;
            self.s.op(
                self.cur,
                "textureStore.f32",
                Attr::NoUnwind,
                void,
                67,
                &args,
            );
            self.end_guarded(at, &[]);
            return Ok(());
        }
        // Samples and gathers: the DXIL operation follows from the form.
        let form = sample_form(name);
        let gather = form.is_some_and(|f| f.gather);
        let (op, sample) = match name {
            "image_get_lod" if self.s.pixel => (81, true),
            "image_load" | "image_load_mip" => (66, false),
            "image_get_resinfo" => (72, false),
            _ if form.is_some() => (0, true),
            _ => return Err(name),
        };
        if op == 72 {
            // Width, height, depth or layers, levels (per dmask) of the level
            // in the first address: DXIL's dimensions; a channel DXIL leaves
            // out reads 1 (UAVs: one level).
            let h = self.handle(site, src, false, props)?;
            let mip = if uav || matches!(kind, 3 | 8) {
                self.s.m.undef(self.s.t.i32)
            } else {
                addr_val(self, 0)?
            };
            let i32t = self.s.t.i32;
            let dims = self.s.m.named_struct("dx.types.Dimensions", vec![i32t; 4]);
            let d = self
                .s
                .op(c, "getDimensions", Attr::ReadOnly, dims, 72, &[h, mip]);
            let one = self.s.m.i32(1);
            let present = match kind {
                1 => [true, false, false, true],
                2 | 3 | 5 | 6 => [true, true, false, true],
                _ => [true, true, true, true],
            };
            let Operand::Vector { reg, .. } = *data else {
                return Err("an image result");
            };
            for (k, ch) in (0..4u32).filter(|ch| dmask >> ch & 1 != 0).enumerate() {
                let x = if ch == 3 && uav || !present[ch as usize] {
                    one
                } else {
                    self.s.m.extract(c, d, ch)
                };
                self.write(inst, Reg::V(reg + k as u8), x, guard);
            }
            return Ok(());
        }
        if uav && op != 66 {
            return Err("sampling a UAV");
        }
        let h = self.handle(site, src, false, props)?;
        let fu = self.s.m.undef(self.s.t.f32);
        let iu = self.s.m.undef(self.s.t.i32);
        let r = if sample {
            let ss = self
                .descriptors
                .get(&(site, true))
                .copied()
                .ok_or("an untracked sampler")?;
            let f = form.unwrap_or_default();
            if f.c && dmask.count_ones() != 1 {
                return Err("a comparison sample of several channels");
            }
            // Comparison: the sampler is a comparison sampler.
            let sh = self.handle(site, ss, true, (14 | if f.c { 0x8000 } else { 0 }, 0))?;
            // Coordinates the size divides (not the array slice).
            let spatial = coords - usize::from(kind == 6 || kind == 7);
            // Address dwords in order (LLVM ImageDimIntrinsics.td, Mesa
            // ac_build_image_opcode): the texel offset, bias and comparison
            // reference, a dword each (an a16 bias is an f16 in the low
            // half, LLVM SIISelLowering::lowerImage); the derivatives, x
            // then y; the coordinates, then the level or level clamp.
            // Derivatives are 16-bit in the g16 opcodes, coordinates and
            // levels with a16 (GFX10.3 keeps the two apart, as LLVM's
            // lowerImage does where hasG16): two per dword, low half first,
            // each direction starting a dword.
            let whole = usize::from(f.o) + usize::from(f.b) + usize::from(f.c);
            let bias_at = (usize::from(f.o), a16.then_some(0));
            // A cube's gradients are those of its two face coordinates.
            let grads = match (f.d, kind) {
                (false, _) => 0,
                (true, 5) => 2,
                (true, _) => spatial,
            };
            let g16 = f.g16;
            let per_direction = if g16 { grads.div_ceil(2) } else { grads };
            let grad_at = |dir: usize, k: usize| {
                if g16 {
                    (whole + dir * per_direction + k / 2, Some(k as u32 % 2))
                } else {
                    (whole + dir * grads + k, None)
                }
            };
            let first = whole + 2 * per_direction;
            // Coordinate k; k = coords is the level or the level clamp.
            let coord_at = |k: usize| {
                if *a16 {
                    (first + k / 2, Some(k as u32 % 2))
                } else {
                    (first + k, None)
                }
            };
            let raw = |l: &mut Self, at: usize| -> Result<V, Why> {
                let r = *regs.get(at).ok_or("an image address")?;
                let v = inst.read(Reg::V(r)).ok_or("an image address")?;
                l.i32_of(v)
            };
            // A float address: a dword, or an f16 half of one.
            let float = |l: &mut Self, (at, half): (usize, Option<u32>)| -> Result<V, Why> {
                let x = raw(l, at)?;
                let Some(h) = half else {
                    return Ok(l.as_float(x));
                };
                let c = l.cur;
                let (sh, mask) = (l.s.m.i32(16 * h), l.s.m.i32(0xFFFF));
                let x = l.s.m.bin(c, Bin::LShr, x, sh);
                let x = l.s.m.bin(c, Bin::And, x, mask);
                let f32t = l.s.t.f32;
                Ok(l.s.op(c, "legacyF16ToF32", Attr::ReadNone, f32t, 131, &[x]))
            };
            // A float address known at translation (an f16 only when 0).
            let known =
                |l: &mut Self, (at, half): (usize, Option<u32>)| -> Result<Option<f32>, Why> {
                    let x = raw(l, at)?;
                    Ok(l.s.m.known(x).and_then(|bits| match half {
                        None => Some(f32::from_bits(bits as u32)),
                        Some(h) => (bits >> (16 * h) & 0x7FFF == 0).then_some(0.0),
                    }))
                };
            let level = if f.l {
                known(self, coord_at(coords))?
            } else {
                f.lz.then_some(0.0)
            };
            let level_zero = level == Some(0.0);
            // A gather at a level not known to be 0 (`image_gather4_l`):
            // DXIL's gathers read level 0 only, so it is emulated below with
            // four loads at that level.
            let gather_at_level = f.gather && f.l && !f.c && !level_zero;
            // The DXIL operation. SM 6.6 has no comparison at a level, with
            // a bias or with gradients (SampleCmpLevel is SM 6.7,
            // SampleCmpBias and SampleCmpGrad 6.8) and gathers level 0 only;
            // those forms are taken where a level or bias known to be 0 makes
            // them one SM 6.6 has.
            let op = if op == 81 {
                81
            } else if f.gather {
                if f.b {
                    return Err("a gather with a level bias");
                }
                if !f.l && !f.lz {
                    return Err("a gather with implicit level");
                }
                if !level_zero && !gather_at_level {
                    return Err("a gather at a level not known to be 0");
                }
                if gather_at_level && kind != 2 {
                    return Err("a gather with explicit level on a non-2D texture");
                }
                if dmask.count_ones() != 1 {
                    return Err("a gather of several channels");
                }
                if !matches!(kind, 2 | 5 | 7) {
                    return Err("a gather from a 1D or 3D image");
                }
                if f.c { 74 } else { 73 }
            } else if f.c {
                if f.d {
                    return Err("a comparison sample with explicit derivatives");
                }
                if f.l && !level_zero {
                    return Err("a comparison sample at a level not known to be 0");
                }
                if f.b && known(self, bias_at)?.is_none_or(|b| b != 0.0) {
                    return Err("a comparison sample with a level bias");
                }
                if f.l || f.lz {
                    65
                } else if self.s.pixel || self.s.threads.is_some() && self.s.mesh.is_none() {
                    // Implicit derivatives: pixel and compute shaders.
                    64
                } else {
                    return Err(
                        "a comparison sample with implicit derivatives outside pixel and compute shaders",
                    );
                }
            } else if f.d {
                63
            } else if f.l || f.lz {
                62
            } else if f.b {
                61
            } else {
                60
            };
            let mut cs = vec![fu; 4];
            for (k, slot) in cs.iter_mut().enumerate().take(coords) {
                *slot = float(self, coord_at(k))?;
            }
            let face = cs[2];
            if kind == 5 {
                if *unorm || op == 81 {
                    return Err("a cube sample with texel coordinates or a level query");
                }
                if f.o {
                    return Err("a texel offset on a cube image");
                }
                let dir = self.cube_direction(cs[0], cs[1], face);
                cs[..3].copy_from_slice(&dir);
            }
            if *unorm {
                // Texel coordinates: divided by the size of level 0.
                let i32t = self.s.t.i32;
                let dims = self.s.m.named_struct("dx.types.Dimensions", vec![i32t; 4]);
                let z = self.s.m.i32(0);
                let d = self
                    .s
                    .op(c, "getDimensions", Attr::ReadOnly, dims, 72, &[h, z]);
                for (k, slot) in cs.iter_mut().enumerate().take(spatial) {
                    let n = self.s.m.extract(c, d, k as u32);
                    let n = self.s.m.cast(c, Cast::UiToFp, n, self.s.t.f32);
                    *slot = self.s.m.bin(c, Bin::SDiv, *slot, n);
                }
            }
            if op == 81 {
                // The clamped then the unclamped level of detail.
                let Operand::Vector { reg, .. } = *data else {
                    return Err("an image result");
                };
                let f32t = self.s.t.f32;
                // The size's dimensions only: an array slice is undef.
                let mut lc = [fu; 3];
                let n = lod_dims(kind);
                lc[..n].copy_from_slice(&cs[..n]);
                for (k, ch) in (0..2u32).filter(|ch| dmask >> ch & 1 != 0).enumerate() {
                    let clamped = self.s.m.bool(ch == 0);
                    let lod = self.s.op(
                        c,
                        "calculateLOD.f32",
                        Attr::ReadOnly,
                        f32t,
                        81,
                        &[h, sh, lc[0], lc[1], lc[2], clamped],
                    );
                    let x = self.as_int(lod);
                    self.write(inst, Reg::V(reg + k as u8), x, guard);
                }
                return Ok(());
            }
            let offsets = if kind == 5 { 0 } else { spatial.min(3) };
            let zero = self.s.m.i32(0);
            let mut os = vec![iu; 3];
            for o in os.iter_mut().take(offsets) {
                *o = zero;
            }
            if f.o {
                // Texel offsets: x in bits 0-5, y in 8-13, z in 16-21, signed.
                let w = raw(self, 0)?;
                let field = |w: i64, k: usize| (w as i32) << (26 - 8 * k) >> 26;
                let immediate = self
                    .s
                    .m
                    .known(w)
                    .filter(|&w| (0..offsets).all(|k| (-8..=7).contains(&field(w, k))));
                if let (Some(w), false) = (immediate, f.gather) {
                    for (k, o) in os.iter_mut().enumerate().take(offsets) {
                        *o = self.s.m.i32(field(w, k) as u32);
                    }
                } else {
                    // Bits at..at+5 sign-extended: shift up, then down.
                    let k26 = self.s.m.i32(26);
                    for (k, o) in os.iter_mut().enumerate().take(offsets) {
                        let left = self.s.m.i32(26 - 8 * k as u32);
                        let t = self.s.m.bin(c, Bin::Shl, w, left);
                        *o = self.s.m.bin(c, Bin::AShr, t, k26);
                    }
                }
                if !f.gather && immediate.is_none() {
                    // A sample's offsets are immediates in -8..7 (gathers
                    // take any, per lane). At a level known here, the
                    // coordinates move by offset / that level's size instead:
                    // the same texels, the coordinate rounded once more (the
                    // console adds the offset to the texel coordinate). A
                    // sampler whose LOD clamp or bias moves the sampled
                    // level away from the one given, or a level past the
                    // view's last, differs.
                    let Some(n) = level.filter(|n| *n >= 0.0 && n.fract() == 0.0) else {
                        return Err(
                            "a texel offset past -8..7 or per lane at a level not known here",
                        );
                    };
                    let i32t = self.s.t.i32;
                    let f32t = self.s.t.f32;
                    let dims = self.s.m.named_struct("dx.types.Dimensions", vec![i32t; 4]);
                    let n = self.s.m.i32(n as u32);
                    let d = self
                        .s
                        .op(c, "getDimensions", Attr::ReadOnly, dims, 72, &[h, n]);
                    for (k, o) in os.iter_mut().enumerate().take(offsets) {
                        let size = self.s.m.extract(c, d, k as u32);
                        let size = self.s.m.cast(c, Cast::UiToFp, size, f32t);
                        let texels = self.s.m.cast(c, Cast::SiToFp, *o, f32t);
                        let shift = self.s.m.bin(c, Bin::SDiv, texels, size);
                        cs[k] = self.s.m.bin(c, Bin::Add, cs[k], shift);
                        *o = zero;
                    }
                }
            }
            // The level clamp, after the coordinates.
            let clamp = if f.cl {
                float(self, coord_at(coords))?
            } else {
                fu
            };
            let mut args = vec![h, sh];
            args.extend(&cs);
            match op {
                60 | 61 if !self.s.pixel => {
                    // Outside pixel shaders the console still takes the level
                    // from each quad of lanes (lane 1 minus lane 0 across,
                    // lane 2 minus lane 0 down); DXIL before validator 1.8
                    // has no implicit derivatives there, so the quad's
                    // differences become explicit gradients. A bias scales
                    // them by 2^bias, the same level for isotropic filtering.
                    self.cross_lane(true)?;
                    let scale = if op == 61 {
                        let bias = float(self, bias_at)?;
                        Some(self.unary(21, bias))
                    } else {
                        None
                    };
                    let g = self.quad_gradients(&cs[..spatial], scale);
                    args.extend(&os);
                    args.extend(g);
                    args.push(clamp);
                    self.s.op(
                        c,
                        "sampleGrad.f32",
                        Attr::ReadOnly,
                        self.s.t.res_ret,
                        63,
                        &args,
                    )
                }
                60 => {
                    args.extend(&os);
                    args.push(clamp);
                    self.s
                        .op(c, "sample.f32", Attr::ReadOnly, self.s.t.res_ret, 60, &args)
                }
                61 => {
                    let bias = float(self, bias_at)?;
                    args.extend(&os);
                    args.extend([bias, clamp]);
                    self.s.op(
                        c,
                        "sampleBias.f32",
                        Attr::ReadOnly,
                        self.s.t.res_ret,
                        61,
                        &args,
                    )
                }
                63 => {
                    // shortcut: the coarse forms (`_cd`) compute one level
                    // per quad; the host's per pixel is the same when the
                    // gradients agree across the quad.
                    let mut g = vec![fu; 6];
                    for k in 0..grads {
                        g[k] = float(self, grad_at(0, k))?;
                        g[3 + k] = float(self, grad_at(1, k))?;
                    }
                    if kind == 5 {
                        // The host takes the gradients of the direction.
                        let x = self.cube_gradient(g[0], g[1], face);
                        let y = self.cube_gradient(g[3], g[4], face);
                        g[..3].copy_from_slice(&x);
                        g[3..].copy_from_slice(&y);
                    }
                    args.extend(&os);
                    args.extend(g);
                    args.push(clamp);
                    self.s.op(
                        c,
                        "sampleGrad.f32",
                        Attr::ReadOnly,
                        self.s.t.res_ret,
                        63,
                        &args,
                    )
                }
                64 | 65 => {
                    let at = usize::from(f.o) + usize::from(f.b);
                    let reference = float(self, (at, None))?;
                    args.extend(&os);
                    args.push(reference);
                    let name = if op == 64 {
                        args.push(clamp);
                        "sampleCmp.f32"
                    } else {
                        "sampleCmpLevelZero.f32"
                    };
                    self.s
                        .op(c, name, Attr::ReadOnly, self.s.t.res_ret, op, &args)
                }
                62 => {
                    let lod = if f.l {
                        float(self, coord_at(coords))?
                    } else {
                        self.s.m.f32(0)
                    };
                    args.extend(&os);
                    args.push(lod);
                    self.s.op(
                        c,
                        "sampleLevel.f32",
                        Attr::ReadOnly,
                        self.s.t.res_ret,
                        62,
                        &args,
                    )
                }
                _ => {
                    if gather_at_level {
                        // Console-only Gather at an explicit mip level: DXIL's
                        // textureGather has no LOD argument, so emulate with
                        // four clamped textureLoad calls at that mip in HLSL
                        // gather order: (-,+), (+,+), (+,-), (-,-).
                        let lod = float(self, coord_at(coords))?;
                        let i32t = self.s.t.i32;
                        let f32t = self.s.t.f32;
                        let dims = self.s.m.named_struct("dx.types.Dimensions", vec![i32t; 4]);
                        let d0 = self
                            .s
                            .op(c, "getDimensions", Attr::ReadOnly, dims, 72, &[h, zero]);
                        let levels = self.s.m.extract(c, d0, 3);
                        let one = self.s.m.i32(1);
                        let max_mip = self.s.m.bin(c, Bin::Sub, levels, one);
                        let fzero = self.s.m.f32(0);
                        let lod = self.binary(35, lod, fzero);
                        let mip = self.s.m.cast(c, Cast::FpToUi, lod, i32t);
                        let over = self.s.m.cmp(c, pred::IUGT, mip, max_mip);
                        let mip = self.s.m.select(c, over, max_mip, mip);
                        let dm = self
                            .s
                            .op(c, "getDimensions", Attr::ReadOnly, dims, 72, &[h, mip]);
                        let w = self.s.m.extract(c, dm, 0);
                        let ht = self.s.m.extract(c, dm, 1);
                        let half = self.s.m.f32(0.5f32.to_bits());
                        let clamp_pair = |l: &mut Self, coord: V, dim: V, off: V| {
                            let df = l.s.m.cast(c, Cast::UiToFp, dim, f32t);
                            let p = l.s.m.bin(c, Bin::Mul, coord, df);
                            let p = l.s.m.bin(c, Bin::Sub, p, half);
                            let fl = l.s.op(c, "unary.f32", Attr::ReadNone, f32t, 27, &[p]);
                            let t0 = l.s.m.cast(c, Cast::FpToSi, fl, i32t);
                            let t0 = l.s.m.bin(c, Bin::Add, t0, off);
                            let t1 = l.s.m.bin(c, Bin::Add, t0, one);
                            let hi = l.s.m.bin(c, Bin::Sub, dim, one);
                            let mut cl = |x: V| {
                                let lo_c = l.s.m.cmp(c, pred::SLT, x, zero);
                                let x = l.s.m.select(c, lo_c, zero, x);
                                let hi_c = l.s.m.cmp(c, pred::SGT, x, hi);
                                l.s.m.select(c, hi_c, hi, x)
                            };
                            (cl(t0), cl(t1))
                        };
                        let (x0, x1) = clamp_pair(self, cs[0], w, os[0]);
                        let (y0, y1) = clamp_pair(self, cs[1], ht, os[1]);
                        let ch = dmask.trailing_zeros();
                        let Operand::Vector { reg, .. } = *data else {
                            return Err("an image result");
                        };
                        let mut xs = Vec::with_capacity(4);
                        for (xi, yi) in [(x0, y1), (x1, y1), (x1, y0), (x0, y0)] {
                            let tl = self.s.op(
                                c,
                                "textureLoad.f32",
                                Attr::ReadOnly,
                                self.s.t.res_ret,
                                66,
                                &[h, mip, xi, yi, iu, iu, iu, iu],
                            );
                            let x = self.s.m.extract(c, tl, ch);
                            xs.push(if *d16 {
                                self.s
                                    .op(c, "legacyF32ToF16", Attr::ReadNone, i32t, 130, &[x])
                            } else {
                                self.as_int(x)
                            });
                        }
                        if *d16 {
                            let k16 = self.s.m.i32(16);
                            xs = xs
                                .chunks(2)
                                .map(|pair| {
                                    let lo = pair[0];
                                    match pair.get(1) {
                                        Some(&hi) => {
                                            let hi = self.s.m.bin(c, Bin::Shl, hi, k16);
                                            self.s.m.bin(c, Bin::Or, lo, hi)
                                        }
                                        None => lo,
                                    }
                                })
                                .collect();
                        }
                        for (k, x) in xs.into_iter().enumerate() {
                            self.write(inst, Reg::V(reg + k as u8), x, guard);
                        }
                        return Ok(());
                    }
                    args.extend(&os[..2]);
                    args.push(self.s.m.i32(dmask.trailing_zeros()));
                    let name = if op == 74 {
                        let reference = float(self, (usize::from(f.o), None))?;
                        args.push(reference);
                        "textureGatherCmp.f32"
                    } else {
                        "textureGather.f32"
                    };
                    self.s
                        .op(c, name, Attr::ReadOnly, self.s.t.res_ret, op, &args)
                }
            }
        } else {
            let mut cs = vec![iu; 3];
            for (k, slot) in cs.iter_mut().enumerate().take(coords) {
                *slot = addr_val(self, k)?;
            }
            let mip = if name == "image_load_mip" || matches!(kind, 3 | 8) {
                addr_val(self, coords)?
            } else if uav {
                // Our declarations have four channels.
                self.s.flags |= flags::TYPED_UAV_LOADS;
                iu
            } else {
                self.s.m.i32(0)
            };
            let mut args = vec![h, mip];
            args.extend(&cs);
            args.extend([iu, iu, iu]);
            self.s.op(
                c,
                "textureLoad.f32",
                Attr::ReadOnly,
                self.s.t.res_ret,
                66,
                &args,
            )
        };
        let Operand::Vector { reg, .. } = *data else {
            return Err("an image result");
        };
        let channels: Vec<u32> = if gather {
            vec![0, 1, 2, 3]
        } else {
            (0..4).filter(|ch| dmask >> ch & 1 != 0).collect()
        };
        let mut xs = Vec::new();
        for ch in channels {
            let x = self.s.m.extract(c, r, ch);
            xs.push(if *d16 {
                // shortcut: d16 results are taken as floats (rounded to
                // nearest even); integer formats would keep 16 bits.
                let i32t = self.s.t.i32;
                self.s
                    .op(c, "legacyF32ToF16", Attr::ReadNone, i32t, 130, &[x])
            } else {
                self.as_int(x)
            });
        }
        if *d16 {
            // Two channels per VGPR, low half first (an odd last high half 0).
            let k16 = self.s.m.i32(16);
            xs = xs
                .chunks(2)
                .map(|p| match *p {
                    [lo, hi] => {
                        let hi = self.s.m.bin(c, Bin::Shl, hi, k16);
                        self.s.m.bin(c, Bin::Or, lo, hi)
                    }
                    _ => p[0],
                })
                .collect();
        }
        for (k, x) in xs.into_iter().enumerate() {
            self.write(inst, Reg::V(reg + k as u8), x, guard);
        }
        Ok(())
    }

    /// The direction a cube sample's face coordinates stand for: the
    /// console takes s and t in [1, 2] (`v_cubesc`/`v_cubetc` over
    /// `|v_cubema|`, plus 1.5) and the face id (`v_cubeid`), which the
    /// host's cube sampling turns back into the same texel.
    // shortcut: the view is taken as a cube, not a cube array (whose face
    // operand adds 8 × the slice).
    fn cube_direction(&mut self, s: V, t: V, face: V) -> [V; 3] {
        let c = self.cur;
        let k = |l: &mut Self, v: f32| l.s.m.f32(v.to_bits());
        let (two, three) = (k(self, 2.0), k(self, 3.0));
        let u = self.s.m.bin(c, Bin::Mul, s, two);
        let u = self.s.m.bin(c, Bin::Sub, u, three);
        let v = self.s.m.bin(c, Bin::Mul, t, two);
        let v = self.s.m.bin(c, Bin::Sub, v, three);
        let z = self.s.m.f32(0x8000_0000);
        let (nu, nv) = (
            self.s.m.bin(c, Bin::Sub, z, u),
            self.s.m.bin(c, Bin::Sub, z, v),
        );
        let (one, mone) = (k(self, 1.0), k(self, -1.0));
        let f = self.s.m.cast(c, Cast::FpToUi, face, self.s.t.i32);
        // Per face +x, -x, +y, -y, +z, -z.
        self.cube_face(
            f,
            [
                [one, mone, u, u, u, nu],
                [nv, nv, one, mone, nv, nv],
                [nu, u, v, nv, one, mone],
            ],
        )
    }

    /// The gradient of [`Self::cube_direction`] for the gradients `ds`, `dt`
    /// of the face coordinates: the direction is linear in them (twice
    /// each, signed per face) and constant along the face's own axis.
    fn cube_gradient(&mut self, ds: V, dt: V, face: V) -> [V; 3] {
        let c = self.cur;
        let two = self.s.m.f32(2.0f32.to_bits());
        let u = self.s.m.bin(c, Bin::Mul, ds, two);
        let v = self.s.m.bin(c, Bin::Mul, dt, two);
        let z = self.s.m.f32(0x8000_0000);
        let (nu, nv) = (
            self.s.m.bin(c, Bin::Sub, z, u),
            self.s.m.bin(c, Bin::Sub, z, v),
        );
        let o = self.s.m.f32(0);
        let f = self.s.m.cast(c, Cast::FpToUi, face, self.s.t.i32);
        self.cube_face(
            f,
            [
                [o, o, u, u, u, nu],
                [nv, nv, o, o, nv, nv],
                [nu, u, v, nv, o, o],
            ],
        )
    }

    /// Each row's entry for the face (an integer, +x -x +y -y +z -z).
    fn cube_face(&mut self, face: V, table: [[V; 6]; 3]) -> [V; 3] {
        let c = self.cur;
        table.map(|row| {
            let mut r = row[0];
            for (n, &x) in row.iter().enumerate().skip(1) {
                let kn = self.s.m.i32(n as u32);
                let is = self.s.m.cmp(c, pred::EQ, face, kn);
                r = self.s.m.select(c, is, x, r);
            }
            r
        })
    }

    fn raw_dword(&mut self, h: V, coords: [V; 2]) -> V {
        let c = self.cur;
        let (mask, align) = (self.s.m.i8(1), self.s.m.i32(4));
        let r = self.s.op(c, "rawBufferLoad.f32", Attr::ReadOnly, self.s.t.res_ret, 139,
            &[h, coords[0], coords[1], mask, align]);
        let x = self.s.m.extract(c, r, 0);
        self.as_int(x)
    }

    fn buffer_compare_exchange(&mut self, h: V, coords: [V; 3], old: V, new: V, wide: bool) -> V {
        let name = if wide { "atomicCompareExchange.i64" } else { "atomicCompareExchange.i32" };
        let ty = self.s.m.type_of(old);
        self.s.op(self.cur, name, Attr::NoUnwind, ty, 79,
            &[h, coords[0], coords[1], coords[2], old, new])
    }

    /// A CAS must run on a real lane and at an in-range address: outside
    /// those cases DXIL can return an unspecified old value.
    fn buffer_bounds(&mut self, h: V, index: Option<V>, off: V, stride: u32, size: u32) -> V {
        let c = self.cur;
        let i32t = self.s.t.i32;
        let dims = self.s.m.named_struct("dx.types.Dimensions", vec![i32t; 4]);
        let unused = self.s.m.undef(i32t);
        let dimensions = self.s.op(c, "getDimensions", Attr::ReadOnly, dims, 72, &[h, unused]);
        let length = self.s.m.extract(c, dimensions, 0);
        if let Some(index) = index {
            let record = self.s.m.cmp(c, pred::IULT, index, length);
            let limit = self.s.m.i32(stride - (size - 1));
            let member = self.s.m.cmp(c, pred::IULT, off, limit);
            return self.s.m.bin(c, Bin::And, record, member);
        }
        let tail = self.s.m.i32(size - 1);
        let last = self.s.m.bin(c, Bin::Add, off, tail);
        let fits = self.s.m.cmp(c, pred::IULT, last, length);
        let no_wrap = self.s.m.cmp(c, pred::IUGE, last, off);
        self.s.m.bin(c, Bin::And, fits, no_wrap)
    }

    fn buffer_store_guard(&mut self, bounds: V, guard: Option<V>) -> V {
        let c = self.cur;
        let g = guard.map_or(bounds, |g| self.s.m.bin(c, Bin::And, g, bounds));
        if !self.s.pixel {
            return g;
        }
        let helper = self.s.op(c, "isHelperLane.i1", Attr::ReadOnly, self.s.t.i1, 221, &[]);
        let no = self.s.m.bool(false);
        self.s.m.select(c, helper, no, g)
    }

    fn buffer(&mut self, inst: &Inst, b: usize, i: usize) -> Result<(), Why> {
        let Fields::Buffer {
            data,
            addr,
            soffset,
            offset,
            offen,
            idxen,
            format,
            lds,
            tfe,
            ..
        } = &inst.fields
        else {
            unreachable!()
        };
        let name = inst.name();
        if *lds || *tfe {
            return Err("a buffer access to LDS or with tfe");
        }
        if format.is_some() && !name.starts_with("tbuffer_load_format_") {
            return Err("an MTBUF store requiring a packed-format write");
        }
        if name.contains("_format_d16_hi_") {
            return Err("a typed buffer access to the high half with an untracked numeric format");
        }
        let site = (b, i);
        let guard = self.guard(b, i)?;
        let c = self.cur;
        let store = name.starts_with("buffer_store_");
        let atomic = name.strip_prefix("buffer_atomic_");
        let wide = atomic.is_some_and(|n| n.ends_with("_x2"));
        let atomic = atomic.map(|n| n.strip_suffix("_x2").unwrap_or(n));
        // DXIL atomic operations: add, and, or, xor, signed and unsigned
        // min and max, exchange; compare-exchange apart.
        let native_atomic = match atomic {
            None => None,
            Some("add" | "sub") => Some(0),
            Some("and") => Some(1),
            Some("or") => Some(2),
            Some("xor") => Some(3),
            Some("smin") => Some(4),
            Some("smax") => Some(5),
            Some("umin") => Some(6),
            Some("umax") => Some(7),
            Some("swap") => Some(8),
            Some("cmpswap" | "inc" | "dec" | "csub") => None,
            Some("fcmpswap" | "fmin" | "fmax") => {
                // Mesa aco_insert_fp_mode.cpp, vmem_default_needs.
                return Err("a buffer floating-point atomic requiring the console's denormal mode");
            }
            Some(_) => return Err(name),
        };
        let sub_load = name.strip_prefix("buffer_load_").and_then(subword_load);
        let sub_store = match name {
            "buffer_store_byte" | "buffer_store_byte_d16_hi" => Some(8),
            "buffer_store_short" | "buffer_store_short_d16_hi" => Some(16),
            _ => None,
        };
        // d16: two 16-bit channels per VGPR, low half first (buffer and
        // tbuffer formats alike).
        let d16 = name.contains("_format_d16_");
        let plain = name.replace("_format_d16_", "_format_");
        let (typed, count) = match plain
            .strip_prefix("buffer_load_")
            .or(plain.strip_prefix("buffer_store_"))
            .or(plain.strip_prefix("tbuffer_load_"))
        {
            _ if atomic.is_some() || sub_load.is_some() || sub_store.is_some() => (false, 1),
            Some("dword") => (false, 1),
            Some("dwordx2") => (false, 2),
            Some("dwordx3") => (false, 3),
            Some("dwordx4") => (false, 4),
            Some("format_x") => (true, 1),
            Some("format_xy") => (true, 2),
            Some("format_xyz") => (true, 3),
            Some("format_xyzw") => (true, 4),
            _ => return Err(name),
        };
        let Operand::Vector { reg: areg, .. } = *addr else {
            return Err("a buffer address");
        };
        let vaddr = |l: &mut Self, k: u8| -> Result<V, Why> {
            let v = inst.read(Reg::V(areg + k)).ok_or("a buffer address")?;
            l.i32_of(v)
        };
        let index = if *idxen { Some(vaddr(self, 0)?) } else { None };
        let mut off = self.s.m.i32(u32::from(*offset));
        if *offen {
            let o = vaddr(self, u8::from(*idxen))?;
            off = self.s.m.bin(c, Bin::Add, off, o);
        }
        if !matches!(
            soffset,
            Operand::Scalar { code: NULL, .. } | Operand::Const(128)
        ) {
            let so = self.operand(inst, soffset)?;
            off = self.s.m.bin(c, Bin::Add, off, so);
        }
        let src = self
            .descriptors
            .get(&(site, false))
            .copied()
            .ok_or("an untracked buffer")?;
        let used = self.used_kind(src);
        let stride = used.map_or(0, |u| u.stride);
        if self.by_address(src)
            && let Fields::Buffer { rsrc, .. } = &inst.fields
        {
            if typed && format.is_none() {
                return Err("a typed buffer access by address");
            }
            let (h, start, stride) = self.made_buffer(inst, rsrc)?;
            self.made.insert(site, h);
            off = self.s.m.bin(c, Bin::Add, off, start);
            if let Some(i) = index {
                let rows = self.s.m.bin(c, Bin::Mul, i, stride);
                off = self.s.m.bin(c, Bin::Add, off, rows);
            }
        }
        let index = if self.by_address(src) { None } else { index };
        let (base, extra) = if self.by_address(src) {
            (0, None)
        } else {
            self.built_offset(src)?
        };
        if base != 0 {
            let b = self.s.m.i32(base);
            off = self.s.m.bin(c, Bin::Add, off, b);
        }
        if let Some(x) = extra {
            off = self.s.m.bin(c, Bin::Add, off, x);
        }
        if used.is_some_and(|u| u.kind == ResourceType::Cbv) && !typed {
            if store || atomic.is_some() {
                return Err("a write or atomic operation on a constant buffer");
            }
            // A constant buffer read per lane (a dynamically indexed array).
            // shortcut: the console's CBV descriptor is taken to have a 16-byte
            // stride, as the vector loads of whole rows suggest.
            let h = self.handle(site, src, false, (13, 0x1_0000))?;
            let byte = match index {
                Some(i) => {
                    let sixteen = self.s.m.i32(16);
                    let rows = self.s.m.bin(c, Bin::Mul, i, sixteen);
                    self.s.m.bin(c, Bin::Add, rows, off)
                }
                None => off,
            };
            let Some(Operand::Vector { reg, .. }) = *data else {
                return Err("a buffer result");
            };
            if let Some((width, signed, half)) = sub_load {
                let x = self.read_subword(byte, width, signed, |l, at| Ok(l.cbuffer_dword(h, at)))?;
                self.write_subword_result(inst, reg, x, half, guard)?;
                return Ok(());
            }
            for k in 0..count {
                let four = self.s.m.i32(4 * k);
                let a = self.s.m.bin(c, Bin::Add, byte, four);
                let x = self.cbuffer_dword(h, a);
                self.write(inst, Reg::V(reg + k as u8), x, guard);
            }
            return Ok(());
        }
        // A dword access by index through a descriptor built in code: the
        // stride is the code's own, so the element becomes a byte offset in
        // the raw root view.
        let (index, off) = match index {
            Some(i) if !typed && format.is_none() => match self.built_element(src, i, off)? {
                Some(byte) => (None, byte),
                // Structured buffers of 16-byte multiples are raw views of
                // their bytes on the host: D3D12 takes structured views only
                // on multiples of the stride, and AMD reads element 0 for all
                // of a raw view's. (Others find a start they fit, and raw
                // views are to start on 16 bytes.)
                None if stride >= 16 && stride.is_multiple_of(16) => {
                    let st = self.s.m.i32(stride);
                    let first = self.s.m.bin(c, Bin::Mul, i, st);
                    (None, self.s.m.bin(c, Bin::Add, first, off))
                }
                None => (Some(i), off),
            },
            x => (x, off),
        };
        let uav = store || atomic.is_some() || used.is_some_and(|u| u.kind == ResourceType::Uav);
        let u = if uav { 0x1000 } else { 0 };
        let iu = self.s.m.undef(self.s.t.i32);
        if sub_load.is_some() || sub_store.is_some() {
            let data = data.as_ref().ok_or("buffer sub-dword data")?;
            let Operand::Vector { reg, .. } = *data else {
                return Err("buffer sub-dword data");
            };
            let h = if index.is_some() {
                if stride < 4 || !stride.is_multiple_of(4) {
                    return Err("a structured sub-dword buffer access without a dword-aligned stride");
                }
                self.handle(site, src, false, (12 | u, stride))?
            } else {
                self.handle(site, src, false, (11 | u, 0))?
            };
            let coords = |byte| index.map_or([byte, iu], |i| [i, byte]);
            if let Some((width, signed, half)) = sub_load {
                // Root SRVs have no size in DXIL.
                let bounds = (!matches!(src, Source::Built { .. } | Source::Address))
                    .then(|| self.buffer_bounds(h, index, off, stride, width / 8));
                let at = self.begin_guarded(bounds);
                let x = self.read_subword(off, width, signed, |l, at| Ok(l.raw_dword(h, coords(at))))?;
                let zero = self.s.m.i32(0);
                let x = self.end_guarded(at, &[(x, zero)])[0];
                self.write_subword_result(inst, reg, x, half, guard)?;
            } else if let Some(width) = sub_store {
                let mut x = self.vgprs(inst, data, 1)?[0];
                if name.ends_with("_hi") {
                    let sixteen = self.s.m.i32(16);
                    x = self.s.m.bin(c, Bin::LShr, x, sixteen);
                }
                let bounds = self.buffer_bounds(h, index, off, stride, width / 8);
                let active = self.buffer_store_guard(bounds, guard);
                let at = self.begin_guarded(Some(active));
                self.write_subword(off, width, x, |l, byte, mask, bits| {
                    let [a, b] = coords(byte);
                    let zero = l.s.m.i32(0);
                    l.atomic_update(zero, |l, old| {
                        let x = l.atomic_new_value(AtomicUpdate::MaskOr, old, mask, bits);
                        l.buffer_compare_exchange(h, [a, b, iu], old, x, false)
                    });
                });
                self.end_guarded(at, &[]);
            }
            return Ok(());
        }
        if let Some(op) = atomic {
            let returns = matches!(inst.fields, Fields::Buffer { cache, .. } if cache.glc);
            if op == "csub" && !returns {
                // LLVM BUFInstructions.td only selects the returning form
                // on GFX10.3.
                return Err("buffer_atomic_csub without the required GLC bit");
            }
            let d = data.as_ref().ok_or("buffer atomic data")?;
            let words = if wide { 2 } else { 1 };
            let vals = self.vgprs(inst, d, if op == "cmpswap" { 2 * words } else { words })?;
            let val = if wide { self.join64(vals[0], vals[1]) } else { vals[0] };
            let ty = self.s.m.type_of(val);
            let zero = self.s.m.int_const(ty, 0);
            let (h, coords) = match index {
                Some(index) => {
                    if stride >= 4 && stride.is_multiple_of(4) {
                        if wide && !stride.is_multiple_of(8) {
                            return Err("a 64-bit buffer atomic without an eight-byte-aligned stride");
                        }
                        let h = self.handle(site, src, false, (12 | u, stride))?;
                        (h, [index, off, iu])
                    } else {
                        if wide {
                            return Err("a 64-bit buffer atomic through a structured buffer of unknown stride");
                        }
                        let byte = self.runtime_stride_offset(src, index, off)?;
                        (self.handle(site, src, false, (11 | u, 0))?, [byte, iu, iu])
                    }
                }
                None => (self.handle(site, src, false, (11 | u, 0))?, [off, iu, iu]),
            };
            let Operand::Vector { reg, .. } = *d else {
                return Err("buffer atomic data");
            };
            if wide {
                self.s.flags |= flags::INT64 | flags::ATOMIC_HEAP64;
            }
            let active = if wide || matches!(op, "sub" | "inc" | "dec" | "csub") {
                let bounds = self.buffer_bounds(h, index, off, stride, 4 * u32::from(words));
                Some(self.buffer_store_guard(bounds, guard))
            } else {
                guard
            };
            let at = self.begin_guarded(active);
            let r = if op == "cmpswap" {
                // cmpswap: data is (new, compare).
                let compare = if wide { self.join64(vals[2], vals[3]) } else { vals[1] };
                self.buffer_compare_exchange(h, coords, compare, val, wide)
            } else if let Some(code) = native_atomic {
                let name = if wide { "atomicBinOp.i64" } else { "atomicBinOp.i32" };
                let code = self.s.m.i32(code);
                let data = if op == "sub" { self.s.m.bin(self.cur, Bin::Sub, zero, val) } else { val };
                self.s.op(self.cur, name, Attr::NoUnwind, ty, 78,
                    &[h, code, coords[0], coords[1], coords[2], data])
            } else {
                let update = match op {
                    "inc" => AtomicUpdate::Inc,
                    "dec" => AtomicUpdate::Dec,
                    "csub" => AtomicUpdate::ClampSub,
                    _ => unreachable!(),
                };
                self.atomic_update(zero, |l, old| {
                    let x = l.atomic_new_value(update, old, val, zero);
                    l.buffer_compare_exchange(h, coords, old, x, wide)
                })
            };
            if returns {
                let r = self.end_guarded(at, &[(r, zero)])[0];
                self.write_atomic_result(inst, reg, r, wide, guard);
            } else {
                self.end_guarded(at, &[]);
            }
            return Ok(());
        }
        if store {
            let regs = if d16 { count.div_ceil(2) } else { count };
            let mut vals =
                self.vgprs(inst, data.as_ref().ok_or("buffer store data")?, regs as u8)?;
            if d16 {
                // shortcut: d16 data is taken as f16 (integer formats would
                // keep 16 bits), widened for the 32-bit float store.
                let mut wide = Vec::new();
                for k in 0..count {
                    let mut x = vals[(k / 2) as usize];
                    if k % 2 == 1 {
                        let k16 = self.s.m.i32(16);
                        x = self.s.m.bin(c, Bin::LShr, x, k16);
                    }
                    let f32t = self.s.t.f32;
                    let f = self
                        .s
                        .op(c, "legacyF16ToF32", Attr::ReadNone, f32t, 131, &[x]);
                    wide.push(self.as_int(f));
                }
                vals = wide;
            }
            let mut args = Vec::new();
            let (op, name) = if typed {
                // shortcut: typed stores are taken to write a float format.
                // The channels the opcode leaves out are written as 0: the
                // validator has typed stores write all four.
                if *offen || *offset != 0 || matches!(off, V::Inst(_)) {
                    return Err("a typed buffer store at an offset");
                }
                args.push(self.handle(site, src, false, (10 | u, 0x409))?);
                args.extend([index.unwrap_or(self.s.m.i32(0)), iu]);
                for x in vals {
                    args.push(self.as_float(x));
                }
                args.resize(3 + 4, self.s.m.f32(0));
                (69, "bufferStore.f32")
            } else {
                match index {
                    Some(index) => {
                        let stride = used.map_or(0, |u| u.stride);
                        if stride >= 4 && stride.is_multiple_of(4) {
                            args.push(self.handle(site, src, false, (12 | u, stride))?);
                            args.extend([index, off]);
                        } else {
                            let byte = self.runtime_stride_offset(src, index, off)?;
                            args.push(self.handle(site, src, false, (11 | u, 0))?);
                            args.extend([byte, iu]);
                        }
                    }
                    None => {
                        args.push(self.handle(site, src, false, (11 | u, 0))?);
                        args.extend([off, iu]);
                    }
                }
                args.extend(&vals);
                args.resize(3 + 4, iu);
                (140, "rawBufferStore.i32")
            };
            args.push(self.s.m.i8(if typed { 0xF } else { (1u8 << count) - 1 }));
            if !typed {
                args.push(self.s.m.i32(4));
            }
            let at = self.begin_guarded(guard);
            let void = self.s.t.void;
            self.s.op(self.cur, name, Attr::NoUnwind, void, op, &args);
            self.end_guarded(at, &[]);
            return Ok(());
        }
        if let Some(f) = *format {
            if index.is_some() {
                return Err("an indexed MTBUF load requiring a raw view of a structured descriptor");
            }
            let h = self.handle(site, src, false, (11 | u, 0))?;
            let Some(Operand::Vector { reg, .. }) = *data else {
                return Err("a buffer result");
            };
            let mut xs = self.format_load(h, off, f, count)?;
            if d16 {
                xs = self.d16_channels(f, xs)?;
            }
            for (k, x) in xs.into_iter().enumerate() {
                self.write(inst, Reg::V(reg + k as u8), x, guard);
            }
            return Ok(());
        }
        let r = if typed {
            if *offen || matches!(off, V::Inst(_)) || *offset != 0 {
                return Err("a typed buffer load at an offset");
            }
            if uav {
                self.s.flags |= flags::TYPED_UAV_LOADS;
            }
            let h = self.handle(site, src, false, (10 | u, 0x409))?;
            let index = index.unwrap_or(self.s.m.i32(0));
            self.s.op(
                c,
                "bufferLoad.f32",
                Attr::ReadOnly,
                self.s.t.res_ret,
                68,
                &[h, index, iu],
            )
        } else {
            let mask = self.s.m.i8((1u8 << count) - 1);
            let align = self.s.m.i32(4);
            match index {
                Some(index) => {
                    let stride = used.map_or(0, |u| u.stride);
                    if stride >= 4 && stride.is_multiple_of(4) {
                        let h = self.handle(site, src, false, (12 | u, stride))?;
                        self.s.op(
                            c,
                            "rawBufferLoad.f32",
                            Attr::ReadOnly,
                            self.s.t.res_ret,
                            139,
                            &[h, index, off, mask, align],
                        )
                    } else {
                        let byte = self.runtime_stride_offset(src, index, off)?;
                        let h = self.handle(site, src, false, (11 | u, 0))?;
                        self.s.op(
                            c,
                            "rawBufferLoad.f32",
                            Attr::ReadOnly,
                            self.s.t.res_ret,
                            139,
                            &[h, byte, iu, mask, align],
                        )
                    }
                }
                None => {
                    let h = self.handle(site, src, false, (11 | u, 0))?;
                    self.s.op(
                        c,
                        "rawBufferLoad.f32",
                        Attr::ReadOnly,
                        self.s.t.res_ret,
                        139,
                        &[h, off, iu, mask, align],
                    )
                }
            }
        };
        let Some(Operand::Vector { reg, .. }) = *data else {
            return Err("a buffer result");
        };
        if d16 {
            // Two f16 channels per VGPR (an odd last high half 0).
            let k16 = self.s.m.i32(16);
            let i32t = self.s.t.i32;
            for k in 0..count.div_ceil(2) {
                let mut x = None;
                for ch in (2 * k..(2 * k + 2).min(count)).rev() {
                    let f = self.s.m.extract(c, r, ch);
                    let h = self
                        .s
                        .op(c, "legacyF32ToF16", Attr::ReadNone, i32t, 130, &[f]);
                    x = Some(match x {
                        Some(hi) => {
                            let hi = self.s.m.bin(c, Bin::Shl, hi, k16);
                            self.s.m.bin(c, Bin::Or, h, hi)
                        }
                        None => h,
                    });
                }
                self.write(inst, Reg::V(reg + k as u8), x.expect("a channel"), guard);
            }
            return Ok(());
        }
        for k in 0..count {
            let x = self.s.m.extract(c, r, k);
            let x = self.as_int(x);
            self.write(inst, Reg::V(reg + k as u8), x, guard);
        }
        Ok(())
    }

    fn export(&mut self, b: usize, i: usize) -> Result<(), Why> {
        let e = *self
            .exports
            .get(&(b, i))
            .ok_or("an export I/O raising did not see")?;
        if e.compressed {
            return Err("a compressed export");
        }
        let guard = self.guard(b, i)?;
        let mesh = self.s.mesh.is_some();
        let c = self.cur;
        if let Some(g) = guard
            && !mesh
        {
            if !(e.done && self.s.pixel) {
                return Err("a conditional export");
            }
            let t = self.s.m.bool(true);
            let kill = self.s.m.bin(c, Bin::Xor, g, t);
            self.s
                .op(c, "discard", Attr::NoUnwind, self.s.t.void, 82, &[kill]);
        }
        if mesh && e.target == Target::Primitive {
            return self.emit_indices(e, guard);
        }
        let param_row = |usage: Usage, row: u8| match usage {
            // The 2020 compiler leaves a mesh program's first entry zeroed
            // (usage "position"): its row is the one no other entry names.
            Usage::Other(0) if mesh => self.unnamed_row(),
            _ => Some(row),
        };
        // (output element, channel) per exported channel; per-primitive
        // outputs are elements of `prims`.
        let mut stores = Vec::new();
        let primitive = matches!(e.target, Target::PrimitiveParam { .. });
        let outputs = if primitive {
            &self.s.prims
        } else {
            &self.s.outputs
        };
        let by_kind = |kind: u8| outputs.iter().position(|o| o.kind == kind);
        // 16-bit vertex outputs: the halves each dword carries, as (output
        // element, channel, high half) per channel.
        let halves = match e.target {
            Target::Param { usage, row } => {
                param_row(usage, row).and_then(|r| self.half_outputs.get(&r).map(|&h| (r, h)))
            }
            _ => None,
        };
        if let Some((row, (pack2, b))) = halves {
            for ch in 0..4u8 {
                let Some(value) = e.values[usize::from(ch)] else {
                    continue;
                };
                for high in [false, true] {
                    let h = 2 * ch + u8::from(high);
                    let src = if pack2 {
                        packed_half(outputs, row, b, h)
                    } else {
                        (!high).then_some((row, ch))
                    };
                    let found = src.and_then(|(r, c)| {
                        outputs.iter().position(|o| {
                            o.row == Some(u32::from(r)) && c >= o.col && c < o.col + o.cols
                        })
                    });
                    if let Some(id) = found {
                        let col = src.expect("found").1 - outputs[id].col;
                        stores.push((id, col, value, Some(high)));
                    }
                }
            }
        }
        for ch in 0..4u8 {
            let Some(value) = e.values[usize::from(ch)] else {
                continue;
            };
            if halves.is_some() {
                break;
            }
            let found = match e.target {
                Target::Color(n) => outputs
                    .iter()
                    .position(|o| o.kind == 16 && o.row == Some(u32::from(n)))
                    .map(|id| (id, ch)),
                Target::Position(0) => by_kind(3).map(|id| (id, ch)),
                Target::Depth => match ch {
                    0 => by_kind(17)
                        .or(by_kind(18))
                        .or(by_kind(19))
                        .map(|id| (id, 0)),
                    1 => by_kind(20).map(|id| (id, 0)),
                    2 => by_kind(14).map(|id| (id, 0)),
                    _ => None,
                },
                Target::Param { usage, row } | Target::PrimitiveParam { usage, row } => {
                    let row = param_row(usage, row);
                    outputs
                        .iter()
                        .position(|o| {
                            row.is_some_and(|r| o.row == Some(u32::from(r)))
                                && ch >= o.col
                                && ch < o.col + o.cols
                        })
                        .map(|id| (id, ch - outputs[id].col))
                }
                Target::Null | Target::Unread => None,
                Target::Position(n @ 1..=2) => {
                    // shortcut: without the misc vector (point size, render
                    // target or viewport index) pos1 and pos2 carry the clip
                    // then cull distances, in semantic order.
                    if outputs.iter().any(|o| matches!(o.kind, 4 | 5)) {
                        return Err("render-target or viewport index exports");
                    }
                    let mut dists: Vec<usize> = (0..outputs.len())
                        .filter(|&i| matches!(outputs[i].kind, 6 | 7))
                        .collect();
                    dists.sort_by_key(|&i| (outputs[i].kind, outputs[i].index));
                    let flat: Vec<(usize, u8)> = dists
                        .iter()
                        .flat_map(|&i| (0..outputs[i].cols).map(move |c| (i, c)))
                        .collect();
                    // Channels past the distances the signature declares are
                    // ones the clipper does not enable (`PA_CL_VS_OUT_CNTL`).
                    let k = usize::from(n - 1) * 4 + usize::from(ch);
                    flat.get(k).copied()
                }
                Target::Position(_) => return Err("clip, cull or point-size exports"),
                Target::Primitive => return Err("a primitive export"),
            };
            if let Some((id, col)) = found {
                stores.push((id, col, value, None));
            }
        }
        fn elements<'l>(l: &'l mut Lower, primitive: bool) -> &'l mut Vec<SigElement> {
            if primitive {
                &mut l.s.prims
            } else {
                &mut l.s.outputs
            }
        }
        if stores
            .iter()
            .any(|s| s.3.is_none() && elements(self, primitive)[s.0].half)
        {
            return Err("a 16-bit output the stage data does not mark");
        }
        // A mesh lane writes its own vertex or primitive, where it exports.
        let at = if mesh {
            self.begin_guarded(guard)
        } else {
            None
        };
        let (name, opcode) = match (mesh, primitive) {
            (false, _) => ("storeOutput", 5),
            (true, false) => ("storeVertexOutput", 171),
            (true, true) => ("storePrimitiveOutput", 172),
        };
        for (id, col, value, half) in stores {
            let mut x = self.i32_of(value)?;
            if let Some(high) = half {
                // An f16 half, declared a 32-bit float on the host.
                let c = self.cur;
                if high {
                    let k16 = self.s.m.i32(16);
                    x = self.s.m.bin(c, Bin::LShr, x, k16);
                }
                let f32t = self.s.t.f32;
                let f = self
                    .s
                    .op(c, "legacyF16ToF32", Attr::ReadNone, f32t, 131, &[x]);
                x = self.as_int(f);
            }
            let el = &mut elements(self, primitive)[id];
            el.usage |= 1 << col;
            let (suffix, x) = match el.comp {
                9 if half.is_some() => (".f32", self.as_float(x)),
                9 => {
                    let f = match self.floats.get(&value) {
                        Some(&f) => f,
                        None => self.as_float(x),
                    };
                    (".f32", f)
                }
                _ => (".i32", x),
            };
            let mut args = vec![
                self.s.m.i32(id as u32),
                self.s.m.i32(0),
                self.s.m.i8(col),
                x,
            ];
            if mesh {
                args.push(self.mesh_thread());
            }
            let void = self.s.t.void;
            self.s.op(
                self.cur,
                &format!("{name}{suffix}"),
                Attr::NoUnwind,
                void,
                opcode,
                &args,
            );
        }
        self.end_guarded(at, &[]);
        Ok(())
    }

    /// Mesh: `exp prim` as the lane's primitive. Bit 31 culls it (all
    /// indices 0: no area).
    fn emit_indices(&mut self, e: &Export, guard: Option<V>) -> Result<(), Why> {
        let v = e.values[0].ok_or("a primitive export without its indices")?;
        let x = self.i32_of(v)?;
        let at = self.begin_guarded(guard);
        let c = self.cur;
        let z = self.s.m.i32(0);
        let culled = self.s.m.cmp(c, pred::SLT, x, z);
        let mut args = vec![self.mesh_thread()];
        for k in 0..3 {
            let (shift, mask) = (self.s.m.i32(10 * k), self.s.m.i32(0x3FF));
            let i = self.s.m.bin(c, Bin::LShr, x, shift);
            let i = self.s.m.bin(c, Bin::And, i, mask);
            args.push(self.s.m.select(c, culled, z, i));
        }
        let void = self.s.t.void;
        self.s
            .op(c, "emitIndices", Attr::NoUnwind, void, 169, &args);
        self.end_guarded(at, &[]);
        Ok(())
    }

    /// The per-vertex arbitrary row no mesh output entry names but one
    /// (the 2020 compiler's zeroed entry).
    fn unnamed_row(&self) -> Option<u8> {
        let StageIo::Ms(ms) = &self.x.native.io else {
            return None;
        };
        let named = |r: u32| {
            ms.outputs
                .iter()
                .any(|o| o.param & 0x80 == 0 && o.usage != Usage::Other(0) && u32::from(o.row) == r)
        };
        let free: std::collections::BTreeSet<u32> = self
            .s
            .outputs
            .iter()
            .filter(|o| o.kind == 0)
            .filter_map(|o| o.row)
            .filter(|&r| !named(r))
            .collect();
        match free.into_iter().collect::<Vec<_>>()[..] {
            [r] => u8::try_from(r).ok(),
            _ => None,
        }
    }

    /// The program's natural loops (from their reachable back edges), and
    /// for each that lanes leave through one block (every edge into it from
    /// the loop): the scalar values written in the loop that code after it
    /// reads (see [`Lower::leave_loops`]).
    fn find_loop_exits(&mut self) {
        let p = self.x.p;
        let reachable = |l: &Self, q: usize| l.block[q].is_some();
        let mut succs: Vec<Vec<usize>> = vec![Vec::new(); p.blocks.len()];
        for (b, block) in p.blocks.iter().enumerate() {
            if reachable(self, b) {
                for &q in block.preds.iter().filter(|&&q| reachable(self, q)) {
                    succs[q].push(b);
                }
            }
        }
        let scalar = |r: Reg| match r {
            Reg::S(c) => c != EXEC && c != EXEC + 1,
            Reg::Scc => true,
            _ => false,
        };
        let mut headers: Vec<usize> = self.counters.keys().copied().collect();
        headers.sort_unstable();
        for h in headers {
            let mut body: HashSet<usize> = HashSet::from([h]);
            let mut work: Vec<usize> = p.blocks[h]
                .preds
                .iter()
                .copied()
                .filter(|&q| reachable(self, q) && self.rpo[h] <= self.rpo[q])
                .collect();
            while let Some(q) = work.pop() {
                if body.insert(q) {
                    work.extend(
                        p.blocks[q]
                            .preds
                            .iter()
                            .copied()
                            .filter(|&r| reachable(self, r)),
                    );
                }
            }
            let mut exits: Vec<usize> = body
                .iter()
                .flat_map(|&b| succs[b].iter().copied())
                .filter(|t| !body.contains(t))
                .collect();
            exits.sort_unstable();
            exits.dedup();
            let single = match exits[..] {
                [e] if p.blocks[e]
                    .preds
                    .iter()
                    .all(|q| !reachable(self, *q) || body.contains(q)) =>
                {
                    Some(e)
                }
                _ => None,
            };
            // A waterfall loop runs once with each lane's own descriptor (its
            // compares hold in every lane), so its scalars stay each lane's.
            let waterfall = self
                .x
                .res
                .waterfall
                .compares
                .iter()
                .any(|site| body.contains(&site.0));
            if let (Some(e), true) = (single, waterfall) {
                // Listed without values: the exit joins nothing either.
                self.loop_exits.entry(e).or_default();
            } else if let Some(e) = single {
                let mut written = HashSet::new();
                for &b in &body {
                    let block = &p.blocks[b];
                    let phis = block.phis.iter().map(|phi| (phi.reg, phi.value));
                    let writes = block.insts.iter().flat_map(|i| i.writes.iter().copied());
                    written.extend(
                        phis.chain(writes)
                            .filter(|&(r, _)| scalar(r))
                            .map(|(_, v)| v),
                    );
                }
                let mut after = Vec::new();
                let mut seen = HashSet::new();
                for (b, block) in p.blocks.iter().enumerate() {
                    if body.contains(&b) || !reachable(self, b) {
                        continue;
                    }
                    let reads = block
                        .insts
                        .iter()
                        .flat_map(|i| i.reads.iter().map(|r| r.1))
                        .chain(block.phis.iter().flat_map(|phi| phi.args.iter().copied()));
                    for v in reads {
                        if written.contains(&v) && seen.insert(v) {
                            after.push(v);
                        }
                    }
                }
                self.loop_exits.entry(e).or_default().push((h, after));
            }
            self.loop_bodies.insert(h, body);
        }
    }

    /// Entering `b`, a join of branches (not a loop header): its scalar
    /// joins as the wave has them. The console runs each path some lane
    /// takes, in code order, with one set of scalar registers, so after the
    /// join every lane reads what the last path run left; here each lane
    /// reads what its own path left. Each lane's path is ranked by its code
    /// order, and the lanes of the highest rank hold the wave's values.
    // shortcut: a scalar both paths change is the later path's own result,
    // not that path run on the earlier one's.
    fn join_scalars(&mut self, b: usize) {
        let p = self.x.p;
        if self.counters.contains_key(&b) {
            return;
        }
        let scalar = |r: Reg| match r {
            Reg::S(c) => c != EXEC && c != EXEC + 1,
            Reg::Scc => true,
            _ => false,
        };
        let joins: Vec<usize> = (0..p.blocks[b].phis.len())
            .filter(|&i| {
                let phi = &p.blocks[b].phis[i];
                scalar(phi.reg)
                    && matches!(self.vals.get(&phi.value), Some(Val::I32(_) | Val::Bit(_)))
            })
            .collect();
        if joins.is_empty() {
            return;
        }
        // Reachable predecessors and the DXIL block each edge arrives from.
        let mut preds: Vec<(usize, usize)> = Vec::new();
        for &q in &p.blocks[b].preds {
            let Some(tail) = self.tail[q] else { continue };
            let via = self.edges.get(&(q, b)).copied().unwrap_or(tail);
            preds.push((q, via));
        }
        if preds.len() < 2 {
            return;
        }
        let mut order: Vec<usize> = preds.iter().map(|&(q, _)| q).collect();
        order.sort_by_key(|&q| (p.blocks[q].offset, self.rpo[q]));
        order.dedup();
        let c = self.cur;
        let i32t = self.s.t.i32;
        let rank = self.s.m.phi(c, i32t);
        let incoming: Vec<(V, usize)> = preds
            .iter()
            .map(|&(q, via)| {
                let r = order.iter().position(|&x| x == q).expect("a predecessor") as u32;
                (self.s.m.i32(r), via)
            })
            .collect();
        self.s.m.set_phi(rank, incoming);
        let (max, unsigned) = (self.s.m.i8(3), self.s.m.i8(1));
        let wave_max = |l: &mut Self, x: V| {
            l.s.op(
                c,
                "waveActiveOp.i32",
                Attr::NoUnwind,
                i32t,
                119,
                &[x, max, unsigned],
            )
        };
        let last = wave_max(self, rank);
        let mine = self.s.m.cmp(c, pred::EQ, rank, last);
        self.s.flags |= flags::WAVE_OPS;
        let zero = self.s.m.i32(0);
        for i in joins {
            let v = p.blocks[b].phis[i].value;
            let f = match self.vals[&v] {
                Val::I32(x) => {
                    let x = self.s.m.select(c, mine, x, zero);
                    Val::I32(wave_max(self, x))
                }
                Val::Bit(x) => {
                    let x = self.s.m.cast(c, Cast::ZExt, x, i32t);
                    let x = self.s.m.select(c, mine, x, zero);
                    let w = wave_max(self, x);
                    Val::Bit(self.s.m.cmp(c, pred::NE, w, zero))
                }
                Val::Missing(_) => continue,
            };
            self.vals.insert(v, f);
        }
    }

    /// Entering `b`, the block lanes leave loops through: the scalar values
    /// those loops wrote, as the wave's last iteration left them. The console
    /// runs a divergent loop until its last lane leaves, so afterwards every
    /// lane reads the last iteration's scalars; here each lane leaves with
    /// those of its own last iteration. The lanes that ran the most
    /// iterations hold the wave's (a scalar is the same in all of them).
    // shortcut: a value lowered after this block (a loop block later in
    // reverse postorder) keeps each lane's own.
    fn leave_loops(&mut self, b: usize) {
        let Some(loops) = self.loop_exits.get(&b).cloned() else {
            return;
        };
        let c = self.cur;
        let i32t = self.s.t.i32;
        for (h, values) in loops {
            let (count, _) = self.counters[&h];
            let (max, unsigned) = (self.s.m.i8(3), self.s.m.i8(1));
            let wave_max = |l: &mut Self, x: V| {
                l.s.op(
                    c,
                    "waveActiveOp.i32",
                    Attr::NoUnwind,
                    i32t,
                    119,
                    &[x, max, unsigned],
                )
            };
            let last = wave_max(self, count);
            let mine = self.s.m.cmp(c, pred::EQ, count, last);
            self.s.flags |= flags::WAVE_OPS;
            let zero = self.s.m.i32(0);
            let final_of = |l: &mut Self, val: Val| -> Option<Val> {
                Some(match val {
                    Val::I32(x) => {
                        let x = l.s.m.select(c, mine, x, zero);
                        Val::I32(wave_max(l, x))
                    }
                    Val::Bit(x) => {
                        let x = l.s.m.cast(c, Cast::ZExt, x, i32t);
                        let x = l.s.m.select(c, mine, x, zero);
                        let w = wave_max(l, x);
                        Val::Bit(l.s.m.cmp(c, pred::NE, w, zero))
                    }
                    Val::Missing(_) => return None,
                })
            };
            for v in values {
                let Some(val) = self.vals.get(&v).copied() else {
                    continue;
                };
                if let Some(f) = final_of(self, val) {
                    self.finals.insert(v, (f, h));
                }
            }
            // This block's joins of the loop's scalars.
            for phi in &self.x.p.blocks[b].phis {
                let scalar = matches!(phi.reg, Reg::S(r) if r != EXEC && r != EXEC + 1)
                    || phi.reg == Reg::Scc;
                if !scalar {
                    continue;
                }
                if let Some(val) = self.vals.get(&phi.value).copied()
                    && let Some(f) = final_of(self, val)
                {
                    self.vals.insert(phi.value, f);
                }
            }
        }
    }

    fn exit(&mut self, b: usize) -> Result<(), Stop> {
        let offset = self.x.p.blocks[b]
            .insts
            .last()
            .map_or(self.x.p.blocks[b].offset, |i| i.machine.offset);
        let term = match self.x.st.exits[b] {
            LaneExit::Jump(t) if self.counters.contains_key(&t) && self.rpo[t] <= self.rpo[b] => {
                let ok = self.count_iteration(t, pred::IULT);
                let abort = self.abort_block();
                Term::CondBr(ok, self.block[t].expect("reachable"), abort)
            }
            LaneExit::Jump(t) => {
                self.enter_loop(b, t, self.cur);
                Term::Br(self.block[t].expect("reachable"))
            }
            LaneExit::Branch {
                taken, not_taken, ..
            } if self.console_cf => {
                let cond = self.console_branch(b).map_err(|w| unsupported(offset, w))?;
                self.branch_term(b, cond, taken, not_taken)
            }
            LaneExit::Branch {
                when,
                taken,
                not_taken,
            } => {
                // A negated condition branches the other way round.
                let (cond, taken, not_taken) = match self.x.st.bools.node(when) {
                    Some((atom, Bool::TRUE, Bool::FALSE)) => {
                        let a = self
                            .atom(atom, self.cur)
                            .map_err(|w| unsupported(offset, w))?;
                        (a, not_taken, taken)
                    }
                    _ => {
                        let c = self
                            .boolean(when, self.cur)
                            .map_err(|w| unsupported(offset, w))?;
                        (c, taken, not_taken)
                    }
                };
                self.branch_term(b, cond, taken, not_taken)
            }
            LaneExit::End => {
                if self.s.pixel && !self.exported[b] && !self.has_done_export(b) {
                    // A lane leaving without its final export was killed.
                    let t = self.s.m.bool(true);
                    self.s
                        .op(self.cur, "discard", Attr::NoUnwind, self.s.t.void, 82, &[t]);
                }
                Term::Ret
            }
            LaneExit::Indirect => return Err(unsupported(offset, "an indirect jump")),
        };
        let _ = Exit::End;
        self.s.m.terminate(self.cur, term);
        Ok(())
    }

    /// The terminator of block `b` branching to `taken` where `cond` holds.
    fn branch_term(&mut self, b: usize, cond: V, taken: usize, not_taken: usize) -> Term {
        // A back edge is taken only below the iteration limit.
        let back = |l: &Self, to: usize| l.counters.contains_key(&to) && l.rpo[to] <= l.rpo[b];
        // (No `xor` of a compare: a loop exit testing one hung the
        // AMD driver.)
        let cond = if back(self, taken) {
            let ok = self.count_iteration(taken, pred::IULT);
            self.s.m.bin(self.cur, Bin::And, cond, ok)
        } else if back(self, not_taken) {
            let stop = self.count_iteration(not_taken, pred::IUGE);
            self.s.m.bin(self.cur, Bin::Or, cond, stop)
        } else {
            cond
        };
        // Critical edges get a block of their own, so loops have
        // dedicated exits and preheaders as DXC emits them.
        let (t, f) = (self.edge(b, taken), self.edge(b, not_taken));
        for (to, via) in [(taken, t), (not_taken, f)] {
            if !back(self, to) {
                // From an edge block, or from here.
                let from = if Some(via) == self.block[to] {
                    self.cur
                } else {
                    via
                };
                self.enter_loop(b, to, from);
            }
        }
        Term::CondBr(cond, t, f)
    }

    /// Console flow: whether block `b`'s `s_cbranch_*` is taken, the same
    /// in both halves (wave-wide mask tests through [`Self::any_lane`]).
    fn console_branch(&mut self, b: usize) -> Result<V, Why> {
        let inst = self.x.p.blocks[b].insts.last().ok_or("a branch")?;
        let name = inst.name();
        let c = self.cur;
        let (any, set) = match name {
            "s_cbranch_scc0" | "s_cbranch_scc1" => {
                let v = inst.read(Reg::Scc).ok_or("a branch's SCC")?;
                let scc = self.scc_bit(v)?;
                (scc, name == "s_cbranch_scc1")
            }
            "s_cbranch_vccz" | "s_cbranch_vccnz" | "s_cbranch_execz" | "s_cbranch_execnz" => {
                let reg = if name.contains("vcc") {
                    crate::ir::VCC
                } else {
                    EXEC
                };
                let lo = inst.read(Reg::S(reg)).ok_or("a branch's mask")?;
                let &m = self
                    .x
                    .st
                    .masks
                    .get(&lo)
                    .ok_or("a branch on an untracked mask")?;
                (self.any_lane(m)?, name.ends_with("nz"))
            }
            _ => return Err("a branch the console flow does not model"),
        };
        Ok(if set {
            any
        } else {
            let t = self.s.m.bool(true);
            self.s.m.bin(c, Bin::Xor, any, t)
        })
    }

    /// Console flow: an SCC value as a bit. A mask operation's SCC (result
    /// not zero) is whether some lane of the console wave has its bit.
    fn scc_bit(&mut self, v: Value) -> Result<V, Why> {
        if let Some(&Val::Bit(x)) = self.vals.get(&v) {
            return Ok(x);
        }
        let b = *self
            .def_block
            .get(&v)
            .ok_or("an SCC from the program's start")?;
        let inst = self.x.p.blocks[b]
            .insts
            .iter()
            .find(|i| i.writes.iter().any(|w| w.1 == v))
            .ok_or("an SCC without its instruction")?;
        let mask = inst
            .writes
            .iter()
            .filter(|w| matches!(w.0, Reg::S(_)))
            .find_map(|w| self.x.st.masks.get(&w.1).copied())
            .ok_or("an SCC the emitter did not compute")?;
        self.any_lane(mask)
    }

    /// Console flow: whether `m` holds in some lane of the console wave.
    fn any_lane(&mut self, m: Bool) -> Result<V, Why> {
        let c = self.cur;
        let bit = self.boolean(m, c)?;
        let (lo, hi) = self.console_ballot(c, bit);
        let or = self.s.m.bin(c, Bin::Or, lo, hi);
        let z = self.s.m.i32(0);
        Ok(self.s.m.cmp(c, pred::NE, or, z))
    }

    /// Console flow: the console wave's ballot of `bit`, low and high word.
    fn console_ballot(&mut self, at: usize, bit: V) -> (V, V) {
        if self.split {
            let word = self.ballot_word(at, bit);
            return self.exchange(at, word);
        }
        // One host wave is the console wave (the high word is 0 in waves
        // of 32).
        let i32t = self.s.t.i32;
        let four = self.s.m.named_struct("dx.types.fouri32", vec![i32t; 4]);
        let r = self
            .s
            .op(at, "waveActiveBallot", Attr::NoUnwind, four, 116, &[bit]);
        self.s.flags |= flags::WAVE_OPS;
        (self.s.m.extract(at, r, 0), self.s.m.extract(at, r, 1))
    }

    /// The host wave's ballot of `bit` (32 lanes: the first word).
    fn ballot_word(&mut self, at: usize, bit: V) -> V {
        let i32t = self.s.t.i32;
        let four = self.s.m.named_struct("dx.types.fouri32", vec![i32t; 4]);
        let r = self
            .s
            .op(at, "waveActiveBallot", Attr::NoUnwind, four, 116, &[bit]);
        self.s.flags |= flags::WAVE_OPS;
        self.s.m.extract(at, r, 0)
    }

    /// The console thread's flat id in the group (see [`Self::thread_id`]).
    fn console_flat(&mut self) -> V {
        if let Some(f) = self.console_flat {
            return f;
        }
        let e = self.s.entry;
        let i32t = self.s.t.i32;
        let flat = self.s.op(
            e,
            "flattenedThreadIdInGroup.i32",
            Attr::ReadNone,
            i32t,
            96,
            &[],
        );
        let first = self.s.op(
            e,
            "waveReadLaneFirst.i32",
            Attr::NoUnwind,
            i32t,
            118,
            &[flat],
        );
        let wave = self.s.wave_size.expect("set with console lanes");
        let mask = self.s.m.i32(!(wave - 1));
        let base = self.s.m.bin(e, Bin::And, first, mask);
        let lane = self.s.op(
            e,
            "waveGetLaneIndex",
            Attr::ReadOnly,
            self.s.t.i32,
            111,
            &[],
        );
        let f = self.s.m.bin(e, Bin::Add, base, lane);
        self.s.flags |= flags::WAVE_OPS;
        self.console_flat = Some(f);
        f
    }

    /// This lane's console lane (0..63, 0..31 in waves of 32) and its
    /// half of a wave64 (0, 1).
    fn console_lane(&mut self) -> (V, V) {
        let f = self.console_flat();
        let e = self.s.entry;
        let (mask, k5) = (self.s.m.i32(self.wave() - 1), self.s.m.i32(5));
        let lane = self.s.m.bin(e, Bin::And, f, mask);
        let half = self.s.m.bin(e, Bin::LShr, lane, k5);
        (lane, half)
    }

    /// Console flow: each half gives `x` (the same in all its lanes) and
    /// both get (the lower half's, the upper half's): through two group-
    /// shared dwords between group barriers, which every thread of the
    /// group reaches (see [`CONSOLE_FLOW`]). A group of 32 threads or
    /// fewer has no upper half: its lanes count as inactive there (0).
    fn exchange(&mut self, at: usize, x: V) -> (V, V) {
        let threads: u32 = self.s.threads.expect("compute").iter().product();
        let z = self.s.m.i32(0);
        if threads <= 32 {
            return (x, z);
        }
        let (_, half) = self.console_lane();
        let slots = match self.xchg {
            Some(g) => g,
            None => {
                let arr = self.s.m.array(2, self.s.t.i32);
                let g = self.s.m.global("xchg", arr, 3);
                self.xchg = Some(g);
                g
            }
        };
        let void = self.s.t.void;
        // Thread-group sync with a group-shared memory fence.
        let mode = self.s.m.i32(1 | 8);
        let mine = self.s.m.element(at, slots, half);
        self.s.m.store(at, mine, x);
        self.s
            .op(at, "barrier", Attr::NoDuplicate, void, 80, &[mode]);
        let one = self.s.m.i32(1);
        let p0 = self.s.m.element(at, slots, z);
        let p1 = self.s.m.element(at, slots, one);
        let lo = self.s.m.load(at, p0);
        let hi = self.s.m.load(at, p1);
        // Both read before the next exchange writes.
        self.s
            .op(at, "barrier", Attr::NoDuplicate, void, 80, &[mode]);
        (lo, hi)
    }

    /// A forward edge into loop header `to` (from DXIL block `via`) starts
    /// its count at 0.
    fn enter_loop(&mut self, _from: usize, to: usize, via: usize) {
        if let Some((_, incoming)) = self.counters.get_mut(&to) {
            let z = self.s.m.i32(0);
            incoming.push((z, via));
        }
    }

    /// A back edge to header `to` (a back edge keeps its block, no
    /// critical-edge block): the count goes up; the result compares it with
    /// [`LOOP_LIMIT`] by `p` (below: go on; at or above: stop).
    fn count_iteration(&mut self, to: usize, p: u8) -> V {
        let c = self.cur;
        let (phi, _) = self.counters[&to];
        let one = self.s.m.i32(1);
        let next = self.s.m.bin(c, Bin::Add, phi, one);
        let limit = self.s.m.i32(super::loop_limit());
        self.counters
            .get_mut(&to)
            .expect("header")
            .1
            .push((next, c));
        self.s.m.cmp(c, p, next, limit)
    }

    /// The block a lane stopped by [`LOOP_LIMIT`] leaves through.
    fn abort_block(&mut self) -> usize {
        if let Some(a) = self.abort {
            return a;
        }
        let a = self.s.m.new_block();
        if self.s.pixel {
            let t = self.s.m.bool(true);
            self.s
                .op(a, "discard", Attr::NoUnwind, self.s.t.void, 82, &[t]);
        }
        self.s.m.terminate(a, Term::Ret);
        self.abort = Some(a);
        a
    }

    /// The DXIL block a branch from IR block `from` to `to` goes to.
    fn edge(&mut self, from: usize, to: usize) -> usize {
        let target = self.block[to].expect("reachable");
        let preds = self.x.p.blocks[to]
            .preds
            .iter()
            .filter(|&&q| self.block[q].is_some())
            .count();
        // Back edges stay as they are: the exiting block is the latch.
        if preds < 2 || self.rpo[to] <= self.rpo[from] {
            return target;
        }
        let e = self.s.m.new_block();
        self.s.m.terminate(e, Term::Br(target));
        self.edges.insert((from, to), e);
        e
    }

    /// A dword a scalar load writes, where the load is a descriptor the
    /// program uses as a resource or sampler.
    fn descriptor_dword(&self, v: Value) -> bool {
        let Some(load) = self.x.p.blocks.iter().enumerate().find_map(|(b, block)| {
            block
                .insts
                .iter()
                .position(|i| i.writes.iter().any(|w| w.1 == v))
                .map(|i| (b, i))
        }) else {
            return false;
        };
        self.x.p.blocks[load.0].insts[load.1]
            .name()
            .starts_with("s_load_dword")
            && self.x.res.descriptors.iter().any(|d| {
                d.source
                    .leaves()
                    .iter()
                    .any(|l| matches!(l, Source::Loaded { load: at, .. } if *at == load))
            })
    }

    fn fill_phis(&mut self) -> Result<(), Stop> {
        self.fill_paths().map_err(|w| unsupported(0, w))?;
        let p = self.x.p;
        // A phi that only another read phi consumes is live too. Without this, a value with no DXIL form that
        // reaches a read through two joins is taken for dead at the first join and replaced by zero below, and the
        // lowering accepts what it refuses when the same value is read directly.
        let mut work: Vec<Value> = self.read_phis.iter().copied().collect();
        while let Some(v) = work.pop() {
            let Some(&(b, i)) = self.phi_at.get(&v) else {
                continue;
            };
            for &arg in &p.blocks[b].phis[i].args {
                if self.phi_at.contains_key(&arg) && self.read_phis.insert(arg) {
                    work.push(arg);
                }
            }
        }
        for (v, b, i) in std::mem::take(&mut self.data_phis) {
            let phi = &p.blocks[b].phis[i];
            let bit = phi.reg == Reg::Scc;
            // A register initialized only on one branch may carry a dead
            // descriptor dword left over from earlier register reuse on the
            // other branch (guarded by a separate predicate). Only treat a
            // descriptor-dword `Missing` as fatal when no incoming edge carries
            // a real data value.
            let has_data = !bit
                && p.blocks[b]
                    .preds
                    .iter()
                    .enumerate()
                    .filter(|&(_, &q)| self.tail[q].is_some())
                    .any(|(n, _)| matches!(self.vals.get(&phi.args[n]), Some(Val::I32(_))));
            let mut incoming = Vec::new();
            for (n, &q) in p.blocks[b].preds.iter().enumerate() {
                let Some(qb) = self.tail[q] else { continue };
                let qb = self.edges.get(&(q, b)).copied().unwrap_or(qb);
                let arg = phi.args[n];
                if self.read_phis.contains(&phi.value) && !self.vals.contains_key(&arg) {
                    let _ = self.lazy_barycentric(arg);
                }
                // A root parameter no instruction read.
                if !self.vals.contains_key(&arg)
                    && let Some(&(param, dword)) = self.params.get(&arg)
                    && self.read_phis.contains(&phi.value)
                {
                    let val = self.user(param, dword);
                    self.vals.insert(arg, val);
                }
                // An edge from outside a loop takes the loop's final value.
                let looked = match self.finals.get(&arg) {
                    Some(&(f, h)) if !self.loop_bodies[&h].contains(&q) => Some(f),
                    _ => self.vals.get(&arg).copied(),
                };
                let x = match (looked, bit) {
                    (Some(Val::I32(x)), false) | (Some(Val::Bit(x)), true) => x,
                    (Some(Val::Missing(why)), false)
                        if has_data
                            && matches!(
                                why,
                                "a descriptor's address or format read as data"
                                    | "a descriptor read only as data"
                            ) =>
                    {
                        self.s.m.i32(0)
                    }
                    (Some(Val::Bit(_)), false) if self.read_phis.contains(&phi.value) => {
                        return Err(Stop::Retry(phi.value, "an SCC joined with SGPR values"));
                    }
                    // A descriptor's dwords reaching a join that also takes
                    // data (the registers are reused after the last access).
                    // shortcut: undefined on that edge, assuming the program
                    // reads the joined value only after the data edge.
                    (Some(Val::Missing(_)), _) if self.descriptor_dword(arg) => {
                        if bit {
                            self.s.m.undef(self.s.t.i1)
                        } else {
                            self.s.m.undef(self.s.t.i32)
                        }
                    }
                    // SCC of a mask operation joined with a comparison's: the
                    // edge past a section no lane is in, where it is clear.
                    (Some(Val::Missing(why)), true)
                        if why == LANE_MASK
                            && phi
                                .args
                                .iter()
                                .any(|a| matches!(self.vals.get(a), Some(Val::Bit(_)))) =>
                    {
                        self.s.m.bool(false)
                    }
                    // As above, of a lane mask: the registers held one before.
                    // shortcut: as above, zero there.
                    (Some(Val::Missing(why)), false)
                        if has_data
                            && (why == LANE_MASK
                                || self.x.st.masks.contains_key(&arg)
                                || self.x.st.highs.contains_key(&arg)) =>
                    {
                        self.s.m.i32(0)
                    }
                    (Some(Val::Missing(why)), _) if self.read_phis.contains(&phi.value) => {
                        return Err(Stop::Retry(phi.value, why));
                    }
                    _ if self.read_phis.contains(&phi.value) => {
                        return Err(Stop::Retry(phi.value, "a join of mismatched values"));
                    }
                    _ if bit => self.s.m.bool(false),
                    _ => self.s.m.i32(0),
                };
                incoming.push((x, qb));
            }
            self.s.m.set_phi(v, incoming);
        }
        for (phi, incoming) in std::mem::take(&mut self.counters).into_values() {
            self.s.m.set_phi(phi, incoming);
        }
        while let Some((v, b, i)) = self.pending_masks.pop() {
            let phi = &p.blocks[b].phis[i];
            let offset = p.blocks[b].offset;
            let mut incoming = Vec::new();
            for (n, &q) in p.blocks[b].preds.iter().enumerate() {
                let Some(qb) = self.tail[q] else { continue };
                let qb = self.edges.get(&(q, b)).copied().unwrap_or(qb);
                // Console flow: every lane arrives by every edge, so a join
                // also takes masks that are constants or built from scalars.
                let arg = &phi.args[n];
                // shortcut: a plain scalar on the edge is the register in
                // another use, taken for a mask no lane is in; right as long
                // as the program does not read the join after that edge.
                let x = match self.x.st.masks.get(arg).or_else(|| self.x.st.others.get(arg)) {
                    Some(&m) => self.boolean(m, qb).map_err(|w| unsupported(offset, w))?,
                    None => self.s.m.bool(false),
                };
                incoming.push((x, qb));
            }
            self.s.m.set_phi(v, incoming);
        }
        Ok(())
    }
}

/// The DXIL interpolation mode of a kind of barycentrics (none for the
/// pull model).
/// The host's limit on a mesh shader's vertex plus primitive outputs
/// (DXC's `kMaxMSOutputTotalBytes`).
const MAX_MESH_OUTPUT_BYTES: u32 = 32768;

/// A mesh shader's output size as the validator counts it: every element's
/// components times the maximum vertex or primitive count, each rounded up
/// to 32, four bytes each.
fn mesh_output_bytes(
    outputs: &[SigElement],
    prims: &[SigElement],
    mesh: super::shader::Mesh,
) -> u32 {
    let scalars = |els: &[SigElement], n: u32| -> u32 {
        els.iter().map(|e| u32::from(e.cols)).sum::<u32>() * n.next_multiple_of(32)
    };
    4 * (scalars(outputs, mesh.max_vertices) + scalars(prims, mesh.max_primitives))
}

/// The coordinates `calculateLOD` takes for a texture of DXIL resource
/// kind `kind` (DXC's `GetNumDimensionsForCalcLOD`): the size's dimensions,
/// without an array slice. The validator wants the rest undef ("out of
/// bound coord must be undef").
fn lod_dims(kind: u32) -> usize {
    match kind {
        1 | 6 => 1,
        2 | 7 => 2,
        _ => 3,
    }
}

/// Gives the elements of each packed row one interpolation mode, as DXIL
/// requires ("interpolation mode that differs from another element packed
/// into the same row"). Elements whose mode is fixed (pixel inputs that
/// are read, integers and `SV_IsFrontFace`, which are constant, and the
/// position) set the row's mode; the others, floats nobody reads (vertex
/// outputs: any float), take it. Rows stay where the console put them:
/// the host links stages by register. A pixel row read two ways is an
/// error rather than a shader the validator rejects; `per_vertex` makes
/// rows with a constant element constant beforehand.
pub(super) fn settle_row_modes(elements: &mut [SigElement], pixel: bool) -> Result<(), Why> {
    // Packed elements only: barycentrics, sample index and coverage are not.
    let packed = |e: &SigElement| e.row.is_some() && !matches!(e.kind, 12 | 14 | 15 | 28 | 30);
    let fixed = |e: &SigElement| e.comp != 9 || e.kind == 3 || (pixel && e.usage != 0);
    let mut rows: Vec<u32> = elements
        .iter()
        .filter(|e| packed(e))
        .filter_map(|e| e.row)
        .collect();
    rows.sort_unstable();
    rows.dedup();
    for row in rows {
        let in_row = |e: &SigElement| packed(e) && e.row == Some(row);
        let mut modes: Vec<u8> = elements
            .iter()
            .filter(|e| in_row(e) && fixed(e))
            .map(|e| e.interp)
            .collect();
        modes.sort_unstable();
        modes.dedup();
        let mode = match modes[..] {
            [m] => m,
            // Nothing fixed: the first element's.
            [] => match elements.iter().find(|e| in_row(e)) {
                Some(e) => e.interp,
                None => continue,
            },
            _ if pixel => return Err("input elements of one row interpolated two ways"),
            // Vertex outputs: the host interpolates by the pixel inputs' modes.
            _ => continue,
        };
        for e in elements.iter_mut().filter(|e| in_row(e) && !fixed(e)) {
            e.interp = mode;
        }
    }
    Ok(())
}

/// A pixel program under conservative depth declares its position centroid
/// (`emit`); one that runs per sample (an `SV_SampleIndex` input or a
/// per-sample interpolation) declares it per sample instead.
pub(super) fn settle_position_mode(inputs: &mut [SigElement]) {
    let per_sample = inputs
        .iter()
        .any(|e| e.kind == 12 || matches!(e.interp, 6 | 7));
    for e in inputs.iter_mut().filter(|e| e.kind == 3 && e.interp == 5) {
        if per_sample {
            e.interp = 7;
        }
    }
}

fn interp_mode(kind: Barycentric) -> Option<u8> {
    Some(match kind {
        Barycentric::PerspectiveCenter => 2,
        Barycentric::PerspectiveCentroid => 3,
        Barycentric::PerspectiveSample => 6,
        Barycentric::LinearCenter => 4,
        Barycentric::LinearCentroid => 5,
        Barycentric::LinearSample => 7,
        Barycentric::PerspectivePullModel => return None,
    })
}

#[cfg(test)]
mod tests {
    use super::super::container::SigElement;
    use super::{Lowered, Numeric, Parts, SubDword, buffer_format, half_shift, inline_f16, lower};
    use crate::native::{
        ComputeIo, HwStage, NativeProgram, ResourceType, RootParam, RootType, StageIo, UsedResource,
    };
    use crate::{ShaderError, io, ir, resource, structure};

    const S_ENDPGM: u32 = 0xBF81_0000;

    /// Mesh output size as the validator counts it (53 programs were over
    /// the limit; they now fail as unsupported).
    #[test]
    fn mesh_output_size_counts_aligned_maxima() {
        let el = |cols| SigElement {
            name: "TEXCOORD".into(),
            index: 0,
            kind: 0,
            comp: 9,
            interp: 2,
            row: Some(0),
            col: 0,
            cols,
            usage: 0,
            half: false,
        };
        let mesh = |max_vertices, max_primitives| crate::emit::shader::Mesh {
            max_vertices,
            max_primitives,
            topology: 2,
        };
        // 4 + 4 columns of 64 vertices, 1 column of 126 primitives (128).
        let size = super::mesh_output_bytes(&[el(4), el(4)], &[el(1)], mesh(64, 126));
        assert_eq!(size, 4 * (8 * 64 + 128));
        // 32 columns of 256 vertices: exactly the limit; one more is over.
        let full: Vec<_> = (0..8).map(|_| el(4)).collect();
        assert_eq!(
            super::mesh_output_bytes(&full, &[], mesh(256, 1)),
            super::MAX_MESH_OUTPUT_BYTES
        );
        assert!(
            super::mesh_output_bytes(&full, &[el(1)], mesh(256, 1)) > super::MAX_MESH_OUTPUT_BYTES
        );
    }

    /// `calculateLOD` takes the size's dimensions only, as DXC's
    /// `GetNumDimensionsForCalcLOD` counts them: an array's slice
    /// coordinate is left undef.
    #[test]
    fn level_queries_leave_the_array_slice_undef() {
        // 1D, 2D, 3D, cube, 1D array, 2D array.
        let dims: Vec<usize> = [1, 2, 4, 5, 6, 7].map(super::lod_dims).to_vec();
        assert_eq!(dims, [1, 2, 3, 3, 1, 2]);
    }

    /// VOP3 `op dst, a, b` (GFX10 encoding); `flags` are the clamp, op_sel
    /// and abs bits of the first dword.
    fn vop3(op: u32, flags: u32, dst: u32, a: u32, b: u32) -> [u32; 2] {
        [0xD400_0000 | op << 16 | flags | dst, a | b << 9]
    }

    /// The DXIL of a pixel shader that exports `body`'s v2, computed from
    /// a per-lane value in v3.
    fn lower_ps(body: [u32; 2]) -> Result<Vec<u8>, ShaderError> {
        let words = [
            // v_mbcnt_lo_u32_b32 v3, -1, 0
            vop3(0x365, 0, 3, 193, 128)[0],
            vop3(0x365, 0, 3, 193, 128)[1],
            body[0],
            body[1],
            0xF800_180F, // exp mrt0, v2, v2, v2, v2 done vm
            0x0202_0202,
            0xBF81_0000, // s_endpgm
        ];
        let code: Vec<u8> = words.iter().flat_map(|w| w.to_le_bytes()).collect();
        let p = ir::build(&code, false, 256)?;
        let native = NativeProgram {
            stage: HwStage::Ps,
            wave64: false,
            vgprs: 256,
            layout: vec![],
            used: vec![],
            io: StageIo::Ps {
                inputs: vec![],
                input_addr: 0,
                db_shader_control: 0,
                patches: vec![],
            },
            code,
        };
        let io = io::raise(&p, &native)?;
        let res = resource::track(&p, &native.layout, native.stage)?;
        let st = structure::structure(&p)?;
        let target = SigElement {
            name: "SV_Target".into(),
            index: 0,
            kind: 16,
            comp: 9,
            interp: 0,
            row: Some(0),
            col: 0,
            cols: 4,
            usage: 15,
            half: false,
        };
        let parts = Parts {
            native: &native,
            p: &p,
            st: &st,
            io: &io,
            res: &res,
            inputs: vec![],
            outputs: vec![target],
            prims: vec![],
            mesh_stride: 0,
            target: crate::emit::Target::ANY,
            psv: None,
            console_cf: false,
            per_lane_only: false,
        };
        lower(&parts).map(|l| l.dxil)
    }

    /// Lowers a compute program of constants: whether it translates.
    fn lowers(words: &[u32]) -> Result<(), crate::ShaderError> {
        lower_with(words, vec![], vec![]).map(|_| ())
    }

    /// [`lowers`] with the input registers `rsrc2` names.
    fn lowers_rsrc2(words: &[u32], rsrc2: u32) -> Result<(), crate::ShaderError> {
        lower_io(words, vec![], vec![], false, Some(rsrc2)).map(|_| ())
    }

    /// Lowers a compute program with the root parameters `layout` (in user
    /// SGPRs from s0 on) and the used resources `used`.
    fn lower_with(
        words: &[u32],
        layout: Vec<RootParam>,
        used: Vec<UsedResource>,
    ) -> Result<Lowered, crate::ShaderError> {
        lower_wave(words, layout, used, false)
    }

    /// [`lower_with`] in wave64 if `wave64`.
    fn lower_wave(
        words: &[u32],
        layout: Vec<RootParam>,
        used: Vec<UsedResource>,
        wave64: bool,
    ) -> Result<Lowered, crate::ShaderError> {
        lower_io(words, layout, used, wave64, None)
    }

    /// [`lower_wave`] with the input registers `rsrc2` names (default: the
    /// layout's user SGPRs, at least two, and 512 bytes of LDS).
    fn lower_io(
        words: &[u32],
        layout: Vec<RootParam>,
        used: Vec<UsedResource>,
        wave64: bool,
        rsrc2: Option<u32>,
    ) -> Result<Lowered, crate::ShaderError> {
        let code: Vec<u8> = words.iter().flat_map(|w| w.to_le_bytes()).collect();
        let sgprs: u32 = layout.iter().map(|p| p.sgprs).sum();
        let native = NativeProgram {
            stage: HwStage::Cs,
            wave64,
            vgprs: 256,
            layout,
            used,
            io: StageIo::Cs(ComputeIo {
                rsrc2: rsrc2.unwrap_or(1 << 15 | sgprs.max(2) << 1),
                threads: [1, 1, 1],
            }),
            code,
        };
        let p = ir::build(&native.code, wave64, 256).unwrap();
        let res = resource::track(&p, &native.layout, native.stage)?;
        let io = io::raise(&p, &native).unwrap();
        let st = structure::structure(&p).unwrap();
        lower(&Parts {
            psv: None,
            native: &native,
            p: &p,
            st: &st,
            io: &io,
            res: &res,
            inputs: vec![],
            outputs: vec![],
            prims: vec![],
            mesh_stride: 0,
            target: super::super::Target::ANY,
            console_cf: false,
            per_lane_only: false,
        })
    }

    /// Root parameter `slot` in user SGPRs from `user_data` on.
    fn root(root: RootType, user_data: u8, slot: u8) -> RootParam {
        RootParam {
            root,
            table_offset_dwords: 0,
            sgprs: if root == RootType::Descriptor { 2 } else { 1 },
            base_register: 0,
            user_data,
            slot,
        }
    }

    /// The view a root descriptor at `user_data` is.
    fn root_view(kind: ResourceType, user_data: u8, slot: u8) -> UsedResource {
        UsedResource {
            kind,
            root: RootType::Descriptor,
            table_offset_dwords: 0,
            lower_bound: 0,
            stride: 0,
            dwords: 4,
            user_data,
            slot,
        }
    }

    #[test]
    fn scalar_code_inside_a_whole_wave_section_is_lowered() {
        // Assembled with llvm-mc (gfx1030, wave32): scalar code the
        // compiler scheduled into a whole-wave section, branched on after.
        let words = [
            0xBE84_0385, // s_mov_b32 s4, 5
            0xBE80_3DC1, // s_or_saveexec_b32 s0, -1
            0x8005_8104, // s_add_u32 s5, s4, 1
            0x7E02_0280, // v_mov_b32_e32 v1, 0
            0xBEFE_0300, // s_mov_b32 exec_lo, s0
            0xBF06_8605, // s_cmp_eq_u32 s5, 6
            0xBF85_0001, // s_cbranch_scc1 -> s_endpgm
            0xBE86_0380, // s_mov_b32 s6, 0
            S_ENDPGM,
        ];
        lowers(&words).unwrap();
    }

    #[test]
    fn constants_loaded_from_a_root_view() {
        // Two dwords at byte 0x10 of the memory of a root descriptor (in
        // s[0:1]) read with s_load and branched on: read through the root
        // CBV, or the root SRV when the root signature says it is one.
        let words = [
            0xF404_0080,
            0xFA00_0010, // s_load_dwordx2 s[2:3], s[0:1], 0x10
            0x8002_0302, // s_add_u32 s2, s2, s3
            0xBF06_8502, // s_cmp_eq_u32 s2, 5
            0xBF85_0001, // s_cbranch_scc1 -> s_endpgm
            0xBE84_0380, // s_mov_b32 s4, 0
            S_ENDPGM,
        ];
        let layout = || vec![root(RootType::Descriptor, 0, 3)];
        let cbv = lower_with(&words, layout(), vec![]).unwrap();
        assert_eq!((cbv.root_cbvs, cbv.root_srvs), (vec![3], vec![]));
        let used = vec![root_view(ResourceType::Srv, 0, 3)];
        let srv = lower_with(&words, layout(), used).unwrap();
        assert_eq!((srv.root_cbvs, srv.root_srvs), (vec![], vec![3]));
        // A root UAV's memory has no read-only host view.
        let used = vec![root_view(ResourceType::Uav, 0, 3)];
        assert!(lower_with(&words, layout(), used).is_err());
    }

    #[test]
    fn a_dword_of_a_descriptor_another_load_reads_whole() {
        // A 2D image's descriptor (a table range at s0) loaded whole for
        // image_get_resinfo, and its dword 2 (width and height) loaded
        // again alone and branched on.
        let words = [
            0xBE81_0380, // s_mov_b32 s1, 0
            0xF40C_0100,
            0xFA00_0000, // s_load_dwordx8 s[4:11], s[0:1], null
            0x7E04_0480, // v_mov_b32 v2, 0
            0xF038_0308,
            0x0001_0402, // image_get_resinfo v[4:5], v2, s[4:11] dim:SQ_RSRC_IMG_2D
            0xF400_0300,
            0xFA00_0008, // s_load_dword s12, s[0:1], 0x8
            0xBF06_850C, // s_cmp_eq_u32 s12, 5
            0xBF85_0001, // s_cbranch_scc1 -> s_endpgm
            0xBE84_0380, // s_mov_b32 s4, 0
            S_ENDPGM,
        ];
        let layout = || vec![root(RootType::Table, 0, 0)];
        lower_with(&words, layout(), vec![]).unwrap();
        // Dword 2 of the next descriptor: no load of it is used.
        let mut next = words;
        next[7] = 0xFA00_0028;
        let e = lower_with(&next, layout(), vec![]).err().unwrap();
        assert!(
            e.to_string().contains("a descriptor read only as data"),
            "{e}"
        );
    }

    #[test]
    fn a_descriptor_dword_loaded_through_a_rebuilt_table_pointer() {
        // A 2D image's descriptor (a table range at s0) loaded whole for an
        // image_store in an exec-masked tail, and its dword 2 (width and
        // height) loaded alone through another copy of the table pointer
        // {s0, 0} (a copy in other registers, or the high half set to 0
        // again after the registers held scratch), its height bits read.
        let layout = || vec![root(RootType::Table, 0, 0)];
        let copied = [
            0xBE81_0380, // s_mov_b32 s1, 0
            0xF400_0300,
            0xFA00_0008, // s_load_dword s12, s[0:1], 0x8
            0xBE82_0300, // s_mov_b32 s2, s0
            0xBE83_0380, // s_mov_b32 s3, 0
            0xF40C_0101,
            0xFA00_0000, // s_load_dwordx8 s[4:11], s[2:3], 0x0
            0xBF8C_C07F, // s_waitcnt lgkmcnt(0)
            0x938D_FF0C,
            0x000E_000E, // s_bfe_u32 s13, s12, 0xe000e
            0x7E0A_020D, // v_mov_b32 v5, s13
            0xD834_0000,
            0x0000_0500, // ds_write_b32 v0, v5
            0x7D88_0084, // v_cmp_gt_u32 vcc_lo, 4, v0
            0xBE82_3C6A, // s_and_saveexec_b32 s2, vcc_lo
            0xBF88_0002, // s_cbranch_execz -> s_endpgm
            0xF020_1F08,
            0x0001_0100, // image_store v[1:4], v[0:1], s[4:11] dmask:0xf dim:SQ_RSRC_IMG_2D unorm
            S_ENDPGM,
        ];
        lower_with(&copied, layout(), vec![]).unwrap();
        let rebuilt = [
            0xBE81_0380, // s_mov_b32 s1, 0
            0xF40C_0100,
            0xFA00_0000, // s_load_dwordx8 s[4:11], s[0:1], 0x0
            0x7D88_0084, // v_cmp_gt_u32 vcc_lo, 4, v0
            0xBE82_3C6A, // s_and_saveexec_b32 s2, vcc_lo
            0xBF88_0002, // s_cbranch_execz -> s_or_b32
            0xF020_1F08,
            0x0001_0100, // image_store v[1:4], v[0:1], s[4:11] dmask:0xf dim:SQ_RSRC_IMG_2D unorm
            0x887E_027E, // s_or_b32 exec_lo, exec_lo, s2
            0x7E08_0500, // v_readfirstlane_b32 s4, v0
            0x7E02_0501, // v_readfirstlane_b32 s1, v1
            0x8704_FF04,
            0x0000_01FF, // s_and_b32 s4, s4, 0x1ff
            0x8F01_8901, // s_lshl_b32 s1, s1, 9
            0x8804_0104, // s_or_b32 s4, s4, s1
            0xBE81_0380, // s_mov_b32 s1, 0
            0xF400_0300,
            0xFA00_0008, // s_load_dword s12, s[0:1], 0x8
            0xBF8C_C07F, // s_waitcnt lgkmcnt(0)
            0x938D_FF0C,
            0x000E_000E, // s_bfe_u32 s13, s12, 0xe000e
            0x8804_0D04, // s_or_b32 s4, s4, s13
            0x7E0A_0204, // v_mov_b32 v5, s4
            0xD834_0000,
            0x0000_0500, // ds_write_b32 v0, v5
            S_ENDPGM,
        ];
        lower_with(&rebuilt, layout(), vec![]).unwrap();
        // Dword 2 of the next descriptor: no load of it is used.
        let mut next = copied;
        next[2] = 0xFA00_0028;
        let e = lower_with(&next, layout(), vec![]).err().unwrap();
        assert!(
            e.to_string().contains("a descriptor read only as data"),
            "{e}"
        );
    }

    #[test]
    fn descriptor_registers_reused_after_a_masked_image_store() {
        // An image_store in an exec-masked tail is the only use of its
        // descriptor load; after the join the descriptor's registers hold
        // scratch (v_readfirstlane, then bit packing), alone or as the low
        // half of a 64-bit shift whose high half is still a descriptor
        // dword.
        let layout = || vec![root(RootType::Table, 0, 0)];
        let head = [
            0xBE81_0380, // s_mov_b32 s1, 0
            0xF40C_0100,
            0xFA00_0000, // s_load_dwordx8 s[4:11], s[0:1], 0x0
            0x7D88_0084, // v_cmp_gt_u32 vcc_lo, 4, v0
            0xBE82_3C6A, // s_and_saveexec_b32 s2, vcc_lo
            0xBF88_0002, // s_cbranch_execz -> past the store
            0xF020_1F08,
            0x0001_0100, // image_store v[1:4], v[0:1], s[4:11] dmask:0xf dim:SQ_RSRC_IMG_2D unorm
        ];
        let packed = [
            0x887E_027E, // s_or_b32 exec_lo, exec_lo, s2
            0x7E08_0500, // v_readfirstlane_b32 s4, v0
            0x7E0A_0501, // v_readfirstlane_b32 s5, v1
            0x8704_FF04,
            0x0000_01FF, // s_and_b32 s4, s4, 0x1ff
            0x8F05_8905, // s_lshl_b32 s5, s5, 9
            0x8804_0504, // s_or_b32 s4, s4, s5
            0x7E0A_0204, // v_mov_b32 v5, s4
            0xD834_0000,
            0x0000_0500, // ds_write_b32 v0, v5
            S_ENDPGM,
        ];
        let wide = [
            0x887E_027E, // s_or_b32 exec_lo, exec_lo, s2
            0x7E08_0500, // v_readfirstlane_b32 s4, v0
            0x8F84_8904, // s_lshl_b64 s[4:5], s[4:5], 9
            0x7E0A_0204, // v_mov_b32 v5, s4
            0xD834_0000,
            0x0000_0500, // ds_write_b32 v0, v5
            S_ENDPGM,
        ];
        lower_with(&[&head[..], &[S_ENDPGM]].concat(), layout(), vec![]).unwrap();
        lower_with(&[&head[..], &packed].concat(), layout(), vec![]).unwrap();
        lower_with(&[&head[..], &wide].concat(), layout(), vec![]).unwrap();
    }

    #[test]
    fn a_scalar_carry_chain_reaches_the_select() {
        // The high dword of a 64-bit add or subtract, then a select on the
        // carry (borrow) the high half leaves in SCC, and a branch on it
        // (only code with an effect needs its operands).
        let add = [
            0xBE80_03C1, // s_mov_b32 s0, -1
            0xBE81_0380, // s_mov_b32 s1, 0
            0x8000_8100, // s_add_u32 s0, s0, 1
            0x8201_8001, // s_addc_u32 s1, s1, 0
            0x8502_0001, // s_cselect_b32 s2, s1, s0
            0xBF07_8002, // s_cmp_lg_u32 s2, 0
            0xBF85_0001, // s_cbranch_scc1 -> s_endpgm
            0xBE83_0380, // s_mov_b32 s3, 0
            S_ENDPGM,
        ];
        let sub = [
            0xBE80_0380, // s_mov_b32 s0, 0
            0xBE81_0380, // s_mov_b32 s1, 0
            0x8080_8100, // s_sub_u32 s0, s0, 1
            0x8281_8001, // s_subb_u32 s1, s1, 0
            0x8502_0001, // s_cselect_b32 s2, s1, s0
            0xBF07_8002, // s_cmp_lg_u32 s2, 0
            0xBF85_0001, // s_cbranch_scc1 -> s_endpgm
            0xBE83_0380, // s_mov_b32 s3, 0
            S_ENDPGM,
        ];
        for words in [add, sub] {
            lowers_rsrc2(&words, 0).unwrap();
        }
    }

    /// A dword load by index through a descriptor built around the root
    /// descriptor in s[0:1] with dword 1 bits `hi` and dword 3 `w3`, stored
    /// to LDS so the load is needed.
    fn indexed_root_load(hi: u32, w3: u32) -> Result<Lowered, crate::ShaderError> {
        let words = [
            0x8805_FF01,
            hi,          // s_or_b32 s5, s1, hi
            0xBE84_0300, // s_mov_b32 s4, s0
            0xBE86_03C1, // s_mov_b32 s6, -1
            0xBE87_03FF,
            w3, // s_mov_b32 s7, w3
            0xE030_2000,
            0x8001_0100, // buffer_load_dword v1, v0, s[4:7], 0 idxen
            0xD834_0000,
            0x0000_0100, // ds_write_b32 v0, v1
            S_ENDPGM,
        ];
        let root = RootParam {
            root: RootType::Descriptor,
            table_offset_dwords: 0,
            sgprs: 2,
            base_register: 0,
            user_data: 0,
            slot: 3,
        };
        // The container lists the root SRV without a stride.
        let used = UsedResource {
            kind: ResourceType::Srv,
            root: RootType::Descriptor,
            table_offset_dwords: 0,
            lower_bound: 0,
            stride: 0,
            dwords: 4,
            user_data: 0,
            slot: 3,
        };
        lower_with(&words, vec![root], vec![used])
    }

    #[test]
    fn indexed_loads_through_built_descriptors_use_their_stride() {
        // No used-resource entry gives a stride: the descriptor's own (12,
        // and 1 as vertex fetch builds them) addresses the root view raw.
        for stride in [12, 1] {
            let l = indexed_root_load(stride << 16, 0x1101_4FAC).unwrap();
            assert_eq!(l.root_srvs, [3]);
        }
    }

    #[test]
    fn indexed_loads_through_swizzled_or_lane_indexed_descriptors_stop() {
        let why = |r: Result<Lowered, crate::ShaderError>| match r {
            Err(crate::ShaderError::Unsupported { what, .. }) => what,
            r => panic!("{:?}", r.map(|l| l.root_srvs)),
        };
        assert_eq!(
            why(indexed_root_load(0x8010_0000, 0x1101_4FAC)),
            "a swizzled buffer"
        );
        assert_eq!(
            why(indexed_root_load(0x0010_0000, 0x1181_4FAC)),
            "a buffer indexed by lane"
        );
    }

    #[test]
    fn scalar_bit_tests_lower() {
        // Each tests one bit of a constant into SCC and branches on it (only
        // code with an effect needs its operands).
        let tail = [
            0xBF85_0001, // s_cbranch_scc1 -> s_endpgm
            0xBE84_0380, // s_mov_b32 s4, 0
            S_ENDPGM,
        ];
        let bit_tests: [&[u32]; 4] = [
            &[0xBE81_0387, 0xBF0C_8301], // s_mov_b32 s1, 7; s_bitcmp0_b32 s1, 3
            &[0xBE81_0387, 0xBF0D_8301], // s_mov_b32 s1, 7; s_bitcmp1_b32 s1, 3
            &[
                0xBE80_0387, // s_mov_b32 s0, 7
                0xBE81_0380, // s_mov_b32 s1, 0
                0xBF0E_8300, // s_bitcmp0_b64 s[0:1], 3
            ],
            &[
                0xBE80_0387, // s_mov_b32 s0, 7
                0xBE81_0380, // s_mov_b32 s1, 0
                0xBF0F_8300, // s_bitcmp1_b64 s[0:1], 3
            ],
        ];
        for head in bit_tests {
            let words: Vec<u32> = head.iter().chain(&tail).copied().collect();
            lowers_rsrc2(&words, 0).unwrap();
        }
    }

    /// An f16 (sign, 5-bit exponent, 10-bit fraction) as an f32; normal numbers only.
    fn f16_value(h: u32) -> f32 {
        let sign = if h >> 15 != 0 { -1.0 } else { 1.0 };
        let exponent = (h >> 10 & 0x1F) as i32 - 15;
        sign * (1.0 + (h & 0x3FF) as f32 / 1024.0) * 2f32.powi(exponent)
    }

    #[test]
    fn inline_float_constants_mean_the_same_in_f16() {
        // v_fma_mix and v_pack_b32_f16 read these as f16: 1.0 must be 0x3C00,
        // not the low half (0) of the f32 bits 0x3F800000.
        assert_eq!(inline_f16(242), 0x3C00);
        for code in 240..=248u8 {
            let f32_value = f32::from_bits(crate::resource::inline(code).unwrap());
            let f16_value = f16_value(inline_f16(code));
            assert!(
                (f32_value - f16_value).abs() <= f32_value.abs() * 1e-3,
                "inline constant {code}: f32 {f32_value}, f16 {f16_value}"
            );
        }
    }

    #[test]
    fn scalar_conditional_moves_lower() {
        // Each sets SCC, moves under it into a destination that already has a
        // value, and branches on the result (only code with an effect needs
        // its operands).
        let tail = [
            0xBF07_8002, // s_cmp_lg_u32 s2, 0
            0xBF85_0001, // s_cbranch_scc1 -> s_endpgm
            0xBE84_0380, // s_mov_b32 s4, 0
            S_ENDPGM,
        ];
        let b32 = [
            0xBE82_0387, // s_mov_b32 s2, 7
            0xBE81_0381, // s_mov_b32 s1, 1
            0xBF07_8001, // s_cmp_lg_u32 s1, 0
            0xBE82_0501, // s_cmov_b32 s2, s1
        ];
        let k32 = [
            0xBE82_0387, // s_mov_b32 s2, 7
            0xBE81_0381, // s_mov_b32 s1, 1
            0xBF07_8001, // s_cmp_lg_u32 s1, 0
            0xB102_0005, // s_cmovk_i32 s2, 5
        ];
        let b64 = [
            0xBE82_0387, // s_mov_b32 s2, 7
            0xBE83_0387, // s_mov_b32 s3, 7
            0xBE80_0381, // s_mov_b32 s0, 1
            0xBE81_0381, // s_mov_b32 s1, 1
            0xBF07_8000, // s_cmp_lg_u32 s0, 0
            0xBE82_0600, // s_cmov_b64 s[2:3], s[0:1]
        ];
        for head in [&b32[..], &k32[..], &b64[..]] {
            let words: Vec<u32> = head.iter().chain(&tail).copied().collect();
            lowers(&words).unwrap();
        }
    }

    #[test]
    fn signed_names_of_wrapping_integer_operations_match_the_unsigned_ones() {
        // The low 32 bits of a sum, difference or product do not depend on
        // the sign, so the _i32 forms lower exactly as their _u32 twins.
        for (signed, unsigned) in [
            (0x37F, 0x125), // v_add_nc_i32, v_add_nc_u32
            (0x376, 0x126), // v_sub_nc_i32, v_sub_nc_u32
            (0x16B, 0x169), // v_mul_lo_i32, v_mul_lo_u32
        ] {
            let a = lower_ps(vop3(signed, 0, 2, 259, 129)).unwrap();
            let b = lower_ps(vop3(unsigned, 0, 2, 259, 129)).unwrap();
            assert_eq!(a, b, "opcode {signed:#x}");
        }
    }

    #[test]
    fn sixteen_bit_signed_add_and_sub_lower_with_half_selects() {
        // v_add_nc_i16 and v_sub_nc_i16, also reading the high halves.
        for op in [0x30D, 0x30E] {
            for opsel in [0, 1 << 11, 1 << 12 | 1 << 14] {
                lower_ps(vop3(op, opsel, 2, 259, 259)).unwrap();
            }
        }
    }

    #[test]
    fn clamped_signed_operations_are_refused_not_miscompiled() {
        // Clamping saturates; wrapping them would give other values.
        let clamp = 1 << 15;
        for op in [0x37F, 0x376, 0x16B, 0x30D, 0x30E] {
            let err = lower_ps(vop3(op, clamp, 2, 259, 129)).unwrap_err();
            assert!(
                matches!(err, ShaderError::Unsupported { what, .. } if what.contains("modifier")),
                "{op:#x}: {err:?}"
            );
        }
    }

    #[test]
    fn sub_dword_lds_names() {
        let s = |write, bits, signed, d16| {
            Some(SubDword {
                write,
                bits,
                signed,
                d16,
            })
        };
        assert_eq!(SubDword::of("ds_read_i8"), s(false, 8, true, None));
        assert_eq!(SubDword::of("ds_read_u16"), s(false, 16, false, None));
        assert_eq!(
            SubDword::of("ds_read_u8_d16"),
            s(false, 8, false, Some(false))
        );
        assert_eq!(
            SubDword::of("ds_read_i16_d16_hi"),
            s(false, 16, true, Some(true))
        );
        assert_eq!(SubDword::of("ds_write_b8"), s(true, 8, false, None));
        assert_eq!(
            SubDword::of("ds_write_b16_d16_hi"),
            s(true, 16, false, Some(true))
        );
        for name in [
            "ds_read_b32",
            "ds_read2_b32",
            "ds_write_b64",
            "ds_write_b8_d16",
            "ds_read_u8_d32",
            "ds_add_u32",
        ] {
            assert_eq!(SubDword::of(name), None, "{name}");
        }
    }

    #[test]
    fn scalar_pack_extend_and_absdiff_lower() {
        // Each computes into s2 from two constants and branches on it (only
        // code with an effect needs its operands).
        let head = [
            0xBE80_0387, // s_mov_b32 s0, 7
            0xBE81_0383, // s_mov_b32 s1, 3
        ];
        let tail = [
            0xBF07_8002, // s_cmp_lg_u32 s2, 0
            0xBF85_0001, // s_cbranch_scc1 -> s_endpgm
            0xBE84_0380, // s_mov_b32 s4, 0
            S_ENDPGM,
        ];
        let ops = [
            0x9602_0100, // s_absdiff_i32 s2, s0, s1
            0xBE82_1901, // s_sext_i32_i8 s2, s1
            0xBE82_1A01, // s_sext_i32_i16 s2, s1
            0x9902_0100, // s_pack_ll_b32_b16 s2, s0, s1
            0x9982_0100, // s_pack_lh_b32_b16 s2, s0, s1
            0x9A02_0100, // s_pack_hh_b32_b16 s2, s0, s1
        ];
        for op in ops {
            let words: Vec<u32> = head.iter().chain(&[op]).chain(&tail).copied().collect();
            lowers(&words).unwrap();
        }
    }

    #[test]
    fn cube_descriptor_dwords_read_as_data() {
        // A cube image's descriptor (a table range at s0) is loaded, sized
        // with image_get_resinfo, and its depth dword (4: the six faces, so
        // 5) is read as a number and branched on.
        let table = || vec![root(RootType::Table, 0, 0)];
        let words = [
            0xBE81_0380, // s_mov_b32 s1, 0
            0xF40C_0100,
            0xFA00_0000, // s_load_dwordx8 s[4:11], s[0:1], null
            0x7E04_0480, // v_mov_b32 v2, 0
            0xF038_0318,
            0x0001_0402, // image_get_resinfo v[4:5], v2, s[4:11] dim:SQ_RSRC_IMG_CUBE
            0xBF06_8508, // s_cmp_eq_u32 s8, 5
            0xBF85_0001, // s_cbranch_scc1 -> s_endpgm
            0xBE84_0380, // s_mov_b32 s4, 0
            S_ENDPGM,
        ];
        lower_with(&words, table(), vec![]).unwrap();
        // The same with a 2D image (dim 1) also lowers.
        let mut flat = words;
        flat[4] = 0xF038_0308;
        lower_with(&flat, table(), vec![]).unwrap();
    }

    #[test]
    fn a_descriptor_selected_by_scc_picks_its_heap_index() {
        // Two buffer descriptors loaded from the table at s0, one picked by
        // a root constant (s1) with s_cselect_b64 per half, and loaded
        // through by index and stored to LDS so the load is needed.
        let words = [
            0xBE84_0300, // s_mov_b32 s4, s0
            0xBE85_0380, // s_mov_b32 s5, 0
            0xF408_0202,
            0xFA00_0000, // s_load_dwordx4 s[8:11], s[4:5], null
            0xF408_0302,
            0xFA00_0010, // s_load_dwordx4 s[12:15], s[4:5], 0x10
            0xBF06_8001, // s_cmp_eq_u32 s1, 0
            0x8590_0C08, // s_cselect_b64 s[16:17], s[8:9], s[12:13]
            0x8592_0E0A, // s_cselect_b64 s[18:19], s[10:11], s[14:15]
            0xE030_2000,
            0x8004_0100, // buffer_load_dword v1, v0, s[16:19], 0 idxen
            0xD834_0000,
            0x0000_0100, // ds_write_b32 v0, v1
            S_ENDPGM,
        ];
        let layout = || vec![root(RootType::Table, 0, 0), root(RootType::Constants, 1, 1)];
        // Both are structured buffers of dwords in the table's range.
        let used = || {
            vec![UsedResource {
                kind: ResourceType::Srv,
                root: RootType::Table,
                table_offset_dwords: 0,
                lower_bound: 0,
                stride: 4,
                dwords: 4,
                user_data: 0,
                slot: 0,
            }]
        };
        lower_with(&words, layout(), used()).unwrap();
    }

    /// Tables of 2D textures at s0 and samplers at s1.
    fn texture_tables() -> (Vec<RootParam>, Vec<UsedResource>) {
        let range = |kind, user_data, slot| UsedResource {
            kind,
            root: RootType::Table,
            table_offset_dwords: 0,
            lower_bound: 0,
            stride: 0,
            dwords: if kind == ResourceType::Sampler { 4 } else { 8 },
            user_data,
            slot,
        };
        (
            vec![root(RootType::Table, 0, 0), root(RootType::Table, 1, 1)],
            vec![
                range(ResourceType::Srv, 0, 0),
                range(ResourceType::Sampler, 1, 1),
            ],
        )
    }

    #[test]
    fn waterfall_loops_over_per_lane_descriptors_sample_with_each_lanes_own() {
        use crate::resource::tests::{
            WATERFALL_BRANCHES_WAVE32, WATERFALL_CARRIED_WAVE32, WATERFALL_FETCHED_WAVE32,
            WATERFALL_FETCHED_WAVE64, WATERFALL_PICKED_WAVE32, WATERFALL_REPLACED_WAVE32,
        };
        for (words, wave64) in [
            (WATERFALL_FETCHED_WAVE32, false),
            (WATERFALL_FETCHED_WAVE64, true),
            (WATERFALL_PICKED_WAVE32, false),
            (WATERFALL_CARRIED_WAVE32, false),
            (WATERFALL_BRANCHES_WAVE32, false),
            (WATERFALL_REPLACED_WAVE32, false),
        ] {
            let (layout, used) = texture_tables();
            let l = lower_wave(words, layout, used, wave64).unwrap();
            // The loop's reads of the first lane became each lane's own
            // dwords: no wave operation is left.
            assert_eq!(l.nonuniform_handles, 1);
            assert!(!l.wave_ops);
        }
    }

    #[test]
    fn waterfall_loops_over_descriptors_from_buffer_loads_sample_with_each_lanes_own() {
        let words = crate::resource::tests::waterfall_buffer_wave32();
        let (layout, used) = texture_tables();
        let l = lower_wave(&words, layout, used, false).unwrap();
        assert_eq!(l.nonuniform_handles, 1);
        assert!(!l.wave_ops);
    }

    #[test]
    fn per_lane_descriptors_loaded_under_a_partial_exec_fail() {
        // The if/else without its s_cbranch_execz: both branches run, each
        // loading for its lanes only, so v[4:11] after them is the second
        // load only in the else lanes.
        let mut words = crate::resource::tests::WATERFALL_BRANCHES_WAVE32.to_vec();
        for k in [12, 19] {
            assert_eq!(words[k] >> 16, 0xBF88);
            words[k] = 0xBF80_0000; // s_nop 0
        }
        let (layout, used) = texture_tables();
        let Err(e) = lower_wave(&words, layout, used, false) else {
            panic!("lowered");
        };
        assert!(
            matches!(
                e,
                crate::ShaderError::Unsupported {
                    what: "a per-lane descriptor loaded under a partial EXEC",
                    ..
                }
            ),
            "{e:?}"
        );
    }

    /// A table at s0 holding structured buffers of dwords.
    fn dword_buffers() -> Vec<UsedResource> {
        vec![UsedResource {
            kind: ResourceType::Srv,
            root: RootType::Table,
            table_offset_dwords: 0,
            lower_bound: 0,
            stride: 4,
            dwords: 4,
            user_data: 0,
            slot: 0,
        }]
    }

    #[test]
    fn descriptors_loaded_through_pointers_kept_in_phis() {
        // A buffer descriptor loaded from the table at s0 through a pointer
        // a root constant (s1) picks by a branch, then loaded through by
        // index and stored to LDS so the load is needed.
        let picked = [
            0xBF06_8001, // s_cmp_eq_u32 s1, 0
            0xBF85_0002, // s_cbranch_scc1 -> 0x10
            0x8004_9000, // s_add_u32 s4, s0, 16
            0xBF82_0001, // s_branch -> 0x14
            0x8004_A000, // 0x10: s_add_u32 s4, s0, 32
            0xBE85_0380, // 0x14: s_mov_b32 s5, 0
            0xF408_0202,
            0xFA00_0000, // s_load_dwordx4 s[8:11], s[4:5], null
            0xE030_2000,
            0x8002_0100, // buffer_load_dword v1, v0, s[8:11], 0 idxen
            0xD834_0000,
            0x0000_0100, // ds_write_b32 v0, v1
            S_ENDPGM,
        ];
        // The same through a pointer advanced over four descriptors.
        let advanced = [
            0xBE84_0300, // s_mov_b32 s4, s0
            0xBE85_0380, // s_mov_b32 s5, 0
            0xBE86_0380, // s_mov_b32 s6, 0
            0xF408_0202,
            0xFA00_0000, // 0xc: s_load_dwordx4 s[8:11], s[4:5], null
            0xE030_2000,
            0x8002_0100, // buffer_load_dword v1, v0, s[8:11], 0 idxen
            0xD834_0000,
            0x0000_0100, // ds_write_b32 v0, v1
            0x8004_9004, // s_add_u32 s4, s4, 16
            0x8006_8106, // s_add_u32 s6, s6, 1
            0xBF07_8406, // s_cmp_lg_u32 s6, 4
            0xBF85_FFF6, // s_cbranch_scc1 -> 0xc
            S_ENDPGM,
        ];
        let layout = || vec![root(RootType::Table, 0, 0), root(RootType::Constants, 1, 1)];
        lower_with(&picked, layout(), dword_buffers()).unwrap();
        lower_with(&advanced, layout(), dword_buffers()).unwrap();
    }

    #[test]
    fn data_read_through_a_pointer_into_a_root_descriptor_stops() {
        // A root descriptor's address (s[0:1]) advanced in a loop and read
        // through: its address is no value the translation has, so the
        // offset from it is unknown.
        let words = [
            0xBE82_0300, // s_mov_b32 s2, s0
            0xBE83_0301, // s_mov_b32 s3, s1
            0xF400_0101,
            0xFA00_0000, // 0x8: s_load_dword s4, s[2:3], null
            0x8002_9002, // s_add_u32 s2, s2, 16
            0x8203_8003, // s_addc_u32 s3, s3, 0
            0xBF06_8004, // s_cmp_eq_u32 s4, 0
            0xBF84_FFFA, // s_cbranch_scc0 -> 0x8
            S_ENDPGM,
        ];
        let e = lower_with(&words, vec![root(RootType::Descriptor, 0, 3)], vec![])
            .err()
            .unwrap();
        assert!(
            e.to_string()
                .contains("an advanced pointer into a root descriptor"),
            "{e}"
        );
    }

    #[test]
    fn scalar_zero_counts_and_bit_scans_lower() {
        // s_bcnt0_i32_b32/_b64, s_ff0_i32_b32/_b64, s_flbit_i32_b64 and
        // s_flbit_i32_i64 of s0 (or the pair s[0:1]), with a branch on the
        // result so the instruction has an effect.
        for op in [0x0D, 0x0E, 0x11, 0x12, 0x16, 0x18] {
            let words = [
                0xBE80_03C1,           // s_mov_b32 s0, -1
                0xBE81_0380,           // s_mov_b32 s1, 0
                0xBE82_0000 | op << 8, // <op> s2, s0 (s[0:1])
                0xBF07_8002,           // s_cmp_lg_u32 s2, 0
                0xBF85_0001,           // s_cbranch_scc1 -> s_endpgm
                0xBE83_0380,           // s_mov_b32 s3, 0
                S_ENDPGM,
            ];
            lowers(&words).unwrap_or_else(|e| panic!("SOP1 {op:#x}: {e}"));
        }
        // The zero counts also leave "result != 0" in SCC.
        for op in [0x0D, 0x0E] {
            let words = [
                0xBE80_03C1,           // s_mov_b32 s0, -1
                0xBE81_0380,           // s_mov_b32 s1, 0
                0xBE82_0000 | op << 8, // s_bcnt0_i32_b32/_b64 s2, s0 (s[0:1])
                0xBF85_0001,           // s_cbranch_scc1 -> s_endpgm
                0xBE83_0380,           // s_mov_b32 s3, 0
                S_ENDPGM,
            ];
            lowers(&words).unwrap_or_else(|e| panic!("SOP1 {op:#x} SCC: {e}"));
        }
    }

    #[test]
    fn private_memory_is_as_large_as_its_addresses_reach() {
        let bytes = |words: &[u32]| {
            let code: Vec<u8> = words.iter().flat_map(|w| w.to_le_bytes()).collect();
            super::scratch_bytes(&ir::build(&code, false, 256).unwrap())
        };
        let store = [
            0x7E02_0280, // v_mov_b32 v1, 0
            0xDC70_401C, // scratch_store_dword v1, v2, off offset:28
            0x007D_0201,
        ];
        let indexed = [
            0x3606_0087, // v_and_b32 v3, 7, v0
            0x3406_0682, // v_lshlrev_b32 v3, 2, v3
            0xDC30_4000, // scratch_load_dword v4, v3, off
            0x047D_0003,
        ];
        assert_eq!(bytes(&[&store[..], &indexed, &[S_ENDPGM]].concat()), 32);
        // An address the code does not bound (the thread id).
        let free = [0xDC30_4000, 0x047D_0000]; // scratch_load_dword v4, v0, off
        assert_eq!(
            bytes(&[&store[..], &free, &[S_ENDPGM]].concat()),
            super::SCRATCH_CAP
        );
        let far = [
            0x7E02_02FF, // v_mov_b32 v1, 0x100000
            0x0010_0000,
            0xDC70_4000, // scratch_store_dword v1, v2, off
            0x007D_0201,
            S_ENDPGM,
        ];
        assert_eq!(bytes(&far), super::SCRATCH_MAX);
    }

    #[test]
    fn generic_flat_memory_is_refused() {
        // flat_store_dword v[0:1], v2: the address may be private, shared
        // or global memory.
        let e = lowers(&[0xDC70_0000, 0x007D_0200, S_ENDPGM]).unwrap_err();
        assert!(
            matches!(
                e,
                crate::ShaderError::Unsupported {
                    what: "flat memory",
                    ..
                }
            ),
            "{e}"
        );
    }

    #[test]
    fn a_missing_value_stays_missing_through_two_live_joins() {
        let mut words = [
            0xF490_0180,
            0,           // s_memtime s[6:7]
            0xBF06_8000, // s_cmp_eq_u32 s0, 0 (s0 = group id X)
            0xBF85_0001, // skip the first overwrite
            0xBE86_0380, // s_mov_b32 s6, 0
            0xBF06_8101, // s_cmp_eq_u32 s1, 1 (s1 = group id Y)
            0xBF85_0001, // skip the second overwrite
            0xBE86_0381, // s_mov_b32 s6, 1
            0xBF07_8006, // s_cmp_lg_u32 s6, 0
            0xBF85_0001, // use the result to branch
            0xBE83_0380,
            S_ENDPGM,
        ];
        let group_ids_xy = 3 << 7;
        // Compare to a direct read so future timestamp support works in both cases.
        let refusal = |code: &[u32]| match lowers_rsrc2(code, group_ids_xy) {
            Ok(()) => None,
            Err(crate::ShaderError::Unsupported { what, .. }) => Some(what),
            Err(error) => panic!("{error}"),
        };
        let direct = [&words[..2], &words[8..]].concat();
        assert_eq!(refusal(&words), refusal(&direct));
        words[8] = 0xBF07_8000; // read the group id instead; the timestamp is dead
        lowers_rsrc2(&words, group_ids_xy).expect("dead timestamp");
    }

    #[test]
    fn buffer_formats_by_number() {
        // The numbers the disassembler names 8_8_8_8_UNORM, 16_16_SNORM,
        // 2_10_10_10_UNORM, 16_FLOAT, 32_32_32_32_FLOAT.
        assert_eq!(buffer_format(56), Some((&[8, 8, 8, 8][..], Numeric::Unorm)));
        assert_eq!(buffer_format(24), Some((&[16, 16][..], Numeric::Snorm)));
        assert_eq!(
            buffer_format(50),
            Some((&[10, 10, 10, 2][..], Numeric::Unorm))
        );
        assert_eq!(buffer_format(13), Some((&[16][..], Numeric::Float)));
        assert_eq!(
            buffer_format(77),
            Some((&[32, 32, 32, 32][..], Numeric::Float))
        );
        // Invalid, past the table.
        assert_eq!(buffer_format(0), None);
        assert_eq!(buffer_format(78), None);
    }

    /// Lowers a pixel program that loads a sampler and an image from two
    /// tables, samples v[0:3] from the address at v2 on with the two-dword
    /// instruction `sample`, and exports the result.
    fn lower_sample(sample: [u32; 2]) -> Result<Vec<u8>, crate::ShaderError> {
        let words = [
            0xBE82_0309, // s_mov_b32 s2, s9
            0xBE89_0380, // s_mov_b32 s9, 0
            0xBE83_0380, // s_mov_b32 s3, 0
            0xF408_0204,
            0xFA00_0000, // s_load_dwordx4 s[8:11], s[8:9], null
            0xF40C_0001,
            0xFA00_0000, // s_load_dwordx8 s[0:7], s[2:3], null
            sample[0],
            sample[1],
            0xF800_180F,
            0x0302_0100, // exp mrt0, v0, v1, v2, v3 done vm
            0xBF81_0000, // s_endpgm
        ];
        let table = |user_data, slot| RootParam {
            root: RootType::Table,
            table_offset_dwords: 0,
            sgprs: 1,
            base_register: 0,
            user_data,
            slot,
        };
        let used = |kind, dwords, user_data, slot| UsedResource {
            kind,
            root: RootType::Table,
            table_offset_dwords: 0,
            lower_bound: 0,
            stride: 0,
            dwords,
            user_data,
            slot,
        };
        let native = NativeProgram {
            stage: HwStage::Ps,
            wave64: false,
            vgprs: 256,
            layout: vec![table(8, 0), table(9, 1)],
            used: vec![
                used(ResourceType::Sampler, 4, 8, 0),
                used(ResourceType::Srv, 8, 9, 1),
            ],
            io: StageIo::Ps {
                inputs: Vec::new(),
                input_addr: 0,
                db_shader_control: 0,
                patches: Vec::new(),
            },
            code: words.iter().flat_map(|w| w.to_le_bytes()).collect(),
        };
        let p = crate::ir::build(&native.code, native.wave64, native.vgprs)?;
        let res = crate::resource::track(&p, &native.layout, native.stage)?;
        let io = crate::io::raise(&p, &native)?;
        let st = crate::structure::structure(&p)?;
        let target = SigElement {
            name: "SV_Target".into(),
            index: 0,
            kind: 16,
            comp: 9,
            interp: 0,
            row: Some(0),
            col: 0,
            cols: 4,
            usage: 0xF,
            half: false,
        };
        lower(&Parts {
            native: &native,
            p: &p,
            st: &st,
            io: &io,
            res: &res,
            inputs: Vec::new(),
            outputs: vec![target],
            prims: Vec::new(),
            mesh_stride: 0,
            target: crate::emit::Target::ANY,
            psv: None,
            console_cf: false,
            per_lane_only: false,
        })
        .map(|l| l.dxil)
    }

    /// MIMG words: opcode, all four channels, the dimension; data v0,
    /// address v2, image s[0:7], sampler s[8:11].
    fn mimg(opcode: u32, dim: u32) -> [u32; 2] {
        [0xF000_0F00 | opcode << 18 | dim << 3, 0x0040_0002]
    }

    const SAMPLE_D: u32 = 0x22;
    const SAMPLE_D_CL: u32 = 0x23;
    const DIM_2D: u32 = 1;
    const DIM_CUBE: u32 = 3;

    #[test]
    fn gradient_samples_lower_on_planes_and_cubes() {
        for (opcode, dim) in [
            (SAMPLE_D, DIM_2D),
            (SAMPLE_D, DIM_CUBE),
            (SAMPLE_D_CL, DIM_2D),
            (SAMPLE_D_CL, DIM_CUBE),
        ] {
            if let Err(e) = lower_sample(mimg(opcode, dim)) {
                panic!("opcode {opcode:#x} on dimension {dim}: {e:?}");
            }
        }
    }

    #[test]
    fn host_wave32_mask_ops_use_both_ballot_halves() {
        use std::collections::HashMap;
        let words = [
            0xBE80_0F7E, // s_bcnt1_i32_b32 s0, exec_lo
            0xBE81_137E, // s_ff1_i32_b32 s1, exec_lo
            0xD765_0002,
            0x0001_007E, // v_mbcnt_lo_u32_b32 v2, exec_lo, 0
            0xD765_0003,
            0x0001_00C1, // v_mbcnt_lo_u32_b32 v3, -1, 0
            0xBF06_807E, // s_cmp_eq_u32 exec_lo, 0
            0x8504_0100, // s_cselect_b32 s4, s0, s1
            0x7E00_0204, // v_mov_b32 v0, s4
            0x7E02_0201, // v_mov_b32 v1, s1
            0xF800_180F,
            0x0302_0100, // exp mrt0, v0, v1, v2, v3 done vm
            S_ENDPGM,
        ];
        let code: Vec<u8> = words.iter().flat_map(|w| w.to_le_bytes()).collect();
        let native = NativeProgram {
            stage: HwStage::Ps,
            wave64: false,
            vgprs: 8,
            layout: vec![],
            used: vec![],
            io: StageIo::Ps {
                inputs: vec![],
                input_addr: 0,
                db_shader_control: 0,
                patches: vec![],
            },
            code,
        };
        let p = ir::build(&native.code, false, 8).unwrap();
        let res = resource::track(&p, &native.layout, native.stage).unwrap();
        let io = io::raise(&p, &native).unwrap();
        let st = structure::structure(&p).unwrap();
        let parts = Parts {
            native: &native,
            p: &p,
            st: &st,
            io: &io,
            res: &res,
            inputs: vec![],
            outputs: vec![],
            prims: vec![],
            mesh_stride: 0,
            target: crate::emit::Target::ANY,
            psv: None,
            console_cf: false,
            per_lane_only: false,
        };
        let bad = HashMap::new();
        let mut l = super::Lower::new(&parts, &bad, false);
        assert!(l.lower_ir().is_ok());
        assert_eq!(l.mask_data, super::MaskData::Host);
        assert_eq!(l.s.wave_size, None);
        let &(_lo, hi) = l.ballots.values().next().unwrap();
        assert!(
            l.s.m.uses_of(hi) >= 4,
            "expected high ballot half to be used by s_bcnt1_i32_b32, s_ff1_i32_b32, v_mbcnt_lo_u32_b32, and s_cmp_eq_u32 (>= 4 uses), got {}",
            l.s.m.uses_of(hi)
        );
    }

    /// Lowers a one-thread compute program that copies four root constants
    /// (s0..s3) into v1..v4, runs `body`, and stores v0 to a raw buffer from a
    /// descriptor table, so the vector instructions in `body` have an effect.
    fn lowers_valu(body: &[u32]) -> Result<(), crate::ShaderError> {
        let mut words = vec![
            0xBE85_0380, // s_mov_b32 s5, 0
            0xF408_0202, // s_load_dwordx4 s[8:11], s[4:5], 0
            0xFA00_0000,
        ];
        // v_mov_b32 v1..v4, s0..s3
        words.extend((0..4u32).map(|i| 0x7E00_0200 | (i + 1) << 17 | i));
        words.extend_from_slice(body);
        words.extend_from_slice(&[
            0xE070_0000, // buffer_store_dword v0, off, s[8:11], 0
            0x8002_0000,
            S_ENDPGM,
        ]);
        let code: Vec<u8> = words.iter().flat_map(|w| w.to_le_bytes()).collect();
        let param = |root, sgprs, user_data, slot| RootParam {
            root,
            table_offset_dwords: 0,
            sgprs,
            base_register: 0,
            user_data,
            slot,
        };
        let native = NativeProgram {
            stage: HwStage::Cs,
            wave64: false,
            vgprs: 8,
            layout: vec![
                param(RootType::Constants, 4, 0, 0),
                param(RootType::Table, 1, 4, 1),
            ],
            used: vec![UsedResource {
                kind: ResourceType::Uav,
                root: RootType::Table,
                table_offset_dwords: 0,
                lower_bound: 0,
                stride: 0,
                dwords: 4,
                user_data: 4,
                slot: 1,
            }],
            io: StageIo::Cs(ComputeIo {
                rsrc2: 5 << 1, // five user SGPRs, s0..s4
                threads: [1, 1, 1],
            }),
            code,
        };
        let p = ir::build(&native.code, false, native.vgprs).unwrap();
        let res = resource::track(&p, &native.layout, native.stage).unwrap();
        let io = io::raise(&p, &native).unwrap();
        let st = structure::structure(&p).unwrap();
        lower(&Parts {
            native: &native,
            p: &p,
            st: &st,
            io: &io,
            res: &res,
            inputs: vec![],
            outputs: vec![],
            prims: vec![],
            mesh_stride: 0,
            target: crate::emit::Target::ANY,
            psv: None,
            console_cf: false,
            per_lane_only: false,
        })
        .map(|_| ())
    }

    #[test]
    fn the_24_bit_multiplies_lower_both_halves() {
        // VOP2 `op v0, v1, v2`: v_mul_i32_i24 and v_mul_u32_u24 (the low
        // dword) and v_mul_hi_i32_i24 and v_mul_hi_u32_u24 (the high one).
        for op in [0x09u32, 0x0A, 0x0B, 0x0C] {
            let word = op << 25 | 2 << 9 | 0x101;
            lowers_valu(&[word]).unwrap_or_else(|e| panic!("VOP2 {op:#x}: {e}"));
        }
    }

    #[test]
    fn sums_of_absolute_differences_lower() {
        // VOP3 `op v0, v1, v2, v3`: v_sad_u8, v_sad_hi_u8, v_sad_u16,
        // v_sad_u32 and v_msad_u8.
        for op in [0x15Au32, 0x15B, 0x15C, 0x15D, 0x171] {
            let words = [0xD400_0000 | op << 16, 0x101 | 0x102 << 9 | 0x103 << 18];
            lowers_valu(&words).unwrap_or_else(|e| panic!("VOP3 {op:#x}: {e}"));
        }
    }

    /// MIMG `op vdata, v[vaddr..], s[srsrc..srsrc+7]` on a 2D image.
    const fn mimg_2d(op: u8, dmask: u8, vdata: u8, vaddr: u8, srsrc: u8) -> [u32; 2] {
        [
            0x3C << 26 | (op as u32 & 0x7F) << 18 | (dmask as u32) << 8 | 1 << 3,
            ((srsrc as u32 >> 2) & 0x1F) << 16 | (vdata as u32) << 8 | vaddr as u32,
        ]
    }

    #[test]
    fn an_image_read_as_float_and_by_an_atomic_is_annotated_twice() {
        use crate::native::{ResourceType, RootParam, RootType, UsedResource};

        // One 2D image UAV from the descriptor table at s4, loaded (four
        // float channels) and then atomically minimised in the same block.
        let mut words = vec![
            0xBE85_0380, // s_mov_b32 s5, 0
            // s_load_dwordx8 s[8:15], s[4:5], 0
            0x3D << 26 | 0x03 << 18 | 8 << 6 | 4 >> 1,
            125 << 25,
            0x7E00_0280, // v_mov_b32 v0, 0
            0x7E02_0280, // v_mov_b32 v1, 0
        ];
        words.extend(mimg_2d(0x00, 0xF, 2, 0, 8)); // image_load v[2:5], v[0:1], s[8:15]
        words.extend(mimg_2d(0x15, 0x1, 2, 0, 8)); // image_atomic_umin v2, v[0:1], s[8:15]
        words.push(S_ENDPGM);
        let code: Vec<u8> = words.iter().flat_map(|w| w.to_le_bytes()).collect();
        let native = NativeProgram {
            stage: HwStage::Cs,
            wave64: false,
            vgprs: 8,
            layout: vec![RootParam {
                root: RootType::Table,
                table_offset_dwords: 0,
                sgprs: 1,
                base_register: 0,
                user_data: 4,
                slot: 0,
            }],
            used: vec![UsedResource {
                kind: ResourceType::Uav,
                root: RootType::Table,
                table_offset_dwords: 0,
                lower_bound: 0,
                stride: 0,
                dwords: 8,
                user_data: 4,
                slot: 0,
            }],
            io: StageIo::Cs(ComputeIo {
                rsrc2: 5 << 1, // 5 user SGPRs: s0..s4
                threads: [1, 1, 1],
            }),
            code,
        };
        let p = ir::build(&native.code, false, native.vgprs).unwrap();
        let res = resource::track(&p, &native.layout, native.stage).unwrap();
        let io = io::raise(&p, &native).unwrap();
        let st = structure::structure(&p).unwrap();
        let lowered = lower(&Parts {
            native: &native,
            p: &p,
            st: &st,
            io: &io,
            res: &res,
            inputs: vec![],
            outputs: vec![],
            prims: vec![],
            mesh_stride: 0,
            target: crate::emit::Target::ANY,
            psv: None,
            console_cf: false,
            per_lane_only: false,
        })
        .unwrap();

        // DXIL resource properties: word 0 is the kind (Texture2D = 2) with
        // the UAV bit (0x1000); word 1 is the component type in the low byte
        // (U32 = 5, F32 = 9, DxilConstants.h `ComponentType`) and the
        // component count above it. D3D12 allows atomics (Interlocked*) on
        // integer UAVs only, so the atomic needs its own one-U32 annotation
        // next to the load's four-float one.
        const TEXTURE_2D_UAV: u32 = 0x1002;
        assert!(
            lowered.annotations.contains(&(TEXTURE_2D_UAV, 0x409)),
            "the load's annotation: {:x?}",
            lowered.annotations
        );
        assert!(
            lowered.annotations.contains(&(TEXTURE_2D_UAV, 0x105)),
            "the atomic's annotation: {:x?}",
            lowered.annotations
        );
    }

    #[test]
    fn packed_half_inputs_pair_unlisted_rows() {
        use super::super::container::SigElement;
        use super::half_inputs;
        use crate::native::{PsInput, StageIo, Usage};

        let sig = |row: u32, half: bool| SigElement {
            name: "TEXCOORD".into(),
            index: row,
            kind: 0,
            comp: 9,
            interp: 0,
            row: Some(row),
            col: 0,
            cols: 4,
            usage: 0,
            half,
        };
        let ps_in = |row: u8, attr: u8, pack2: bool, row2: u8| PsInput {
            usage: Usage::Generic,
            row,
            mask: 3,
            attr,
            flat: false,
            fp16: true,
            pack2,
            row2,
        };
        // Row 0 has row2 == row (0) and pairs with unlisted row 1;
        // row 2 explicitly names row2 == 4 (not stolen by row 0);
        // row 5 has row2 == row (5) and no remaining unlisted row after it.
        let inputs = vec![
            sig(0, true),
            sig(1, true),
            sig(2, true),
            sig(3, false),
            sig(4, true),
            sig(5, true),
        ];
        let io = StageIo::Ps {
            inputs: vec![
                ps_in(0, 0, true, 0),
                ps_in(2, 1, true, 4),
                ps_in(5, 3, true, 5),
            ],
            input_addr: 0,
            db_shader_control: 0,
            patches: vec![],
        };
        let map = half_inputs(&io, &inputs);
        assert_eq!(map.get(&0), Some(&(true, Some(1))));
        assert_eq!(map.get(&2), Some(&(true, Some(4))));
        assert_eq!(map.get(&5), Some(&(true, None)));

        // Duplicate entries for the same row keep the first entry and do not
        // consume an extra free row; an earlier unpaired row (0) must not steal
        // a free row (3) that comes after a later fp16 row (2).
        let inputs2 = vec![sig(0, true), sig(2, true), sig(3, true)];
        let io2 = StageIo::Ps {
            inputs: vec![
                ps_in(0, 0, true, 0),
                ps_in(0, 9, false, 0),
                ps_in(2, 1, true, 2),
            ],
            input_addr: 0,
            db_shader_control: 0,
            patches: vec![],
        };
        let map2 = half_inputs(&io2, &inputs2);
        assert_eq!(map2.get(&0), Some(&(true, None)));
        assert_eq!(map2.get(&2), Some(&(true, Some(3))));
    }

    #[test]
    fn packed_buffer_formats_by_number() {
        // LLVM's `UfmtGFX10`: 10_11_11 is 30-36, 11_11_10 37-43,
        // 10_10_10_2 44-49. Channels from the lowest bits: R11G11B10_FLOAT
        // is 10_11_11 (Mesa), 10_10_10_2 has the 2-bit channel first.
        assert_eq!(buffer_format(30), Some((&[11, 11, 10][..], Numeric::Unorm)));
        assert_eq!(buffer_format(36), Some((&[11, 11, 10][..], Numeric::Float)));
        assert_eq!(buffer_format(37), Some((&[10, 11, 11][..], Numeric::Unorm)));
        assert_eq!(buffer_format(43), Some((&[10, 11, 11][..], Numeric::Float)));
        assert_eq!(
            buffer_format(44),
            Some((&[2, 10, 10, 10][..], Numeric::Unorm))
        );
        assert_eq!(
            buffer_format(45),
            Some((&[2, 10, 10, 10][..], Numeric::Snorm))
        );
        assert_eq!(
            buffer_format(46),
            Some((&[2, 10, 10, 10][..], Numeric::Uscaled))
        );
        assert_eq!(
            buffer_format(47),
            Some((&[2, 10, 10, 10][..], Numeric::Sscaled))
        );
        assert_eq!(
            buffer_format(48),
            Some((&[2, 10, 10, 10][..], Numeric::Uint))
        );
        assert_eq!(
            buffer_format(49),
            Some((&[2, 10, 10, 10][..], Numeric::Sint))
        );
    }

    /// A half's value (sign, 5-bit exponent biased 15, 10-bit mantissa), as
    /// `legacyF16ToF32` widens it.
    fn half(h: u32) -> f32 {
        let (s, e, m) = ((h >> 15) & 1, (h >> 10) & 31, h & 1023);
        let v = match e {
            0 => m as f32 * 2f32.powi(-24),
            31 if m == 0 => f32::INFINITY,
            31 => f32::NAN,
            _ => (1024 + m) as f32 * 2f32.powi(e as i32 - 25),
        };
        if s == 1 { -v } else { v }
    }

    #[test]
    fn small_floats_widen_as_halves() {
        // Unsigned 11- and 10-bit floats: a 5-bit exponent biased 15 over a
        // 6- or 5-bit mantissa, no sign; values from the D3D/GL spec.
        let f11 = |x: u32| half(x << half_shift(11));
        let f10 = |x: u32| half(x << half_shift(10));
        assert_eq!(f11(0), 0.0);
        assert_eq!(f11(0x001), 2f32.powi(-20)); // smallest denormal
        assert_eq!(f11(0x03F), 63.0 * 2f32.powi(-20)); // largest denormal
        assert_eq!(f11(0x040), 2f32.powi(-14)); // smallest normal
        assert_eq!(f11(0x3C0), 1.0);
        assert_eq!(f11(0x3E0), 1.5);
        assert_eq!(f11(0x7BF), 65024.0); // largest
        assert_eq!(f11(0x7C0), f32::INFINITY);
        assert!(f11(0x7C1).is_nan());
        assert_eq!(f10(0x001), 2f32.powi(-19));
        assert_eq!(f10(0x020), 2f32.powi(-14));
        assert_eq!(f10(0x1E0), 1.0);
        assert_eq!(f10(0x3DF), 64512.0);
        assert_eq!(f10(0x3E0), f32::INFINITY);
        assert!(f10(0x3FF).is_nan());
        // A 16-bit float is a half already, sign included.
        assert_eq!(half(0xBC00 << half_shift(16)), -1.0);
    }

    /// From #305: 64-bit image atomics declare 64-bit integers and typed
    /// 64-bit atomics.
    #[test]
    fn image_atomics_64_bit_lower_with_typed_int64_flag() {
        use crate::native::{RootParam, RootType};

        let run = |mimg_w0: u32| {
            let words = [
                // s_mov_b32 s1, 0
                0xBE81_0380u32,
                // s_load_dwordx8 s[4:11], s[0:1], null
                0xF40C_0100,
                0xFA00_0000,
                // v_mov_b32 v0..v5, 0/1/2
                0x7E00_0280,
                0x7E02_0280,
                0x7E04_0281,
                0x7E06_0280,
                0x7E08_0282,
                0x7E0A_0280,
                // image_atomic_* (2D, unorm=1, glc=1) v[2..], v0, s[4:11]
                mimg_w0,
                0x0001_0200,
                S_ENDPGM,
            ];
            let code: Vec<u8> = words.iter().flat_map(|w| w.to_le_bytes()).collect();
            let native = NativeProgram {
                stage: HwStage::Cs,
                wave64: false,
                vgprs: 8,
                layout: vec![RootParam {
                    root: RootType::Table,
                    table_offset_dwords: 0,
                    sgprs: 1,
                    base_register: 0,
                    user_data: 0,
                    slot: 0,
                }],
                used: vec![],
                io: StageIo::Cs(ComputeIo {
                    rsrc2: 0,
                    threads: [1, 1, 1],
                }),
                code,
            };
            let p = ir::build(&native.code, false, 8).unwrap();
            let res = resource::track(&p, &native.layout, native.stage).unwrap();
            let io = io::raise(&p, &native).unwrap();
            let st = structure::structure(&p).unwrap();
            lower(&Parts {
                native: &native,
                p: &p,
                st: &st,
                io: &io,
                res: &res,
                inputs: vec![],
                outputs: vec![],
                prims: vec![],
                mesh_stride: 0,
                target: crate::emit::Target::ANY,
                psv: None,
                console_cf: false,
                per_lane_only: false,
            })
        };

        // image_atomic_add (op 0x11), dim=1 (2D), unorm=1, dmask=3, glc=1
        let add = run(0xF044_1388).unwrap();
        // image_atomic_cmpswap (op 0x10), dim=1 (2D), unorm=1, dmask=0xf, glc=1
        let cmp = run(0xF040_1F88).unwrap();
        for lowered in [add, cmp] {
            let sfi0 = crate::dxbc::Container::parse(&lowered.dxil)
                .unwrap()
                .part(crate::dxbc::FourCC::SFI0)
                .unwrap()
                .data;
            let features = u64::from_le_bytes(sfi0.try_into().unwrap());
            assert_ne!(features & 0x8000, 0, "INT64 feature bit missing");
            assert_ne!(
                features & 0x40_0000,
                0,
                "ATOMIC_INT64_ON_TYPED_RESOURCE feature bit missing"
            );
        }
    }

    #[test]
    fn cube_and_msaa_descriptor_data_lower() {
        use crate::native::{RootParam, RootType};

        for dim in [3u32, 6, 7] {
            // s_mov_b32 s1, 0
            // s_load_dwordx8 s[4:11], s[0:1], null
            // s_lshr_b32 s12, s7, 16        (reads descriptor word 3 as data)
            // s_cmp_lg_u32 s12, 0
            // s_cbranch_scc1 -> s_endpgm
            // s_mov_b32 s13, 0
            // v_mov_b32 v0..v3, 0
            // image_get_resinfo (dim) v0, v0, s[4:11]  (op 0x0e, dmask=1)
            // s_endpgm
            let mimg_w0 = 0xF000_0000 | (0x0E << 18) | (1 << 8) | (dim << 3);
            let words = [
                0xBE81_0380u32,
                0xF40C_0100,
                0xFA00_0000,
                0x900C_9007,
                0xBF07_800C,
                0xBF85_0001,
                0xBE8D_0380,
                0x7E00_0280,
                mimg_w0,
                0x0001_0000,
                S_ENDPGM,
            ];
            let code: Vec<u8> = words.iter().flat_map(|w| w.to_le_bytes()).collect();
            let native = NativeProgram {
                stage: HwStage::Cs,
                wave64: false,
                vgprs: 4,
                layout: vec![RootParam {
                    root: RootType::Table,
                    table_offset_dwords: 0,
                    sgprs: 1,
                    base_register: 0,
                    user_data: 0,
                    slot: 0,
                }],
                used: vec![],
                io: StageIo::Cs(ComputeIo {
                    rsrc2: 0,
                    threads: [1, 1, 1],
                }),
                code,
            };
            let p = ir::build(&native.code, false, 4).unwrap();
            let res = resource::track(&p, &native.layout, native.stage).unwrap();
            let io = io::raise(&p, &native).unwrap();
            let st = structure::structure(&p).unwrap();
            let out = lower(&Parts {
                native: &native,
                p: &p,
                st: &st,
                io: &io,
                res: &res,
                inputs: vec![],
                outputs: vec![],
                prims: vec![],
                mesh_stride: 0,
                target: crate::emit::Target::ANY,
                psv: None,
                console_cf: false,
                per_lane_only: false,
            });
            // Cube (3) and MSAA (6, 7) descriptor_data() all lower without failing
            // on "an image dimension (cube or MSAA)".
            out.unwrap();
        }
    }

    #[test]
    fn msaa_image_load_and_get_resinfo_lower() {
        use crate::native::{RootParam, RootType};

        for (dim, op) in [(6u32, 0x00u32), (6, 0x0E), (7, 0x00), (7, 0x0E)] {
            // s_mov_b32 s1, 0
            // s_load_dwordx8 s[4:11], s[0:1], null
            // v_mov_b32 v0..v3, 0
            // image_load (0x00) / image_get_resinfo (0x0e) v[0..3], v[0..3], s[4:11]
            // v_cmp_ne_u32 vcc, v0, 0
            // s_cbranch_vccz -> s_endpgm
            // s_mov_b32 s12, 0
            // s_endpgm
            let mimg_w0 = 0xF000_0000 | (op << 18) | (0xF << 8) | (dim << 3);
            let words = [
                0xBE81_0380u32,
                0xF40C_0100,
                0xFA00_0000,
                0x7E00_0280,
                0x7E02_0280,
                0x7E04_0280,
                0x7E06_0280,
                mimg_w0,
                0x0001_0000,
                0x7D9A_0080, // v_cmpx_ne_u32_e32 v0, 0 -> sets EXEC, effectful?
                0xBF81_0000,
            ];
            let code: Vec<u8> = words.iter().flat_map(|w| w.to_le_bytes()).collect();
            let native = NativeProgram {
                stage: HwStage::Cs,
                wave64: false,
                vgprs: 4,
                layout: vec![RootParam {
                    root: RootType::Table,
                    table_offset_dwords: 0,
                    sgprs: 1,
                    base_register: 0,
                    user_data: 0,
                    slot: 0,
                }],
                used: vec![],
                io: StageIo::Cs(ComputeIo {
                    rsrc2: 0,
                    threads: [1, 1, 1],
                }),
                code,
            };
            let p = ir::build(&native.code, false, 4).unwrap();
            let res = resource::track(&p, &native.layout, native.stage).unwrap();
            let io = io::raise(&p, &native).unwrap();
            let st = structure::structure(&p).unwrap();
            lower(&Parts {
                native: &native,
                p: &p,
                st: &st,
                io: &io,
                res: &res,
                inputs: vec![],
                outputs: vec![],
                prims: vec![],
                mesh_stride: 0,
                target: crate::emit::Target::ANY,
                psv: None,
                console_cf: false,
                per_lane_only: false,
            })
            .unwrap();
        }
    }

    #[test]
    fn partial_channel_image_store_lowers() {
        use crate::native::{RootParam, RootType};

        // image_store (op 0x08), dim=1 (2D), unorm=1
        // Test 32-bit partial masks (0x1, 0x3, 0x5) and d16 partial mask (0x7).
        for (dmask, d16) in [(0x1u32, false), (0x3, false), (0x5, false), (0x7, true)] {
            let mimg_w0 = 0xF000_0000
                | (0x08 << 18)
                | (u32::from(d16) << 15)
                | (dmask << 8)
                | (1 << 7)
                | (1 << 3);
            let words = [
                // s_mov_b32 s1, 0
                0xBE81_0380u32,
                // s_load_dwordx8 s[4:11], s[0:1], null
                0xF40C_0100,
                0xFA00_0000,
                // v_mov_b32 v0..v3, 0 (coords v0..v1, data v2..v3)
                0x7E00_0280,
                0x7E02_0280,
                0x7E04_0280,
                0x7E06_0280,
                // image_store v[2..], v[0..1], s[4:11]
                mimg_w0,
                0x0001_0200,
                S_ENDPGM,
            ];
            let code: Vec<u8> = words.iter().flat_map(|w| w.to_le_bytes()).collect();
            let native = NativeProgram {
                stage: HwStage::Cs,
                wave64: false,
                vgprs: 4,
                layout: vec![RootParam {
                    root: RootType::Table,
                    table_offset_dwords: 0,
                    sgprs: 1,
                    base_register: 0,
                    user_data: 0,
                    slot: 0,
                }],
                used: vec![],
                io: StageIo::Cs(ComputeIo {
                    rsrc2: 0,
                    threads: [1, 1, 1],
                }),
                code,
            };
            let p = ir::build(&native.code, false, 4).unwrap();
            let res = resource::track(&p, &native.layout, native.stage).unwrap();
            let io = io::raise(&p, &native).unwrap();
            let st = structure::structure(&p).unwrap();
            lower(&Parts {
                native: &native,
                p: &p,
                st: &st,
                io: &io,
                res: &res,
                inputs: vec![],
                outputs: vec![],
                prims: vec![],
                mesh_stride: 0,
                target: crate::emit::Target::ANY,
                psv: None,
                console_cf: false,
                per_lane_only: false,
            })
            .unwrap();
        }
    }

    #[test]
    fn partial_channel_typed_buffer_store_lowers() {
        use crate::native::{RootParam, RootType};

        // MUBUF buffer_store_format_{x,xy,xyz,d16_xyz} (ops 0x04, 0x05, 0x06, 0x86)
        // with idxen=1, vaddr=v0, vdata=v1, srsrc=s[4:7].
        for op in [0x04u32, 0x05, 0x06, 0x86] {
            let mubuf_w0 = 0xE000_2000 | (op << 18);
            let words = [
                // s_mov_b32 s1, 0
                0xBE81_0380u32,
                // s_load_dwordx4 s[4:7], s[0:1], null
                0xF408_0100,
                0xFA00_0000,
                // v_mov_b32 v0..v3, 0
                0x7E00_0280,
                0x7E02_0280,
                0x7E04_0280,
                0x7E06_0280,
                // buffer_store_format_* v[1..], v0, s[4:7], 0 idxen
                mubuf_w0,
                0x8001_0100,
                S_ENDPGM,
            ];
            let code: Vec<u8> = words.iter().flat_map(|w| w.to_le_bytes()).collect();
            let native = NativeProgram {
                stage: HwStage::Cs,
                wave64: false,
                vgprs: 4,
                layout: vec![RootParam {
                    root: RootType::Table,
                    table_offset_dwords: 0,
                    sgprs: 1,
                    base_register: 0,
                    user_data: 0,
                    slot: 0,
                }],
                used: vec![],
                io: StageIo::Cs(ComputeIo {
                    rsrc2: 0,
                    threads: [1, 1, 1],
                }),
                code,
            };
            let p = ir::build(&native.code, false, 4).unwrap();
            let res = resource::track(&p, &native.layout, native.stage).unwrap();
            let io = io::raise(&p, &native).unwrap();
            let st = structure::structure(&p).unwrap();
            lower(&Parts {
                native: &native,
                p: &p,
                st: &st,
                io: &io,
                res: &res,
                inputs: vec![],
                outputs: vec![],
                prims: vec![],
                mesh_stride: 0,
                target: crate::emit::Target::ANY,
                psv: None,
                console_cf: false,
                per_lane_only: false,
            })
            .unwrap();
        }
    }
}

#[cfg(test)]
mod select_tests {
    use super::super::emit;
    use crate::native::{ComputeIo, RootParam};
    use crate::{Container, FourCC, HwStage, NativeProgram, ResourceType, RootType, UsedResource};

    #[test]
    fn selects_from_independently_written_mask_words() {
        let words = [
            0xbe81_0380u32, // s1 = 0 (table address high)
            0xf408_0200,
            0xfa00_0000, // load UAV into s[8:11]
            0xbe84_03ff,
            0x8000_0000, // s4 = lane 31
            0xbe85_0381, // s5 = lane 32 in wave64
            0x7e02_028b,
            0x7e04_029d, // v1 = 11, v2 = 29
            0xd501_0003,
            0x0012_0501, // v_cndmask_b32 v3, v1, v2, s[4:5]
            0x3402_0082, // v1 = 4 * v0
            0xe070_1000,
            0x8002_0301, // store v3 to UAV at v1
            0xbf81_0000,
        ];
        let container = crate::dxbc::build(&[]);
        let c = Container::parse(&container).unwrap();
        for (wave64, uses_select) in [(false, true), (true, true), (true, false)] {
            let mut words = words;
            if !uses_select {
                words[12] = 0x8002_0101; // store v1 instead of the select result
            }
            let native = NativeProgram {
                stage: HwStage::Cs,
                wave64,
                vgprs: 4,
                layout: vec![RootParam {
                    root: RootType::Table,
                    table_offset_dwords: 0,
                    sgprs: 1,
                    base_register: 0,
                    user_data: 0,
                    slot: 0,
                }],
                used: vec![UsedResource {
                    kind: ResourceType::Uav,
                    root: RootType::Table,
                    table_offset_dwords: 0,
                    lower_bound: 0,
                    stride: 0,
                    dwords: 4,
                    user_data: 0,
                    slot: 0,
                }],
                io: crate::native::StageIo::Cs(ComputeIo {
                    rsrc2: 2,
                    threads: [if wave64 { 64 } else { 32 }, 1, 1],
                }),
                code: words.iter().flat_map(|w| w.to_le_bytes()).collect(),
            };
            let p = crate::ir::build(&native.code, wave64, native.vgprs).unwrap();
            let st = crate::structure::structure(&p).unwrap();
            let io = crate::io::raise(&p, &native).unwrap();
            let res = crate::resource::track(&p, &native.layout, native.stage).unwrap();
            let shader = emit(&c, &native, &p, &st, &io, &res, crate::emit::Target::ANY).unwrap();
            let emitted = Container::parse(&shader.dxil).unwrap();
            let features = emitted.part(FourCC::SFI0).unwrap();
            let wave_ops = u64::from_le_bytes(features.data.try_into().unwrap()) & 0x4000 != 0;
            assert_eq!(wave_ops, uses_select);
        }
    }
}
