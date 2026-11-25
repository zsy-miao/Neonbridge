//! Resource tracking (decision 0003): where each descriptor a program uses
//! comes from, in terms of the root signature.
//!
//! Root parameters arrive in user SGPRs ([`RootParam`]): a descriptor table
//! as the 32-bit heap address of its range (the code sets the high half to
//! 0), a root descriptor as a 64-bit buffer address, root constants as
//! themselves. A root signature with dynamic resources (SM 6.6) adds the
//! 32-bit base addresses of both heaps; a descriptor is then loaded from
//! one of them at an index the code computes (from a root constant, say). The code loads table descriptors from heap memory with
//! `s_load_dword*`, and builds a buffer descriptor around a root
//! descriptor's address itself. Each descriptor operand and each scalar
//! memory address is traced through the SSA values back to those
//! parameters, by data flow: copies, phis that agree, and additions.

use std::cell::RefCell;

use crate::ShaderError;
use crate::gfx10::{Fields, NULL, Operand};
use crate::ir::{Inst, Program, Reg, Value};
use crate::native::{HwStage, RootParam, RootType};

/// An instruction: block and index in [`crate::ir::Block::insts`].
pub type Site = (usize, usize);

/// A byte address: a root parameter's, plus a constant and at most one
/// value the program computes (an index times a stride, say).
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct Address {
    /// Index into the layout, or [`CODE`].
    pub param: usize,
    /// Added modulo 2^32 (with carry into the high half of a root
    /// descriptor's address).
    pub offset: u32,
    pub dynamic: Option<Value>,
    /// `dynamic` is a pointer the code keeps in a phi (advanced in a loop,
    /// or picked by a branch between offsets from the parameter): it holds
    /// the parameter's address plus a varying offset, so the byte offset
    /// from the parameter is `dynamic` minus the parameter's first dword,
    /// plus `offset`. For [`CODE`] it is the byte offset in the code.
    pub includes_base: bool,
}

/// [`Address::param`] of the program's own code: a constant table the
/// compiler appends after the code and reads through `s_getpc_b64`; the
/// offset is then the byte offset in the code.
pub const CODE: usize = usize::MAX;

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Source {
    /// Dwords `first..` of what the scalar load at `load` reads (see
    /// [`Resources::scalar`] for its address).
    Loaded { load: Site, first: u8 },
    /// A buffer descriptor built around an address: `bits` OR-ed into
    /// dword 1 (stride, swizzle), dwords 2 and 3 as given.
    Built {
        address: Address,
        bits: u32,
        words: [u32; 2],
    },
    /// A buffer descriptor the code makes around an address that is no root
    /// parameter's, or written through one: the dwords the instruction reads
    /// (only with [`Resources::addresses`]).
    Address,
    /// Fixed in the code (an immediate sampler, say).
    Constant(Vec<u32>),
    /// Chosen by control flow: one source per predecessor of `block`, in
    /// [`crate::ir::Block::preds`] order.
    Phi { block: usize, args: Vec<Source> },
    /// Chosen by SCC (`scc`, the value an `s_cselect` or `s_cmov` of each
    /// dword reads): the source when it is set, then when it is clear.
    Select { scc: Value, args: Vec<Source> },
    /// Dwords `first..` of what the vector load at `load` (`global_load_*`)
    /// reads, each lane at its own address (see [`Resources::scalar`]); the
    /// dwords past that load come from loads of the bytes that follow. A
    /// descriptor that differs between lanes.
    Fetched { load: Site, first: u8 },
    /// Picked per lane by `v_cndmask_b32` reading the lane mask whose low
    /// dword is `mask`: the source where the lane's bit is set, then where it
    /// is clear.
    Lanes { mask: Value, args: Vec<Source> },
    /// Picked per lane by the path the lane took into `block`, where VGPR
    /// phis join the dwords: one source per predecessor, in
    /// [`crate::ir::Block::preds`] order. A descriptor that differs between
    /// lanes.
    Paths { block: usize, args: Vec<Source> },
    /// In a [`Source::Paths`] arg: the dwords the enclosing `Paths` at
    /// `block` joins, as they were on the previous pass through the loop.
    Carried { block: usize },
    /// In a [`Source::Paths`] arg: VGPRs no instruction wrote on that path,
    /// undefined unless the hardware loads them (listed in
    /// [`Waterfall::unset`], checked when lowering).
    Unset,
}

impl Source {
    /// The sources this one picks from (itself unless a phi).
    pub fn leaves(&self) -> Vec<&Source> {
        match self {
            Source::Phi { args, .. }
            | Source::Select { args, .. }
            | Source::Lanes { args, .. }
            | Source::Paths { args, .. } => args.iter().flat_map(Source::leaves).collect(),
            s => vec![s],
        }
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Role {
    /// Image, buffer or `s_buffer_load` resource.
    Resource,
    Sampler,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Descriptor {
    pub site: Site,
    pub role: Role,
    pub source: Source,
}

#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct Resources {
    /// Every scalar memory access by address (`s_load_*`, `s_store_*`), and
    /// every global memory access (`global_*`, the dynamic term per lane).
    pub scalar: Vec<(Site, Address)>,
    pub descriptors: Vec<Descriptor>,
    /// User SGPRs holding root parameters: value on entry, layout index,
    /// dword within the parameter.
    pub params: Vec<(Value, usize, u32)>,
    pub waterfall: Waterfall,
    /// Memory is read and written by address where no root parameter or
    /// table leads to it (see [`track_with`]).
    pub addresses: bool,
}

/// Waterfall loops over descriptors that differ between lanes: each pass
/// reads the first active lane's descriptor with `v_readfirstlane_b32`,
/// compares it with every lane's own (`v_cmp_eq`), and runs the access with
/// the lanes that match. Their accesses take each lane's own descriptor
/// ([`Source::Fetched`], [`Source::Lanes`]), so the first lane's dwords are
/// each lane's own too: the compares hold in every lane and one pass serves
/// all lanes.
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct Waterfall {
    /// The compares, true in every lane.
    pub compares: Vec<Site>,
    /// The `v_readfirstlane_b32` results they compare, each the lane's own
    /// source dword.
    pub reads: Vec<Value>,
    /// VGPRs read before any write that a per-lane descriptor's phis join
    /// ([`Source::Unset`], or dropped where the other paths agree): taken
    /// as undefined, so as anything, which holds unless the hardware loads
    /// them.
    pub unset: Vec<Value>,
}

/// Traces every descriptor and scalar address of `p`, which runs as
/// `stage`, to the root parameters in `layout`. Fails on the first one it
/// cannot trace.
/// Whether the program intersects rays with an acceleration structure.
pub fn traces_rays(p: &Program) -> bool {
    p.blocks
        .iter()
        .flat_map(|b| &b.insts)
        .any(|i| i.name().starts_with("image_bvh"))
}

pub fn track(p: &Program, layout: &[RootParam], stage: HwStage) -> Result<Resources, ShaderError> {
    track_with(p, layout, stage, false)
}

/// [`track`]; with `addresses`, memory no root parameter or table leads to
/// is accessed by its address (ray tracing programs, and those that build
/// what they trace).
pub fn track_with(
    p: &Program,
    layout: &[RootParam],
    stage: HwStage,
    addresses: bool,
) -> Result<Resources, ShaderError> {
    let t = Tracer::new(p, layout, stage)?;
    let mut out = Resources {
        params: p
            .inputs
            .iter()
            .filter_map(|&(reg, v)| {
                let (param, dword) = t.param_of(reg)?;
                Some((v, param, dword))
            })
            .collect(),
        addresses,
        ..Resources::default()
    };
    // `v_readfirstlane_b32` results descriptors are read from.
    let mut first_lane: Vec<Value> = Vec::new();
    // Programs that trace rays follow pointers out of the acceleration
    // structure, which the host does not have: those loads are left untracked
    // (they give zeros, and rays hit nothing).
    let traces = traces_rays(p);
    for (b, block) in p.blocks.iter().enumerate() {
        for (i, inst) in block.insts.iter().enumerate() {
            let site = (b, i);
            let fail = |what| ShaderError::Unsupported {
                offset: inst.machine.offset,
                what,
            };
            let mut descriptor = |op: &Operand, role| -> Result<(), ShaderError> {
                let buffer = matches!(
                    inst.fields,
                    Fields::Buffer { .. } | Fields::Smem { .. }
                );
                let source = match t.descriptor(inst, op) {
                    Err(_) if addresses && buffer => Source::Address,
                    found => found.map_err(fail)?,
                };
                first_lane.extend(t.first_lane_words(inst, op).unwrap_or_default());
                out.descriptors.push(Descriptor { site, role, source });
                Ok(())
            };
            match &inst.fields {
                Fields::Image { .. } if inst.name().starts_with("image_bvh") => {}
                Fields::Image { rsrc, sampler, .. } => {
                    descriptor(rsrc, Role::Resource)?;
                    if let Some(s) = sampler {
                        descriptor(s, Role::Sampler)?;
                    }
                }
                Fields::Buffer { rsrc, .. } => {
                    descriptor(rsrc, Role::Resource)?;
                    // Per-lane descriptors may be fetched from a buffer over
                    // the heap (see `Tracer::fetched`).
                    if inst.name().starts_with("buffer_load_dword")
                        && let Ok(address) = t.buffer_address(inst)
                    {
                        out.scalar.push((site, address));
                    }
                }
                Fields::Smem { base, .. } if inst.name().starts_with("s_buffer_") => {
                    descriptor(base, Role::Resource)?;
                }
                Fields::Smem {
                    base,
                    offset,
                    soffset,
                    ..
                } if inst.name().contains("_load_") || inst.name().contains("_store_") => {
                    match t.smem_address(inst, base, *offset, soffset) {
                        Ok(address) => out.scalar.push((site, address)),
                        Err(_) if (traces || addresses) && inst.name().contains("_load_") => {}
                        Err(what) => return Err(fail(what)),
                    }
                }
                Fields::Flat {
                    addr,
                    saddr,
                    offset,
                    ..
                } if inst.name().starts_with("global_") => {
                    match t.flat_address(inst, addr.as_ref(), saddr.as_ref(), *offset) {
                        Ok(address) if addresses && address.param != CODE => {}
                        Err(_) if addresses => {}
                        Ok(address) => out.scalar.push((site, address)),
                        Err(_) if traces && inst.name().starts_with("global_load_dword") => {}
                        Err(what) => return Err(fail(what)),
                    }
                }
                _ => {}
            }
        }
    }
    out.waterfall = t.waterfall(&first_lane);
    out.waterfall.unset = t.unset.take();
    Ok(out)
}

/// A dword after copy propagation.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum Term {
    Const(u32),
    Value(Value),
}

/// A 32-bit sum: optional root parameter (as its first dword), constant,
/// optional computed term.
#[derive(Clone, Copy)]
struct Sum {
    base: Option<Value>,
    offset: u32,
    dynamic: Option<Value>,
}

struct Tracer<'a> {
    p: &'a Program,
    layout: &'a [RootParam],
    stage: HwStage,
    /// Defining instruction per value (phis and inputs have none).
    defs: Vec<Option<Site>>,
    /// Block and index of each phi value.
    phis: Vec<Option<(usize, usize)>>,
    /// Register per input value.
    inputs: Vec<Option<Reg>>,
    origin: Vec<Term>,
    /// Address sums already traced, indexed by SSA value (the operands'
    /// origins are settled before tracing starts, so every use of a value
    /// can reuse its sum, an opaque one included).
    sums: std::cell::RefCell<Vec<Option<Sum>>>,
    /// Per-lane joins being traced: block and the dwords joined there.
    lane_joins: RefCell<Vec<(usize, Vec<Term>)>>,
    /// See [`Waterfall::unset`].
    unset: RefCell<Vec<Value>>,
}

const MAX_ROUNDS: usize = 64;
const MAX_JOINS: usize = 8;
/// Steps through phis and additions when tracing a pointer kept in a phi.
const MAX_POINTER_STEPS: usize = 256;

impl<'a> Tracer<'a> {
    fn new(p: &'a Program, layout: &'a [RootParam], stage: HwStage) -> Result<Self, ShaderError> {
        let all = || {
            p.inputs
                .iter()
                .map(|i| i.1)
                .chain(p.blocks.iter().flat_map(|b| {
                    b.phis
                        .iter()
                        .map(|phi| phi.value)
                        .chain(b.insts.iter().flat_map(|i| i.writes.iter().map(|w| w.1)))
                }))
        };
        let n = all().map(|v| v.0 as usize + 1).max().unwrap_or(0);
        let mut defs = vec![None; n];
        let mut phis = vec![None; n];
        let mut inputs = vec![None; n];
        for &(reg, v) in &p.inputs {
            inputs[v.0 as usize] = Some(reg);
        }
        for (b, block) in p.blocks.iter().enumerate() {
            for (i, phi) in block.phis.iter().enumerate() {
                phis[phi.value.0 as usize] = Some((b, i));
            }
            for (i, inst) in block.insts.iter().enumerate() {
                for &(_, v) in &inst.writes {
                    defs[v.0 as usize] = Some((b, i));
                }
            }
        }
        let mut t = Tracer {
            p,
            layout,
            stage,
            defs,
            phis,
            inputs,
            origin: Vec::new(),
            sums: std::cell::RefCell::new(Vec::new()),
            lane_joins: RefCell::new(Vec::new()),
            unset: RefCell::new(Vec::new()),
        };
        t.origin = t.propagate_copies(n)?;
        Ok(t)
    }

    /// Optimistic copy propagation: a move's result is its source's
    /// origin, a phi's the one origin its arguments agree on.
    fn propagate_copies(&self, n: usize) -> Result<Vec<Term>, ShaderError> {
        let mut o: Vec<Option<Term>> = vec![None; n];
        for &(_, v) in &self.p.inputs {
            o[v.0 as usize] = Some(Term::Value(v));
        }
        let order = crate::ir::reverse_postorder(&self.p.blocks);
        for _ in 0..MAX_ROUNDS {
            let mut changed = false;
            for &b in &order {
                let block = &self.p.blocks[b];
                for phi in &block.phis {
                    let own = Term::Value(phi.value);
                    let mut args = phi
                        .args
                        .iter()
                        .filter_map(|a| o[a.0 as usize])
                        .filter(|&t| t != own);
                    let new = args
                        .next()
                        .map(|first| if args.all(|t| t == first) { first } else { own });
                    if new.is_some() && new != o[phi.value.0 as usize] {
                        o[phi.value.0 as usize] = new;
                        changed = true;
                    }
                }
                for inst in &block.insts {
                    for (k, &(_, v)) in inst.writes.iter().enumerate() {
                        let new = match copy_source(inst, k).or_else(|| self.restored(inst)) {
                            Some(Moved::Const(c)) => Some(Term::Const(c)),
                            Some(Moved::Value(s)) => o[s.0 as usize],
                            None => Some(Term::Value(v)),
                        };
                        if new.is_some() && new != o[v.0 as usize] {
                            o[v.0 as usize] = new;
                            changed = true;
                        }
                    }
                }
            }
            if !changed {
                return Ok(o
                    .into_iter()
                    .enumerate()
                    .map(|(v, t)| t.unwrap_or(Term::Value(Value(v as u32))))
                    .collect());
            }
        }
        Err(ShaderError::Unsupported {
            offset: 0,
            what: "copies that do not settle",
        })
    }

    fn inst(&self, (b, i): Site) -> &'a Inst {
        &self.p.blocks[b].insts[i]
    }

    fn def(&self, v: Value) -> Option<&'a Inst> {
        self.defs[v.0 as usize].map(|s| self.inst(s))
    }

    fn origin(&self, v: Value) -> Term {
        self.origin[v.0 as usize]
    }

    /// The origin of a one-dword operand as `inst` reads it.
    fn term(&self, inst: &Inst, op: &Operand) -> Option<Term> {
        match *op {
            Operand::Scalar { code: NULL, .. } => Some(Term::Const(0)),
            Operand::Scalar { code, .. } => Some(self.origin(inst.read(Reg::S(code))?)),
            Operand::Const(c) => inline(c).map(Term::Const),
            Operand::Literal(x) => Some(Term::Const(x)),
            _ => None,
        }
    }

    /// Layout index and dword of the root parameter `reg` holds on entry.
    fn param_of(&self, reg: Reg) -> Option<(usize, u32)> {
        let Reg::S(code) = reg else { return None };
        self.layout.iter().enumerate().find_map(|(i, p)| {
            let first = self.stage.user_sgpr(p.user_data);
            let dword = u32::from(code).checked_sub(first.into())?;
            (dword < p.sgprs).then_some((i, dword))
        })
    }

    /// The SOP2 operands of `v` if an `s_add` with one of `names` wrote it.
    fn sop2(&self, v: Value, names: &[&str]) -> Option<(&'a Inst, [Term; 2])> {
        let inst = self.def(v)?;
        if !names.contains(&inst.name()) || inst.writes.first()?.1 != v {
            return None;
        }
        let Fields::Sop { src, .. } = &inst.fields else {
            return None;
        };
        Some((
            inst,
            [
                self.term(inst, src.first()?)?,
                self.term(inst, src.get(1)?)?,
            ],
        ))
    }

    fn sum(&self, t: Term) -> Sum {
        let Term::Value(v) = t else {
            return self.sum_uncached(t);
        };
        let index = v.0 as usize;
        if let Some(&Some(sum)) = self.sums.borrow().get(index) {
            return sum;
        }
        let sum = self.sum_uncached(t);
        let mut sums = self.sums.borrow_mut();
        if index >= sums.len() {
            sums.resize(index + 1, None);
        }
        sums[index] = Some(sum);
        sum
    }

    fn sum_uncached(&self, t: Term) -> Sum {
        let v = match t {
            Term::Const(c) => {
                return Sum {
                    base: None,
                    offset: c,
                    dynamic: None,
                };
            }
            Term::Value(v) => v,
        };
        let dynamic = Sum {
            base: None,
            offset: 0,
            dynamic: Some(v),
        };
        if let Some(reg) = self.inputs[v.0 as usize] {
            // A root constant is a number the code adds (a byte offset into
            // a table, say), not a base.
            return match self.param_of(reg) {
                Some((i, 0)) if self.layout[i].root != RootType::Constants => Sum {
                    base: Some(v),
                    offset: 0,
                    dynamic: None,
                },
                _ => dynamic,
            };
        }
        if self.getpc(v).is_some() {
            return Sum {
                base: Some(v),
                offset: 0,
                dynamic: None,
            };
        }
        if let Some((x, k)) = self.plus_const(v) {
            let mut s = self.sum(x);
            s.offset = s.offset.wrapping_add(k);
            return s;
        }
        let Some([a, b]) = self
            .sop2(v, &["s_add_u32", "s_add_i32"])
            .map(|(_, t)| t)
            .or_else(|| self.vadd(v))
        else {
            return dynamic;
        };
        let (x, y) = (self.sum(a), self.sum(b));
        if x.base.is_some() && y.base.is_some() || x.dynamic.is_some() && y.dynamic.is_some() {
            return dynamic;
        }
        Sum {
            base: x.base.or(y.base),
            offset: x.offset.wrapping_add(y.offset),
            dynamic: x.dynamic.or(y.dynamic),
        }
    }

    /// `v` as another dword plus a constant: `s_addk_i32` (in place, a
    /// sign-extended immediate) or `s_sub_u32`/`s_sub_i32` of a constant.
    fn plus_const(&self, v: Value) -> Option<(Term, u32)> {
        if let Some((_, [x, Term::Const(c)])) = self.sop2(v, &["s_sub_u32", "s_sub_i32"]) {
            return Some((x, c.wrapping_neg()));
        }
        let inst = self.def(v)?;
        let Fields::Sop {
            dst: Some(Operand::Scalar { code, .. }),
            imm: Some(imm),
            ..
        } = inst.fields
        else {
            return None;
        };
        (inst.name() == "s_addk_i32" && inst.writes.first()?.1 == v).then_some((
            self.origin(inst.read(Reg::S(code))?),
            i32::from(imm as i16) as u32,
        ))
    }

    /// The `s_getpc_b64` writing `lo` as its low dword.
    fn getpc(&self, lo: Value) -> Option<&'a Inst> {
        let inst = self.def(lo)?;
        (inst.name() == "s_getpc_b64" && inst.writes.first().is_some_and(|w| w.1 == lo))
            .then_some(inst)
    }

    /// The operands of a VALU addition (`v_add_co_u32`, `v_add_nc_u32`)
    /// writing `v`, through VGPR copies of scalars.
    fn vadd(&self, v: Value) -> Option<[Term; 2]> {
        let inst = self.def(v)?;
        let Fields::Valu(f) = &inst.fields else {
            return None;
        };
        if !matches!(inst.name(), "v_add_co_u32" | "v_add_nc_u32")
            || f.dst.is_none_or(|d| !matches!(d, Operand::Vector { .. }))
            || inst.write(Reg::V(match f.dst? {
                Operand::Vector { reg, .. } => reg,
                _ => return None,
            })) != Some(v)
            || f.abs != 0
            || f.neg != 0
            || f.sdwa.is_some()
            || f.dpp.is_some()
        {
            return None;
        }
        Some([
            self.vterm(inst, f.src.first()?)?,
            self.vterm(inst, f.src.get(1)?)?,
        ])
    }

    /// The origin of a VALU operand.
    fn vterm(&self, inst: &Inst, op: &Operand) -> Option<Term> {
        match *op {
            Operand::Vector { reg, .. } => Some(self.vorigin(inst.read(Reg::V(reg))?)),
            op => self.term(inst, &op),
        }
    }

    /// A VGPR value's origin through `v_mov_b32` copies.
    fn vorigin(&self, v: Value) -> Term {
        if let Some(inst) = self.def(v)
            && inst.name() == "v_mov_b32"
            && let Fields::Valu(f) = &inst.fields
            && f.abs == 0
            && f.neg == 0
            && f.sdwa.is_none()
            && f.dpp.is_none()
            && let Some(t) = f.src.first().and_then(|s| self.vterm(inst, s))
        {
            return t;
        }
        self.origin(v)
    }

    /// A global memory address: an SGPR pair plus a VGPR offset, or a VGPR
    /// pair.
    /// The address a `buffer_load_dword*` reads in each lane, for a buffer
    /// whose descriptor is built around an address (a heap or a table, say):
    /// that address plus the VGPR offset (`offen`), `soffset` and the
    /// immediate. None of indexed (`idxen`) or formatted loads.
    fn buffer_address(&self, inst: &Inst) -> Result<Address, &'static str> {
        const WHY: &str = "a descriptor read from the first lane of a buffer load";
        let Fields::Buffer {
            addr,
            rsrc,
            soffset,
            offset,
            offen,
            idxen,
            format: None,
            lds: false,
            tfe: false,
            ..
        } = &inst.fields
        else {
            return Err(WHY);
        };
        if *idxen {
            return Err(WHY);
        }
        let Source::Built { address, .. } = self.descriptor(inst, rsrc)? else {
            return Err("a descriptor read from the first lane of a buffer of descriptors");
        };
        let mut terms = Vec::new();
        if *offen {
            let Operand::Vector { reg, .. } = *addr else {
                return Err(WHY);
            };
            terms.push(self.vorigin(inst.read(Reg::V(reg)).ok_or(WHY)?));
        }
        match *soffset {
            Operand::Scalar { code: NULL, .. } => {}
            Operand::Scalar { code, .. } => {
                terms.push(self.origin(inst.read(Reg::S(code)).ok_or(WHY)?))
            }
            Operand::Const(c) => terms.push(Term::Const(inline(c).ok_or(WHY)?)),
            _ => return Err(WHY),
        }
        let mut a = address;
        a.offset = a.offset.wrapping_add(u32::from(*offset));
        for t in terms {
            let s = self.sum(t);
            if s.base.is_some() {
                return Err(WHY);
            }
            a.offset = a.offset.wrapping_add(s.offset);
            if let Some(d) = s.dynamic {
                if a.dynamic.is_some() {
                    return Err("an address with two computed terms");
                }
                a.dynamic = Some(d);
            }
        }
        Ok(a)
    }

    fn flat_address(
        &self,
        inst: &Inst,
        addr: Option<&Operand>,
        saddr: Option<&Operand>,
        offset: i16,
    ) -> Result<Address, &'static str> {
        let Some(&Operand::Vector { reg, .. }) = addr else {
            return Err("a global address not in VGPRs");
        };
        let off = offset as i32 as u32;
        match saddr {
            Some(&Operand::Scalar { code, .. }) if code != NULL => {
                let word = |k| Some(self.origin(inst.read(Reg::S(code + k))?));
                let (Some(lo), Some(hi)) = (word(0), word(1)) else {
                    return Err("an address not in SGPRs");
                };
                let v = inst.read(Reg::V(reg)).ok_or("a global address")?;
                let mut extra = self.sum(self.vorigin(v));
                if extra.base.is_some() {
                    return Err("an offset from a root parameter");
                }
                extra.offset = extra.offset.wrapping_add(off);
                self.address(lo, hi, extra)
            }
            _ => {
                let word = |k| inst.read(Reg::V(reg + k)).map(|v| self.vorigin(v));
                let (Some(lo), Some(hi)) = (word(0), word(1)) else {
                    return Err("a global address");
                };
                self.address(
                    lo,
                    hi,
                    Sum {
                        base: None,
                        offset: off,
                        dynamic: None,
                    },
                )
            }
        }
    }

    /// The address `lo` and `hi` hold, plus `extra` (an instruction's
    /// immediate and offset register).
    fn address(&self, lo: Term, hi: Term, extra: Sum) -> Result<Address, &'static str> {
        let s = self.sum(lo);
        if s.dynamic.is_some() && extra.dynamic.is_some() {
            return Err("an address with two computed terms");
        }
        if s.base.is_none()
            && let Some(d) = s.dynamic
            && self.phis[d.0 as usize].is_some()
        {
            return self.pointer_address(d, s.offset.wrapping_add(extra.offset), hi);
        }
        // A pointer into a table in the code taken from the first lane.
        if s.base.is_none()
            && let Some(d) = s.dynamic
            && let Ok(a) = self.pointer_address(d, s.offset.wrapping_add(extra.offset), hi)
            && a.param == CODE
        {
            return Ok(a);
        }
        let base = s.base.ok_or_else(|| self.why_no_base(s.dynamic))?;
        if let Some(pc) = self.getpc(base) {
            // A table in the code: the base is the next instruction's offset.
            let high = pc.writes.get(1).map(|w| w.1);
            if !self.high_half(hi, high) && !high.is_some_and(|h| self.carried(hi, h)) {
                return Err("an address with an untracked high half");
            }
            return Ok(Address {
                param: CODE,
                offset: (pc.machine.offset as u32 + 4)
                    .wrapping_add(s.offset)
                    .wrapping_add(extra.offset),
                dynamic: s.dynamic.or(extra.dynamic),
                includes_base: false,
            });
        }
        let reg = self.inputs[base.0 as usize].ok_or("an address not from a root parameter")?;
        let (param, _) = self
            .param_of(reg)
            .ok_or("an address not from a root parameter")?;
        let high = match self.layout[param].root {
            RootType::Table | RootType::ResourceHeap | RootType::SamplerHeap => None,
            RootType::Descriptor => {
                let high = Reg::S(self.stage.user_sgpr(self.layout[param].user_data) + 1);
                let input = self.p.inputs.iter().find(|i| i.0 == high);
                Some(input.ok_or("a root descriptor without its high half")?.1)
            }
            _ => return Err("an address from a root parameter that is no address"),
        };
        // shortcut: with a computed term, a high half that adds anything to the
        // base's is taken as the carry of that term (offsets below 4 GB).
        let dynamic = s.dynamic.or(extra.dynamic);
        if !self.high_half(hi, high)
            && !(dynamic.is_some() && high.is_some_and(|h| self.carried(hi, h)))
        {
            return Err("an address with an untracked high half");
        }
        Ok(Address {
            param,
            offset: s.offset.wrapping_add(extra.offset),
            dynamic: s.dynamic.or(extra.dynamic),
            includes_base: false,
        })
    }

    /// The address of a pointer `d` the code keeps in a phi, plus `offset`:
    /// each argument of the phi is the same root parameter's address plus
    /// offsets, or the phi itself (or another such phi) plus offsets, as a
    /// pointer advanced in a loop or picked by a branch. `d` then holds that
    /// parameter's address plus a varying term (see
    /// [`Address::includes_base`]).
    fn pointer_address(&self, d: Value, offset: u32, hi: Term) -> Result<Address, &'static str> {
        const WHY: &str = "an address from a phi";
        let mut budget = MAX_POINTER_STEPS;
        let base = self
            .pointer_base(Term::Value(d), &mut Vec::new(), &mut budget)
            .ok()
            .flatten()
            .ok_or(WHY)?;
        if self.getpc(base).is_some() {
            // The emitter has `s_getpc_b64` give the offset in the code.
            return Ok(Address {
                param: CODE,
                offset,
                dynamic: Some(d),
                includes_base: true,
            });
        }
        let reg = self.inputs[base.0 as usize].ok_or(WHY)?;
        let (param, _) = self.param_of(reg).ok_or(WHY)?;
        let high = match self.layout[param].root {
            RootType::Table | RootType::ResourceHeap | RootType::SamplerHeap => None,
            RootType::Descriptor => {
                let high = Reg::S(self.stage.user_sgpr(self.layout[param].user_data) + 1);
                let input = self.p.inputs.iter().find(|i| i.0 == high);
                Some(input.ok_or("a root descriptor without its high half")?.1)
            }
            _ => return Err("an address from a root parameter that is no address"),
        };
        if !self.pointer_high(hi, high, &mut Vec::new(), &mut budget) {
            return Err("an address with an untracked high half");
        }
        Ok(Address {
            param,
            offset,
            dynamic: Some(d),
            includes_base: true,
        })
    }

    /// The root parameter (its first dword) that the pointer `t` is an
    /// offset from: `None` when it only goes around the phis in `seen`
    /// (the back edge of a loop), an error when it is no such pointer or
    /// mixes two parameters. `budget` bounds the walk.
    fn pointer_base(
        &self,
        t: Term,
        seen: &mut Vec<Value>,
        budget: &mut usize,
    ) -> Result<Option<Value>, ()> {
        *budget = budget.checked_sub(1).ok_or(())?;
        let Term::Value(v) = t else { return Err(()) };
        if seen.contains(&v) {
            return Ok(None);
        }
        if let Some(base) = self.sum(t).base {
            return match self.inputs[base.0 as usize] {
                Some(_) => Ok(Some(base)),
                None if self.getpc(base).is_some() => Ok(Some(base)),
                None => Err(()),
            };
        }
        if let Some(x) = self.first_lane_source(v) {
            return self.pointer_base(self.vorigin(x), seen, budget);
        }
        if let Some((block, i)) = self.phis[v.0 as usize] {
            seen.push(v);
            let mut found = None;
            let mut result = Ok(());
            for &a in &self.p.blocks[block].phis[i].args {
                match self.pointer_base(self.vorigin(a), seen, budget) {
                    Ok(None) => {}
                    Ok(Some(b))
                        if found.is_none_or(|f| {
                            f == b || self.getpc(f).is_some() && self.getpc(b).is_some()
                        }) =>
                    {
                        found = Some(b)
                    }
                    _ => {
                        result = Err(());
                        break;
                    }
                }
            }
            seen.pop();
            return result.map(|()| found);
        }
        if let Some((x, _)) = self.plus_const(v) {
            return self.pointer_base(x, seen, budget);
        }
        let [a, b] = self
            .sop2(v, &["s_add_u32", "s_add_i32"])
            .map(|(_, t)| t)
            .or_else(|| self.vadd(v))
            .ok_or(())?;
        // One side is the pointer, the other an offset.
        match (
            self.pointer_base(a, seen, budget),
            self.pointer_base(b, seen, budget),
        ) {
            (Ok(p), Err(())) | (Err(()), Ok(p)) => Ok(p),
            _ => Err(()),
        }
    }

    /// `hi` is the high half of a pointer kept in a phi: `high` (0 for a
    /// table) through phis, carries and borrows.
    fn pointer_high(
        &self,
        hi: Term,
        high: Option<Value>,
        seen: &mut Vec<Value>,
        budget: &mut usize,
    ) -> bool {
        let Some(left) = budget.checked_sub(1) else {
            return false;
        };
        *budget = left;
        if self.high_half(hi, high) || high.is_some_and(|h| self.carried(hi, h)) {
            return true;
        }
        let Term::Value(v) = hi else { return false };
        if seen.contains(&v) {
            return true;
        }
        if let Some((block, i)) = self.phis[v.0 as usize] {
            seen.push(v);
            let ok = self.p.blocks[block].phis[i]
                .args
                .iter()
                .all(|&a| self.pointer_high(self.vorigin(a), high, seen, budget));
            seen.pop();
            return ok;
        }
        let carry = self
            .sop2(v, &["s_addc_u32"])
            .map(|(_, t)| t)
            .or_else(|| self.vadd_carry(v));
        let borrow = self
            .sop2(v, &["s_subb_u32"])
            .and_then(|(_, [a, b])| (b == Term::Const(0)).then_some([a, b]));
        match carry.or(borrow) {
            Some([x, Term::Const(0)]) | Some([Term::Const(0), x]) => {
                self.pointer_high(x, high, seen, budget)
            }
            _ => false,
        }
    }

    /// The operands of a `v_add_co_ci_u32` writing `v` (the carry-in
    /// aside), through VGPR copies of scalars.
    fn vadd_carry(&self, v: Value) -> Option<[Term; 2]> {
        let inst = self.def(v)?;
        let Fields::Valu(f) = &inst.fields else {
            return None;
        };
        let Some(Operand::Vector { reg, .. }) = f.dst else {
            return None;
        };
        if inst.name() != "v_add_co_ci_u32" || inst.write(Reg::V(reg)) != Some(v) {
            return None;
        }
        Some([
            self.vterm(inst, f.src.first()?)?,
            self.vterm(inst, f.src.get(1)?)?,
        ])
    }

    /// `hi` is `high` (0 for a table), possibly plus a carry or minus a
    /// borrow.
    fn high_half(&self, hi: Term, high: Option<Value>) -> bool {
        let want = high.map_or(Term::Const(0), Term::Value);
        let direct = |t: Term| {
            t == want
                || matches!(t, Term::Value(v) if self.sop2(v, &["s_addc_u32"]).is_some_and(
                    |(_, [a, b])| a == want && b == Term::Const(0) || b == want && a == Term::Const(0)
                ))
                || matches!(t, Term::Value(v) if self.sop2(v, &["s_subb_u32"]).is_some_and(
                    |(_, [a, b])| a == want && b == Term::Const(0)
                ))
        };
        if direct(hi) {
            return true;
        }
        // A high half written on one branch (e.g. `s_mov_b32 s25, 0` inside an
        // early-skip block) merges with the uninitialized entry SGPR at the join.
        let Term::Value(v) = hi else { return false };
        let Some((b, k)) = self.phis[v.0 as usize] else {
            return false;
        };
        let phi = &self.p.blocks[b].phis[k];
        let mut has_match = false;
        for &arg in &phi.args {
            if arg == v {
                continue;
            }
            let t = self.origin(arg);
            if direct(t) {
                has_match = true;
            } else if let Term::Value(u) = t
                && let Some(reg) = self.inputs[u.0 as usize]
                && self.param_of(reg).is_none()
            {
                // Uninitialized scratch SGPR from an untaken/dead path.
            } else {
                return false;
            }
        }
        has_match
    }

    /// `hi` adds something (with carry) to `high`: `s_addc_u32` or
    /// `v_add_co_ci_u32` with `high` as an operand.
    fn carried(&self, hi: Term, high: Value) -> bool {
        let Term::Value(v) = hi else { return false };
        if v == high {
            return true;
        }
        if let Some((_, [a, b])) = self.sop2(v, &["s_addc_u32"]) {
            return self.carried(a, high) || self.carried(b, high);
        }
        let Some(inst) = self.def(v) else {
            return false;
        };
        let Fields::Valu(f) = &inst.fields else {
            return false;
        };
        inst.name() == "v_add_co_ci_u32"
            && f.src[..2.min(f.src.len())]
                .iter()
                .any(|o| self.vterm(inst, o).is_some_and(|t| self.carried(t, high)))
    }

    fn why_no_base(&self, dynamic: Option<Value>) -> &'static str {
        let Some(v) = dynamic else {
            return "a constant address";
        };
        if self.inputs[v.0 as usize].is_some() {
            return "an address in an SGPR that is no root parameter";
        }
        if self
            .leaves(v)
            .any(|l| self.def(l).is_some_and(|i| i.name() == "s_getpc_b64"))
        {
            return "an address relative to the program counter";
        }
        match self.def(v).map(|i| i.name()) {
            Some(n) if n.starts_with("s_load_") => "an address loaded from memory",
            None => "an address from a phi",
            _ => "an address not from a root parameter",
        }
    }

    /// The terms of the additions that compute `v`.
    fn leaves(&self, v: Value) -> Box<dyn Iterator<Item = Value> + '_> {
        match self.sop2(v, &["s_add_u32", "s_add_i32"]) {
            Some((_, terms)) => Box::new(terms.into_iter().flat_map(move |t| match t {
                Term::Value(x) => self.leaves(x),
                Term::Const(_) => Box::new(std::iter::empty()),
            })),
            None => Box::new(std::iter::once(v)),
        }
    }

    fn smem_address(
        &self,
        inst: &Inst,
        base: &Operand,
        offset: i32,
        soffset: &Operand,
    ) -> Result<Address, &'static str> {
        let Operand::Scalar { code, .. } = *base else {
            return Err("an address not in SGPRs");
        };
        let word = |k| Some(self.origin(inst.read(Reg::S(code + k))?));
        let (Some(lo), Some(hi)) = (word(0), word(1)) else {
            return Err("an address not in SGPRs");
        };
        let mut extra = self.sum(
            self.term(inst, soffset)
                .ok_or("an unknown offset operand")?,
        );
        if extra.base.is_some() {
            return Err("an offset from a root parameter");
        }
        extra.offset = extra.offset.wrapping_add(offset as u32);
        self.address(lo, hi, extra)
    }

    fn descriptor(&self, inst: &Inst, op: &Operand) -> Result<Source, &'static str> {
        let Operand::Scalar { code, dwords } = *op else {
            return Err("a descriptor not in SGPRs");
        };
        let words = (0..dwords)
            .map(|k| inst.read(Reg::S(code + k)).map(|v| self.origin(v)))
            .collect::<Option<Vec<_>>>()
            .ok_or("a descriptor not in SGPRs")?;
        self.classify(&words, &mut Vec::new())
    }

    /// The source of a descriptor's words; `joins` are the phi blocks
    /// being resolved.
    fn classify(&self, words: &[Term], joins: &mut Vec<usize>) -> Result<Source, &'static str> {
        if let Some(source) = self.phi(words, joins)? {
            return Ok(source);
        }
        if let Some(source) = self.select(words, joins)? {
            return Ok(source);
        }
        let read = |w: &Term| match *w {
            Term::Value(v) => self.first_lane_source(v),
            Term::Const(_) => None,
        };
        if let Some(sources) = words.iter().map(read).collect::<Option<Vec<_>>>() {
            // Read from the first active lane, as a waterfall loop does: the
            // dwords each lane holds.
            let lanes: Vec<Term> = sources
                .into_iter()
                .map(|v| self.lane_term(Term::Value(v)))
                .collect();
            return self.per_lane(&lanes, joins);
        }
        if words.iter().any(|w| read(w).is_some()) {
            return Err("a descriptor partly read from the first lane");
        }
        if let Some(consts) = words
            .iter()
            .map(|w| match *w {
                Term::Const(c) => Some(c),
                Term::Value(_) => None,
            })
            .collect::<Option<Vec<_>>>()
        {
            return Ok(Source::Constant(consts));
        }
        if let Some(source) = self.loaded(words) {
            return Ok(source);
        }
        if let Some(source) = self.made_raw(words) {
            return Ok(source);
        }
        if let [lo, hi, Term::Const(w2), Term::Const(w3)] = *words {
            let (mut hi, mut bits) = (hi, 0);
            while let Term::Value(v) = hi
                && let Some((x, more)) = self.set_bits(v)
            {
                (hi, bits) = (x, bits | more);
            }
            let address = self.address(
                lo,
                hi,
                Sum {
                    base: None,
                    offset: 0,
                    dynamic: None,
                },
            )?;
            return Ok(Source::Built {
                address,
                bits,
                words: [w2, w3],
            });
        }
        let loaded = |w: &Term| {
            matches!(*w, Term::Value(v)
                if self.def(v).is_some_and(|i| i.name().starts_with("s_load_")))
        };
        Err(if words.iter().any(loaded) {
            "a descriptor changed after loading"
        } else {
            "a descriptor neither loaded nor built"
        })
    }

    /// Words that phis of one block join, traced per predecessor (words
    /// defined before the join hold on every edge).
    fn phi(&self, words: &[Term], joins: &mut Vec<usize>) -> Result<Option<Source>, &'static str> {
        let phi_of = |w: &Term| match *w {
            Term::Value(v) => self.phis[v.0 as usize],
            Term::Const(_) => None,
        };
        let Some((block, _)) = words.iter().find_map(phi_of) else {
            return Ok(None);
        };
        if words.iter().filter_map(phi_of).any(|(b, _)| b != block) {
            return Err("a descriptor joined in several blocks");
        }
        if joins.contains(&block) {
            return Err("a descriptor that changes in a loop");
        }
        // Each level multiplies the sources by the predecessor count.
        if joins.len() >= MAX_JOINS {
            return Err("a descriptor joined too often");
        }
        joins.push(block);
        let phis = &self.p.blocks[block].phis;
        let args = (0..self.p.blocks[block].preds.len())
            .map(|j| {
                let edge: Vec<Term> = words
                    .iter()
                    .map(|w| match phi_of(w) {
                        Some((_, i)) => self.origin(phis[i].args[j]),
                        None => *w,
                    })
                    .collect();
                self.classify(&edge, joins)
            })
            .collect::<Vec<Result<Source, &'static str>>>();
        joins.pop();
        // shortcut: as for lanes ([`Self::lane_phi`]): the registers' former
        // use, on a path that does not read them as a descriptor.
        const FORMER: &str = "a descriptor neither loaded nor built";
        let known = args.iter().find_map(|a| match a {
            Ok(s @ (Source::Constant(_) | Source::Loaded { .. })) => Some(s.clone()),
            _ => None,
        });
        let args: Vec<Source> = args
            .into_iter()
            .map(|a| match a {
                Err(FORMER) if known.is_some() => Ok(Source::Unset),
                a => a,
            })
            .collect::<Result<_, _>>()?;
        if let Some(one @ Source::Constant(_)) = known
            && args.iter().all(|a| *a == one || matches!(a, Source::Unset))
        {
            return Ok(Some(one));
        }
        Ok(Some(Source::Phi { block, args }))
    }

    /// Words that `s_cselect` (or `s_cmov`) picks by one SCC value, traced
    /// per outcome (other words are the same for both).
    fn select(
        &self,
        words: &[Term],
        joins: &mut Vec<usize>,
    ) -> Result<Option<Source>, &'static str> {
        let picks: Vec<_> = words
            .iter()
            .map(|w| match *w {
                Term::Value(v) => self.picked(v),
                Term::Const(_) => None,
            })
            .collect();
        let Some(scc) = picks.iter().flatten().map(|p| p.0).next() else {
            return Ok(None);
        };
        if picks.iter().flatten().any(|p| p.0 != scc) {
            return Err("a descriptor chosen by several conditions");
        }
        // Each level doubles the sources; selects count like joins (a
        // marker no block index reaches).
        if joins.len() >= MAX_JOINS {
            return Err("a descriptor joined too often");
        }
        joins.push(usize::MAX);
        let side = |set: bool| -> Vec<Term> {
            words
                .iter()
                .zip(&picks)
                .map(|(w, p)| match p {
                    Some((_, t, f)) => {
                        if set {
                            *t
                        } else {
                            *f
                        }
                    }
                    None => *w,
                })
                .collect()
        };
        let args = [true, false]
            .into_iter()
            .map(|set| self.classify(&side(set), joins))
            .collect::<Result<_, _>>();
        joins.pop();
        Ok(Some(Source::Select { scc, args: args? }))
    }

    /// For `v` written by `s_cselect_b32`/`_b64` or `s_cmov_b32`/`_b64`:
    /// the SCC it reads and the dword it is when SCC is set, then clear.
    fn picked(&self, v: Value) -> Option<(Value, Term, Term)> {
        let inst = self.def(v)?;
        let Fields::Sop { dst, src, .. } = &inst.fields else {
            return None;
        };
        let k = inst.writes.iter().position(|w| w.1 == v)? as u8;
        let wide = match inst.name() {
            "s_cselect_b32" | "s_cmov_b32" => false,
            "s_cselect_b64" | "s_cmov_b64" => true,
            _ => return None,
        };
        // A dword of a 32- or 64-bit operand: 64-bit inline integers are
        // sign-extended; 64-bit literals and float constants are not
        // followed.
        let dword = |op: &Operand| match *op {
            Operand::Scalar { code: NULL, .. } => Some(Term::Const(0)),
            Operand::Scalar { code, .. } => Some(self.origin(inst.read(Reg::S(code + k))?)),
            Operand::Const(c) if !wide => inline(c).map(Term::Const),
            Operand::Const(c @ 128..=208) => {
                let low = inline(c)?;
                Some(Term::Const(if k == 0 {
                    low
                } else {
                    ((low as i32) >> 31) as u32
                }))
            }
            Operand::Literal(x) if !wide => Some(Term::Const(x)),
            _ => None,
        };
        let set = dword(src.first()?)?;
        let clear = if inst.name().starts_with("s_cmov") {
            dword(dst.as_ref()?)?
        } else {
            dword(src.get(1)?)?
        };
        Some((inst.read(Reg::Scc)?, set, clear))
    }

    /// `v` as some other dword with constant bits set.
    fn set_bits(&self, v: Value) -> Option<(Term, u32)> {
        if let Some((_, [x, Term::Const(bits)] | [Term::Const(bits), x])) =
            self.sop2(v, &["s_or_b32"])
        {
            return Some((x, bits));
        }
        let inst = self.def(v)?;
        let Fields::Sop {
            dst: Some(Operand::Scalar { code, .. }),
            src,
            ..
        } = &inst.fields
        else {
            return None;
        };
        match (inst.name(), self.term(inst, src.first()?)?) {
            ("s_bitset1_b32", Term::Const(bit)) => {
                Some((self.origin(inst.read(Reg::S(*code))?), 1 << (bit & 31)))
            }
            _ => None,
        }
    }

    /// For a `v_readfirstlane_b32` result: the VGPR value it reads.
    fn first_lane_source(&self, v: Value) -> Option<Value> {
        let inst = self.def(v)?;
        let Fields::Valu(f) = &inst.fields else {
            return None;
        };
        if inst.name() != "v_readfirstlane_b32" || f.sdwa.is_some() || f.dpp.is_some() {
            return None;
        }
        let Some(&Operand::Vector { reg, .. }) = f.src.first() else {
            return None;
        };
        inst.read(Reg::V(reg))
    }

    /// The `v_readfirstlane_b32` results the descriptor operand `op` of
    /// `inst` is, when every dword is one.
    fn first_lane_words(&self, inst: &Inst, op: &Operand) -> Option<Vec<Value>> {
        let Operand::Scalar { code, dwords } = *op else {
            return None;
        };
        (0..dwords)
            .map(|k| match self.origin(inst.read(Reg::S(code + k))?) {
                Term::Value(v) => self.first_lane_source(v).map(|_| v),
                Term::Const(_) => None,
            })
            .collect()
    }

    /// `t` through VGPR copies (of VGPRs or of scalars) and copies.
    fn lane_term(&self, mut t: Term) -> Term {
        for _ in 0..MAX_JOINS * 2 {
            let Term::Value(v) = t else { break };
            let next = self.vorigin(v);
            if next == t {
                break;
            }
            t = next;
        }
        t
    }

    /// Whether `t` is a value each lane holds its own of (one in VGPRs).
    fn varies(&self, t: Term) -> bool {
        let Term::Value(v) = t else { return false };
        let reg = match (self.inputs[v.0 as usize], self.phis[v.0 as usize]) {
            (Some(reg), _) => Some(reg),
            (None, Some((b, i))) => Some(self.p.blocks[b].phis[i].reg),
            (None, None) => self
                .def(v)
                .and_then(|i| i.writes.iter().find(|w| w.1 == v))
                .map(|w| w.0),
        };
        matches!(reg, Some(Reg::V(_)))
    }

    /// The source of descriptor dwords each lane holds its own of (a
    /// waterfall loop reads them from the first active lane): the same in
    /// every lane, loaded per lane from memory a heap address points to, or
    /// picked per lane between such sources.
    fn per_lane(&self, words: &[Term], joins: &mut Vec<usize>) -> Result<Source, &'static str> {
        let Some(&lane) = words.iter().find(|&&w| self.varies(w)) else {
            return self.classify(words, joins);
        };
        if let Some(source) = self.lane_phi(words, joins)? {
            return Ok(source);
        }
        if let Some(source) = self.fetched(words)? {
            return Ok(source);
        }
        if let Some(source) = self.lane_select(words, joins)? {
            return Ok(source);
        }
        let Term::Value(v) = lane else { unreachable!() };
        let name = self.def(v).map(|i| i.name());
        Err(match name {
            _ if self.inputs[v.0 as usize].is_some() => {
                "a descriptor read from the first lane of an input VGPR"
            }
            None => "a descriptor read from the first lane of a phi",
            Some(n) if n.starts_with("ds_read") => {
                "a descriptor read from the first lane of an LDS read"
            }
            Some(n) if n.starts_with("buffer_load") => {
                "a descriptor read from the first lane of a buffer load"
            }
            Some(n) if n.starts_with("global_load") || n.starts_with("flat_load") => {
                "a descriptor read from the first lane of a partial or mixed load"
            }
            Some(_) => "a descriptor read from the first lane of computed VGPRs",
        })
    }

    /// Per-lane words that phis join: a phi that carries one dword is that
    /// dword, other joins are traced per path into their block
    /// ([`Source::Paths`]).
    fn lane_phi(
        &self,
        words: &[Term],
        joins: &mut Vec<usize>,
    ) -> Result<Option<Source>, &'static str> {
        let phi_of = |w: &Term| match *w {
            Term::Value(v) => self.phis[v.0 as usize],
            Term::Const(_) => None,
        };
        if words.iter().all(|w| phi_of(w).is_none()) {
            return Ok(None);
        }
        let carried: Vec<Term> = words
            .iter()
            .map(|&w| match phi_of(&w) {
                Some(_) => self.joined_one(w).unwrap_or(w),
                None => w,
            })
            .collect();
        if carried != words {
            return self.per_lane(&carried, joins).map(Some);
        }
        // Joined in several blocks: the last is looked at first, and its
        // edges bring what the others joined.
        let Some(block) = words.iter().filter_map(phi_of).map(|p| p.0).max() else {
            unreachable!()
        };
        if joins.contains(&block) {
            // Back at a join being traced: through a loop's back edge, the
            // dwords it joined on the previous pass.
            return match self.lane_joins.borrow().iter().find(|j| j.0 == block) {
                Some((_, w)) if w == words => Ok(Some(Source::Carried { block })),
                _ => Err("a per-lane descriptor that changes in a loop"),
            };
        }
        if joins.len() >= MAX_JOINS {
            return Err("a descriptor joined too often");
        }
        joins.push(block);
        self.lane_joins.borrow_mut().push((block, words.to_vec()));
        let phis = &self.p.blocks[block].phis;
        let args = (0..self.p.blocks[block].preds.len())
            .map(|j| {
                let edge: Vec<Term> = words
                    .iter()
                    .map(|w| match phi_of(w) {
                        Some((b, i)) if b == block => {
                            self.lane_term(Term::Value(phis[i].args[j]))
                        }
                        _ => *w,
                    })
                    .collect();
                if edge.iter().all(|&t| self.unset_input(t)) {
                    let mut unset = self.unset.borrow_mut();
                    unset.extend(edge.iter().filter_map(|t| match *t {
                        Term::Value(v) => Some(v),
                        Term::Const(_) => None,
                    }));
                    return Ok(Source::Unset);
                }
                self.per_lane(&edge, joins)
            })
            .collect::<Vec<Result<Source, &'static str>>>();
        self.lane_joins.borrow_mut().pop();
        joins.pop();
        // shortcut: where other paths bring descriptors, one that brings
        // something else holds the registers' former use: no lane that came
        // that way reads them as a descriptor.
        let known = args.iter().any(|a| !matches!(a, Err(_) | Ok(Source::Unset | Source::Carried { .. })));
        let args = args
            .into_iter()
            .map(|a| match a {
                Err(why)
                    if known
                        && why.starts_with("a descriptor read from the first lane of")
                        && !why.ends_with("LDS read") =>
                {
                    Ok(Source::Unset)
                }
                a => a,
            })
            .collect::<Result<_, _>>();
        Ok(Some(Source::Paths { block, args: args? }))
    }

    /// The one dword the phi `t` carries: what the arguments of the phis it
    /// joins through come to (through copies), past VGPRs no one wrote.
    fn joined_one(&self, t: Term) -> Option<Term> {
        let (mut seen, mut todo, mut leaves) = (Vec::new(), vec![t], Vec::new());
        while let Some(t) = todo.pop() {
            let t = self.lane_term(t);
            match t {
                Term::Value(v) if let Some((b, i)) = self.phis[v.0 as usize] => {
                    if seen.contains(&v) {
                        continue;
                    }
                    if seen.len() >= MAX_POINTER_STEPS {
                        return None;
                    }
                    seen.push(v);
                    let phi = &self.p.blocks[b].phis[i];
                    todo.extend(phi.args.iter().map(|&a| Term::Value(a)));
                }
                t if !leaves.contains(&t) => leaves.push(t),
                _ => {}
            }
        }
        let (unset, set): (Vec<Term>, Vec<Term>) =
            leaves.into_iter().partition(|&l| self.unset_input(l));
        let [one] = set[..] else { return None };
        self.unset
            .borrow_mut()
            .extend(unset.into_iter().filter_map(|t| match t {
                Term::Value(v) => Some(v),
                Term::Const(_) => None,
            }));
        Some(one)
    }

    /// Whether `t` is a VGPR read before any write (undefined unless the
    /// hardware loads it).
    fn unset_input(&self, t: Term) -> bool {
        matches!(t, Term::Value(v) if matches!(self.inputs[v.0 as usize], Some(Reg::V(_))))
    }

    /// Dwords of `global_load_dword*` that line up: `first..` of one load,
    /// then the bytes that follow (an image descriptor takes two
    /// `global_load_dwordx4`).
    fn fetched(&self, words: &[Term]) -> Result<Option<Source>, &'static str> {
        let at = |w: &Term| -> Option<(Site, usize)> {
            let Term::Value(v) = *w else { return None };
            let site = self.defs[v.0 as usize]?;
            let inst = self.inst(site);
            if !inst.name().starts_with("global_load_dword")
                && !inst.name().starts_with("buffer_load_dword")
            {
                return None;
            }
            let k = inst
                .writes
                .iter()
                .filter(|w| matches!(w.0, Reg::V(_)))
                .position(|w| w.1 == v)?;
            Some((site, k))
        };
        let Some(sites) = words.iter().map(at).collect::<Option<Vec<_>>>() else {
            return Ok(None);
        };
        let address = |site: Site| {
            let inst = self.inst(site);
            if matches!(inst.fields, Fields::Buffer { .. }) {
                return self.buffer_address(inst);
            }
            let Fields::Flat {
                addr,
                saddr,
                offset,
                ..
            } = &inst.fields
            else {
                return Err("a global load");
            };
            self.flat_address(inst, addr.as_ref(), saddr.as_ref(), *offset)
        };
        let (load, first) = sites[0];
        let a0 = address(load)?;
        for (k, &(site, dword)) in sites.iter().enumerate() {
            let a = address(site)?;
            let want = a0.offset.wrapping_add(4 * (first + k) as u32);
            if (a.param, a.dynamic, a.includes_base) != (a0.param, a0.dynamic, a0.includes_base)
                || a.offset.wrapping_add(4 * dword as u32) != want
            {
                return Err("a descriptor read from the first lane of loads that do not line up");
            }
        }
        Ok(Some(Source::Fetched {
            load,
            first: first as u8,
        }))
    }

    /// Words that `v_cndmask_b32` picks by one lane mask, traced per outcome
    /// (other words are the same for both).
    fn lane_select(
        &self,
        words: &[Term],
        joins: &mut Vec<usize>,
    ) -> Result<Option<Source>, &'static str> {
        let picks: Vec<_> = words
            .iter()
            .map(|w| match *w {
                Term::Value(v) => self.lane_picked(v),
                Term::Const(_) => None,
            })
            .collect();
        // Peel the outermost (highest-SSA) mask first; any word selected by a
        // different mask stays intact for the recursive `per_lane` call.
        let Some(mask) = picks.iter().flatten().map(|p| p.0).max() else {
            return Ok(None);
        };
        if joins.len() >= MAX_JOINS {
            return Err("a descriptor joined too often");
        }
        joins.push(usize::MAX);
        let side = |set: bool| -> Vec<Term> {
            words
                .iter()
                .zip(&picks)
                .map(|(w, p)| match p {
                    Some((m, t, f)) if *m == mask => {
                        if set {
                            *t
                        } else {
                            *f
                        }
                    }
                    _ => *w,
                })
                .collect()
        };
        let args = [true, false]
            .into_iter()
            .map(|set| self.per_lane(&side(set), joins))
            .collect::<Result<_, _>>();
        joins.pop();
        Ok(Some(Source::Lanes { mask, args: args? }))
    }

    /// For `v` written by a plain `v_cndmask_b32`: the low dword of the
    /// lane mask it reads, and the dword it is where the lane's bit is set
    /// (src1), then clear (src0).
    fn lane_picked(&self, v: Value) -> Option<(Value, Term, Term)> {
        let inst = self.def(v)?;
        let Fields::Valu(f) = &inst.fields else {
            return None;
        };
        let Some(Operand::Vector { reg, .. }) = f.dst else {
            return None;
        };
        if inst.name() != "v_cndmask_b32"
            || inst.write(Reg::V(reg)) != Some(v)
            || f.abs != 0
            || f.neg != 0
            || f.clamp
            || f.omod != 0
            || f.sdwa.is_some()
            || f.dpp.is_some()
        {
            return None;
        }
        let [clear, set, Operand::Scalar { code, .. }] = f.src[..] else {
            return None;
        };
        let mask = inst.read(Reg::S(code))?;
        let side = |op: &Operand| self.vterm(inst, op).map(|t| self.lane_term(t));
        Some((mask, side(&set)?, side(&clear)?))
    }

    /// The waterfall loops over descriptors read from the first lane (the
    /// `v_readfirstlane_b32` results `reads`): the `v_cmp_eq` of those
    /// results with the dwords they read.
    fn waterfall(&self, reads: &[Value]) -> Waterfall {
        let mut out = Waterfall::default();
        if reads.is_empty() {
            return out;
        }
        for (b, block) in self.p.blocks.iter().enumerate() {
            for (i, inst) in block.insts.iter().enumerate() {
                let Fields::Valu(f) = &inst.fields else {
                    continue;
                };
                let Some(ty) = inst
                    .name()
                    .strip_prefix("v_cmp_eq_")
                    .or(inst.name().strip_prefix("v_cmpx_eq_"))
                else {
                    continue;
                };
                let dwords = match ty {
                    "u32" | "i32" => 1,
                    "u64" | "i64" => 2,
                    _ => continue,
                };
                if f.abs != 0 || f.neg != 0 || f.sdwa.is_some() || f.dpp.is_some() {
                    continue;
                }
                // The first lane's dwords on one side, the lane's own on the
                // other.
                let pair = |first: &Operand, own: &Operand| -> Option<Vec<Value>> {
                    let (&Operand::Scalar { code, .. }, &Operand::Vector { reg, .. }) =
                        (first, own)
                    else {
                        return None;
                    };
                    (0..dwords)
                        .map(|k| {
                            let Term::Value(x) = self.origin(inst.read(Reg::S(code + k))?) else {
                                return None;
                            };
                            let lane = inst.read(Reg::V(reg + k))?;
                            let same = reads.contains(&x)
                                && self.lane_term(Term::Value(self.first_lane_source(x)?))
                                    == self.lane_term(Term::Value(lane));
                            same.then_some(x)
                        })
                        .collect()
                };
                let (Some(a), Some(c)) = (f.src.first(), f.src.get(1)) else {
                    continue;
                };
                if let Some(xs) = pair(a, c).or_else(|| pair(c, a)) {
                    out.compares.push((b, i));
                    for x in xs {
                        if !out.reads.contains(&x) {
                            out.reads.push(x);
                        }
                    }
                }
            }
        }
        out
    }

    /// Consecutive dwords of one scalar load.
    /// A buffer descriptor loaded and made a raw one (as AMD's ACO compiler
    /// does for byte address buffers): no stride, its bounds in bytes. The
    /// host's view of the slot is of the bytes already.
    fn made_raw(&self, words: &[Term]) -> Option<Source> {
        let [first, stride, records, flags] = *words else {
            return None;
        };
        // `x` with constant bits cleared or set.
        let masked = |w: Term, op: &'static str| -> Option<(Term, u32)> {
            let Term::Value(v) = w else { return None };
            match self.sop2(v, &[op])? {
                (_, [x, Term::Const(c)] | [Term::Const(c), x]) => Some((x, c)),
                _ => None,
            }
        };
        const STRIDE: u32 = 0x3fff_0000;
        const BOUNDS: u32 = 0x3000_0000;
        let (stride, kept) = masked(stride, "s_and_b32")?;
        let (flags, set) = masked(flags, "s_or_b32")?;
        let flags = masked(flags, "s_and_b32").map_or(flags, |(x, _)| x);
        if kept != !STRIDE || set != BOUNDS {
            return None;
        }
        let Term::Value(records) = records else {
            return None;
        };
        let (_, [a, b]) = self.sop2(records, &["s_mul_i32"])?;
        [a, b]
            .into_iter()
            .find_map(|records| self.loaded(&[first, stride, records, flags]))
    }

    fn loaded(&self, words: &[Term]) -> Option<Source> {
        let Term::Value(v0) = words[0] else {
            return None;
        };
        let load = self.defs[v0.0 as usize]?;
        let inst = self.inst(load);
        if !inst.name().starts_with("s_load_") {
            return None;
        }
        let first = inst.writes.iter().position(|w| w.1 == v0)?;
        let same = words.iter().enumerate().all(|(k, w)| {
            inst.writes
                .get(first + k)
                .is_some_and(|x| *w == Term::Value(x.1))
        });
        same.then_some(Source::Loaded {
            load,
            first: first as u8,
        })
    }
}

#[derive(Clone, Copy, PartialEq, Eq)]
enum Moved {
    Const(u32),
    Value(Value),
}

impl Tracer<'_> {
    /// For `v_readlane_b32` of a constant lane: the SGPR spilled to that
    /// lane, found by following the VGPR's `v_writelane_b32` writes.
    fn restored(&self, inst: &Inst) -> Option<Moved> {
        let Fields::Valu(v) = &inst.fields else {
            return None;
        };
        if inst.name() != "v_readlane_b32" {
            return None;
        }
        let [Operand::Vector { reg, .. }, lane] = v.src[..] else {
            return None;
        };
        self.spilled(inst.read(Reg::V(reg))?, lane_of(lane)?, &mut Vec::new())
            .ok()
            .flatten()
    }

    /// What lane `lane` of VGPR value `v` holds: `Ok(None)` for a phi in
    /// `seen` (already being resolved), `Err` if not a spilled SGPR.
    fn spilled(&self, v: Value, lane: u32, seen: &mut Vec<Value>) -> Result<Option<Moved>, ()> {
        if let Some((b, i)) = self.phis[v.0 as usize] {
            if seen.contains(&v) {
                return Ok(None);
            }
            seen.push(v);
            let mut found = None;
            for &a in &self.p.blocks[b].phis[i].args {
                if let Some(m) = self.spilled(a, lane, seen)? {
                    if found.is_some_and(|f| f != m) {
                        return Err(());
                    }
                    found = Some(m);
                }
            }
            return Ok(found);
        }
        let inst = self.def(v).ok_or(())?;
        let Fields::Valu(w) = &inst.fields else {
            return Err(());
        };
        let (Some(Operand::Vector { reg, .. }), [value, at]) = (w.dst, &w.src[..]) else {
            return Err(());
        };
        if inst.name() != "v_writelane_b32" {
            return Err(());
        }
        if lane_of(*at).ok_or(())? != lane {
            return self.spilled(inst.read(Reg::V(reg)).ok_or(())?, lane, seen);
        }
        match *value {
            Operand::Scalar { code, .. } if code != NULL => {
                inst.read(Reg::S(code)).map(Moved::Value)
            }
            Operand::Literal(x) => Some(Moved::Const(x)),
            Operand::Const(c) => inline(c).map(Moved::Const),
            _ => None,
        }
        .map(Some)
        .ok_or(())
    }
}

/// A lane select given as a constant.
fn lane_of(op: Operand) -> Option<u32> {
    match op {
        Operand::Const(c) => inline(c),
        Operand::Literal(x) => Some(x),
        _ => None,
    }
}

/// What dword `k` of a move's result copies.
fn copy_source(inst: &Inst, k: usize) -> Option<Moved> {
    let Fields::Sop { src, imm, .. } = &inst.fields else {
        return None;
    };
    if inst.name() == "s_movk_i32" {
        return Some(Moved::Const(i32::from((*imm)? as i16) as u32));
    }
    let src = src.first()?;
    match (inst.name(), *src) {
        ("s_mov_b32" | "s_mov_b64", Operand::Scalar { code, .. }) if code != NULL => {
            inst.read(Reg::S(code + k as u8)).map(Moved::Value)
        }
        ("s_mov_b32" | "s_mov_b64", Operand::Scalar { .. }) => Some(Moved::Const(0)),
        ("s_mov_b32", Operand::Literal(x)) => Some(Moved::Const(x)),
        ("s_mov_b32", Operand::Const(c)) => inline(c).map(Moved::Const),
        // shortcut: only the one-operand ways the compiler makes a constant
        // that has no inline form; fold more if descriptors need it.
        ("s_brev_b32", Operand::Const(c)) => inline(c).map(|x| Moved::Const(x.reverse_bits())),
        ("s_not_b32", Operand::Const(c)) => inline(c).map(|x| Moved::Const(!x)),
        // Bits `src0` of them from bit `src1`.
        ("s_bfm_b32", Operand::Const(a)) => {
            let Some(&Operand::Const(b)) = (match &inst.fields {
                Fields::Sop { src, .. } => src.get(1),
                _ => None,
            }) else {
                return None;
            };
            let (n, at) = (inline(a)? & 31, inline(b)? & 31);
            Some(Moved::Const(((1u32 << n) - 1) << at))
        }
        ("s_mov_b64", Operand::Literal(x)) => Some(Moved::Const(if k == 0 { x } else { 0 })),
        // 64-bit inline integers are sign-extended.
        ("s_mov_b64", Operand::Const(c @ 128..=208)) => {
            let low = inline(c)?;
            Some(Moved::Const(if k == 0 {
                low
            } else {
                ((low as i32) >> 31) as u32
            }))
        }
        _ => None,
    }
}

/// The 32-bit value of an inline constant operand.
pub(crate) fn inline(code: u8) -> Option<u32> {
    let float = |f: f32| Some(f.to_bits());
    match code {
        128..=192 => Some(u32::from(code) - 128),
        193..=208 => Some((192 - i32::from(code)) as u32),
        240 => float(0.5),
        241 => float(-0.5),
        242 => float(1.0),
        243 => float(-1.0),
        244 => float(2.0),
        245 => float(-2.0),
        246 => float(4.0),
        247 => float(-4.0),
        248 => Some(0x3E22_F983),
        _ => None,
    }
}

#[cfg(test)]
pub(crate) mod tests {
    use super::*;
    use crate::native::RootType::*;

    const S_ENDPGM: u32 = 0xBF81_0000;

    fn track_words(words: &[u32], layout: &[RootParam], stage: HwStage) -> Resources {
        let code: Vec<u8> = words.iter().flat_map(|w| w.to_le_bytes()).collect();
        track(&crate::ir::build(&code, false, 256).unwrap(), layout, stage).unwrap()
    }

    fn param(root: RootType, user_data: u8, sgprs: u32) -> RootParam {
        RootParam {
            root,
            table_offset_dwords: 0,
            sgprs,
            base_register: 0,
            user_data,
            slot: 0,
        }
    }

    fn at(param: usize, offset: u32) -> Address {
        Address {
            param,
            offset,
            dynamic: None,
            includes_base: false,
        }
    }

    #[test]
    fn table_descriptors_are_loaded_from_their_range() {
        let r = track_words(
            &[
                0xBE82_0309, // s_mov_b32 s2, s9
                0xBE89_0380, // s_mov_b32 s9, 0
                0xBE83_0380, // s_mov_b32 s3, 0
                0xF408_0204,
                0xFA00_0000, // s_load_dwordx4 s[8:11], s[8:9], null
                0xF40C_0001,
                0xFA00_0000, // s_load_dwordx8 s[0:7], s[2:3], null
                0xF080_0F08,
                0x0040_0002, // image_sample v[0:3], v[2:3], s[0:7], s[8:11]
                S_ENDPGM,
            ],
            &[param(Table, 8, 1), param(Table, 9, 1)],
            HwStage::Ps,
        );
        assert_eq!(r.scalar, [((0, 3), at(0, 0)), ((0, 4), at(1, 0))]);
        let roles: Vec<_> = r.descriptors.iter().map(|d| (d.role, &d.source)).collect();
        assert_eq!(
            roles,
            [
                (
                    Role::Resource,
                    &Source::Loaded {
                        load: (0, 4),
                        first: 0
                    }
                ),
                (
                    Role::Sampler,
                    &Source::Loaded {
                        load: (0, 3),
                        first: 0
                    }
                ),
            ]
        );
    }

    #[test]
    fn root_descriptors_are_built_in_code() {
        let r = track_words(
            &[
                0x8801_FF0B,
                0x0010_0000, // s_or_b32 s1, s11, 0x100000
                0xBE80_030A, // s_mov_b32 s0, s10
                0xBE82_03C1, // s_mov_b32 s2, -1
                0xBE83_03FF,
                0x1101_4FAC, // s_mov_b32 s3, 0x11014fac
                0xF430_0000,
                0xFA00_0000, // s_buffer_load_dwordx16 s[0:15], s[0:3], null
                S_ENDPGM,
            ],
            &[param(Descriptor, 10, 2)],
            HwStage::Vs,
        );
        assert_eq!(
            r.descriptors[0].source,
            Source::Built {
                address: at(0, 0),
                bits: 0x10_0000,
                words: [u32::MAX, 0x1101_4FAC]
            }
        );
        assert_eq!(
            r.params.iter().map(|p| (p.1, p.2)).collect::<Vec<_>>(),
            [(0, 0), (0, 1)]
        );
    }

    #[test]
    fn heap_indices_from_root_constants_in_a_merged_stage() {
        // User data starts at s8: the constant (index 6) is s14, the heap
        // (index 10) s18.
        let r = track_words(
            &[
                0x8F08_850E, // s_lshl_b32 s8, s14, 5
                0xBE93_0380, // s_mov_b32 s19, 0
                0xF408_0209,
                0x1000_0000, // s_load_dwordx4 s[8:11], s[18:19], s8
                0xE000_2000,
                0x8002_0400, // buffer_load_format_x v4, v0, s[8:11], 0 idxen
                S_ENDPGM,
            ],
            &[param(Constants, 6, 1), param(ResourceHeap, 10, 1)],
            HwStage::Ms,
        );
        let index = r.params.iter().find(|p| p.1 == 0).unwrap().0;
        let shift = r.scalar[0].1.dynamic.unwrap();
        assert_eq!(r.scalar[0].1.param, 1);
        // The computed term is the shifted constant, not the constant itself.
        assert_ne!(shift, index);
        assert_eq!(
            r.descriptors[0].source,
            Source::Loaded {
                load: (0, 2),
                first: 0
            }
        );
    }

    #[test]
    fn spilled_sgprs_are_followed_through_vgpr_lanes() {
        let r = track_words(
            &[
                0xD761_0000,
                0x0001_0609, // v_writelane_b32 v0, s9, 3
                0xD760_0002,
                0x0001_0700, // v_readlane_b32 s2, v0, 3
                0xBE83_0380, // s_mov_b32 s3, 0
                0xF40C_0001,
                0xFA00_0020, // s_load_dwordx8 s[0:7], s[2:3], 0x20
                S_ENDPGM,
            ],
            &[param(Table, 9, 1)],
            HwStage::Ps,
        );
        assert_eq!(r.scalar, [((0, 3), at(0, 0x20))]);
    }

    #[test]
    fn descriptors_joined_by_control_flow_are_traced_per_edge() {
        let r = track_words(
            &[
                0xBE82_0309, // s_mov_b32 s2, s9
                0xBE83_0380, // s_mov_b32 s3, 0
                0xBF06_8001, // s_cmp_eq_u32 s1, 0
                0xBF85_0003, // s_cbranch_scc1 -> 0x1c
                0xF408_0001,
                0xFA00_0000, // s_load_dwordx4 s[0:3], s[2:3], null
                0xBF82_0002, // s_branch -> 0x24
                0xF408_0001,
                0xFA00_0010, // 0x1c: s_load_dwordx4 s[0:3], s[2:3], 0x10
                0xF420_0100,
                0xFA00_0000, // 0x24: s_buffer_load_dword s4, s[0:3], null
                S_ENDPGM,
            ],
            &[param(Table, 9, 1)],
            HwStage::Ps,
        );
        let Source::Phi { args, .. } = &r.descriptors[0].source else {
            panic!("{:?}", r.descriptors)
        };
        let offsets: Vec<_> = args
            .iter()
            .map(|a| match a {
                Source::Loaded { load, .. } => {
                    r.scalar.iter().find(|s| s.0 == *load).unwrap().1.offset
                }
                a => panic!("{a:?}"),
            })
            .collect();
        assert_eq!(offsets.len(), 2);
        assert!(
            offsets.contains(&0) && offsets.contains(&0x10),
            "{offsets:?}"
        );
    }

    #[test]
    fn descriptors_selected_by_scc_are_traced_per_outcome() {
        // Two buffer descriptors loaded from the table at s9 and one
        // picked with s_cselect_b64 per half, as the compiler makes a
        // conditional choice of resources without a branch.
        let r = track_words(
            &[
                0xBE8C_0309, // s_mov_b32 s12, s9
                0xBE8D_0380, // s_mov_b32 s13, 0
                0xF408_0006,
                0xFA00_0000, // s_load_dwordx4 s[0:3], s[12:13], null
                0xF408_0106,
                0xFA00_0010, // s_load_dwordx4 s[4:7], s[12:13], 0x10
                0xBF06_800E, // s_cmp_eq_u32 s14, 0
                0x8588_0400, // s_cselect_b64 s[8:9], s[0:1], s[4:5]
                0x858A_0602, // s_cselect_b64 s[10:11], s[2:3], s[6:7]
                0xF420_0404,
                0xFA00_0000, // s_buffer_load_dword s16, s[8:11], null
                S_ENDPGM,
            ],
            &[param(Table, 9, 1)],
            HwStage::Ps,
        );
        let Source::Select { args, .. } = &r.descriptors[0].source else {
            panic!("{:?}", r.descriptors)
        };
        // SCC set picks the first operand.
        assert_eq!(
            args[..],
            [
                Source::Loaded {
                    load: (0, 2),
                    first: 0
                },
                Source::Loaded {
                    load: (0, 3),
                    first: 0
                },
            ]
        );
        assert_eq!(r.scalar, [((0, 2), at(0, 0)), ((0, 3), at(0, 0x10))]);
    }

    #[test]
    fn a_selection_that_mixes_two_descriptors_fails() {
        // The high half picked from the first load whatever SCC is: when
        // clear, the descriptor is half of one load and half of the other.
        let code: Vec<u8> = [
            0xBE8C_0309, // s_mov_b32 s12, s9
            0xBE8D_0380, // s_mov_b32 s13, 0
            0xF408_0006,
            0xFA00_0000, // s_load_dwordx4 s[0:3], s[12:13], null
            0xF408_0106,
            0xFA00_0010, // s_load_dwordx4 s[4:7], s[12:13], 0x10
            0xBF06_800E, // s_cmp_eq_u32 s14, 0
            0x8588_0400, // s_cselect_b64 s[8:9], s[0:1], s[4:5]
            0x858A_0202, // s_cselect_b64 s[10:11], s[2:3], s[2:3]
            0xF420_0404,
            0xFA00_0000, // s_buffer_load_dword s16, s[8:11], null
            S_ENDPGM,
        ]
        .iter()
        .flat_map(|w: &u32| w.to_le_bytes())
        .collect();
        let p = crate::ir::build(&code, false, 256).unwrap();
        let err = track(&p, &[param(Table, 9, 1)], HwStage::Ps).unwrap_err();
        assert_eq!(
            err,
            ShaderError::Unsupported {
                offset: 36,
                what: "a descriptor changed after loading"
            }
        );
    }

    #[test]
    fn root_constants_are_offsets_added_to_a_table() {
        let r = track_words(
            &[
                0x8002_0409, // s_add_u32 s2, s9, s4
                0xB782_0100, // s_addk_i32 s2, 0x100
                0xBE83_0380, // s_mov_b32 s3, 0
                0xF40C_0101,
                0xFA00_0000, // s_load_dwordx8 s[4:11], s[2:3], null
                S_ENDPGM,
            ],
            &[param(Table, 9, 1), param(Constants, 4, 1)],
            HwStage::Ps,
        );
        let constant = r.params.iter().find(|p| p.1 == 1).unwrap().0;
        assert_eq!(
            r.scalar,
            [(
                (0, 3),
                Address {
                    param: 0,
                    offset: 0x100,
                    dynamic: Some(constant),
                    includes_base: false,
                }
            )]
        );
    }

    #[test]
    fn constants_subtracted_from_a_root_descriptor_borrow_from_its_high_half() {
        let r = track_words(
            &[
                0x8080_900A, // s_sub_u32 s0, s10, 16
                0x8281_800B, // s_subb_u32 s1, s11, 0
                0xF400_0080,
                0xFA00_0020, // s_load_dword s2, s[0:1], 0x20
                S_ENDPGM,
            ],
            &[param(Descriptor, 10, 2)],
            HwStage::Vs,
        );
        assert_eq!(r.scalar, [((0, 2), at(0, 0x10))]);
    }

    /// The phi a pointer address reads, and the block it joins in.
    fn pointer_phi(p: &Program, a: &Address) -> usize {
        let d = a.dynamic.expect("a computed term");
        p.blocks
            .iter()
            .position(|b| b.phis.iter().any(|phi| phi.value == d))
            .expect("a phi")
    }

    #[test]
    fn pointers_advanced_in_a_loop_are_offsets_from_their_table() {
        let code: Vec<u8> = [
            0xBE82_0309, // s_mov_b32 s2, s9
            0xBE83_0380, // s_mov_b32 s3, 0
            0xBE84_0380, // s_mov_b32 s4, 0
            0xF40C_0201,
            0xFA00_0000, // 0xc: s_load_dwordx8 s[8:15], s[2:3], null
            0x8002_A002, // s_add_u32 s2, s2, 32
            0x8203_8003, // s_addc_u32 s3, s3, 0
            0x8004_8104, // s_add_u32 s4, s4, 1
            0xBF07_8404, // s_cmp_lg_u32 s4, 4
            0xBF85_FFF9, // s_cbranch_scc1 -> 0xc
            S_ENDPGM,
        ]
        .iter()
        .flat_map(|w: &u32| w.to_le_bytes())
        .collect();
        let p = crate::ir::build(&code, false, 256).unwrap();
        let r = track(&p, &[param(Table, 9, 1)], HwStage::Ps).unwrap();
        let [(site, a)] = r.scalar[..] else {
            panic!("{:?}", r.scalar)
        };
        assert_eq!((a.param, a.offset, a.includes_base), (0, 0, true));
        // The pointer is the loop header's phi of s2.
        assert_eq!(pointer_phi(&p, &a), site.0);
    }

    #[test]
    fn pointers_picked_by_a_branch_are_offsets_from_their_table() {
        let code: Vec<u8> = [
            0xBF06_8001, // s_cmp_eq_u32 s1, 0
            0xBF85_0002, // s_cbranch_scc1 -> 0x10
            0x8002_A009, // s_add_u32 s2, s9, 32
            0xBF82_0001, // s_branch -> 0x14
            0x8002_C009, // 0x10: s_add_u32 s2, s9, 64
            0xBE83_0380, // 0x14: s_mov_b32 s3, 0
            0xF40C_0201,
            0xFA00_0010, // s_load_dwordx8 s[8:15], s[2:3], 0x10
            S_ENDPGM,
        ]
        .iter()
        .flat_map(|w: &u32| w.to_le_bytes())
        .collect();
        let p = crate::ir::build(&code, false, 256).unwrap();
        let r = track(&p, &[param(Table, 9, 1)], HwStage::Ps).unwrap();
        let [(site, a)] = r.scalar[..] else {
            panic!("{:?}", r.scalar)
        };
        assert_eq!((a.param, a.offset, a.includes_base), (0, 0x10, true));
        assert_eq!(pointer_phi(&p, &a), site.0);
    }

    #[test]
    fn pointers_into_a_root_descriptor_carry_into_its_high_half() {
        let r = track_words(
            &[
                0xBE80_030A, // s_mov_b32 s0, s10
                0xBE81_030B, // s_mov_b32 s1, s11
                0xF400_0080,
                0xFA00_0000, // 0x8: s_load_dword s2, s[0:1], null
                0x8000_9000, // s_add_u32 s0, s0, 16
                0x8201_8001, // s_addc_u32 s1, s1, 0
                0xBF06_8002, // s_cmp_eq_u32 s2, 0
                0xBF84_FFFA, // s_cbranch_scc0 -> 0x8
                S_ENDPGM,
            ],
            &[param(Descriptor, 10, 2)],
            HwStage::Vs,
        );
        let a = r.scalar[0].1;
        assert_eq!((a.param, a.offset, a.includes_base), (0, 0, true));
    }

    #[test]
    fn pointers_from_two_parameters_fail() {
        let code: Vec<u8> = [
            0xBF06_8001, // s_cmp_eq_u32 s1, 0
            0xBF85_0002, // s_cbranch_scc1 -> 0x10
            0x8002_A009, // s_add_u32 s2, s9, 32
            0xBF82_0001, // s_branch -> 0x14
            0x8002_C008, // 0x10: s_add_u32 s2, s8, 64
            0xBE83_0380, // 0x14: s_mov_b32 s3, 0
            0xF40C_0201,
            0xFA00_0000, // 0x18: s_load_dwordx8 s[8:15], s[2:3], null
            S_ENDPGM,
        ]
        .iter()
        .flat_map(|w: &u32| w.to_le_bytes())
        .collect();
        let p = crate::ir::build(&code, false, 256).unwrap();
        let err = track(&p, &[param(Table, 8, 1), param(Table, 9, 1)], HwStage::Ps).unwrap_err();
        assert_eq!(
            err,
            ShaderError::Unsupported {
                offset: 0x18,
                what: "an address from a phi"
            }
        );
    }

    #[test]
    fn untraceable_addresses_fail_with_a_reason() {
        let code: Vec<u8> = [0xF408_0001, 0xFA00_0000, S_ENDPGM] // s_load_dwordx4 s[0:3], s[2:3]
            .iter()
            .flat_map(|w: &u32| w.to_le_bytes())
            .collect();
        let p = crate::ir::build(&code, false, 256).unwrap();
        let err = track(&p, &[param(Table, 9, 1)], HwStage::Ps).unwrap_err();
        assert_eq!(
            err,
            ShaderError::Unsupported {
                offset: 0,
                what: "an address in an SGPR that is no root parameter"
            }
        );
    }

    /// A waterfall loop over an image descriptor each lane loads from the
    /// table at s0 (wave32, assembled with llvm-mc for gfx1030): lane i
    /// reads entry i with two `global_load_dwordx4`, the loop reads the
    /// first active lane's descriptor into s[12:19], samples with the lanes
    /// whose descriptor it is, and repeats for the others. The sampler comes
    /// from the table at s1; the result goes to LDS so the sample is needed.
    pub(crate) const WATERFALL_FETCHED_WAVE32: &[u32] = &[
        0xBE84_0300, // s_mov_b32 s4, s0
        0xBE85_0380, // s_mov_b32 s5, 0
        0xBE86_0301, // s_mov_b32 s6, s1
        0xBE87_0380, // s_mov_b32 s7, 0
        0x3402_0085, // v_lshlrev_b32 v1, 5, v0
        0xDC38_8000,
        0x0404_0001, // global_load_dwordx4 v[4:7], v1, s[4:5]
        0xDC38_8010,
        0x0804_0001, // global_load_dwordx4 v[8:11], v1, s[4:5] offset:16
        0xF408_0203,
        0xFA00_0000, // s_load_dwordx4 s[8:11], s[6:7], 0x0
        0x7E04_0D00, // v_cvt_f32_u32 v2, v0
        0x7E06_0302, // v_mov_b32 v3, v2
        0xBF8C_0070, // s_waitcnt vmcnt(0) lgkmcnt(0)
        0xBE94_037E, // s_mov_b32 s20, exec_lo
        0x7E18_0504, // 0x3c: v_readfirstlane_b32 s12, v4
        0x7E1A_0505, // v_readfirstlane_b32 s13, v5
        0x7E1C_0506, // v_readfirstlane_b32 s14, v6
        0x7E1E_0507, // v_readfirstlane_b32 s15, v7
        0x7E20_0508, // v_readfirstlane_b32 s16, v8
        0x7E22_0509, // v_readfirstlane_b32 s17, v9
        0x7E24_050A, // v_readfirstlane_b32 s18, v10
        0x7E26_050B, // v_readfirstlane_b32 s19, v11
        0xD4E2_0015,
        0x0002_080C, // v_cmp_eq_u64_e64 s21, s[12:13], v[4:5]
        0xD4E2_0016,
        0x0002_0C0E, // v_cmp_eq_u64_e64 s22, s[14:15], v[6:7]
        0x8715_1615, // s_and_b32 s21, s21, s22
        0xD4E2_0016,
        0x0002_1010, // v_cmp_eq_u64_e64 s22, s[16:17], v[8:9]
        0x8715_1615, // s_and_b32 s21, s21, s22
        0xD4E2_0016,
        0x0002_1412, // v_cmp_eq_u64_e64 s22, s[18:19], v[10:11]
        0x8715_1615, // s_and_b32 s21, s21, s22
        0xBE95_3C15, // s_and_saveexec_b32 s21, s21
        0xBF80_0000, // s_nop 0
        0xF09C_0F08,
        0x0043_0C02, // image_sample_lz v[12:15], v[2:3], s[12:19], s[8:11] dmask:0xf dim:SQ_RSRC_IMG_2D
        0xBF8C_3F70, // s_waitcnt vmcnt(0)
        0x897E_157E, // s_xor_b32 exec_lo, exec_lo, s21
        0xBF89_FFE6, // s_cbranch_execnz -> 0x3c
        0xBEFE_0314, // s_mov_b32 exec_lo, s20
        0xD834_0000,
        0x0000_0C00, // ds_write_b32 v0, v12
        0xBF81_0000, // s_endpgm
    ];

    /// [`WATERFALL_FETCHED_WAVE32`] with each lane's descriptor read by
    /// `buffer_load_dwordx4` (offen) through a buffer descriptor built
    /// around the table at s0, in place of the global loads.
    pub(crate) fn waterfall_buffer_wave32() -> Vec<u32> {
        let mut words = vec![
            0xBE98_0300, // s_mov_b32 s24, s0
            0xBE99_0380, // s_mov_b32 s25, 0
            0xBE9A_03C1, // s_mov_b32 s26, -1
            0xBE9B_03FF,
            0x3101_6FAC, // s_mov_b32 s27, 0x31016fac
            0xBE86_0301, // s_mov_b32 s6, s1
            0xBE87_0380, // s_mov_b32 s7, 0
            0x3402_0085, // v_lshlrev_b32 v1, 5, v0
            0xE038_1000,
            0x8006_0401, // buffer_load_dwordx4 v[4:7], v1, s[24:27], 0 offen
            0xE038_1010,
            0x8006_0801, // buffer_load_dwordx4 v[8:11], v1, s[24:27], 0 offen offset:16
        ];
        words.extend_from_slice(&WATERFALL_FETCHED_WAVE32[9..]);
        words
    }

    /// [`WATERFALL_FETCHED_WAVE32`] in wave64: 64-bit lane masks.
    pub(crate) const WATERFALL_FETCHED_WAVE64: &[u32] = &[
        0xBE84_0300, // s_mov_b32 s4, s0
        0xBE85_0380, // s_mov_b32 s5, 0
        0xBE86_0301, // s_mov_b32 s6, s1
        0xBE87_0380, // s_mov_b32 s7, 0
        0x3402_0085, // v_lshlrev_b32 v1, 5, v0
        0xDC38_8000,
        0x0404_0001, // global_load_dwordx4 v[4:7], v1, s[4:5]
        0xDC38_8010,
        0x0804_0001, // global_load_dwordx4 v[8:11], v1, s[4:5] offset:16
        0xF408_0203,
        0xFA00_0000, // s_load_dwordx4 s[8:11], s[6:7], 0x0
        0x7E04_0D00, // v_cvt_f32_u32 v2, v0
        0x7E06_0302, // v_mov_b32 v3, v2
        0xBF8C_0070, // s_waitcnt vmcnt(0) lgkmcnt(0)
        0xBE94_047E, // s_mov_b64 s[20:21], exec
        0x7E18_0504, // 0x3c: v_readfirstlane_b32 s12, v4
        0x7E1A_0505, // v_readfirstlane_b32 s13, v5
        0x7E1C_0506, // v_readfirstlane_b32 s14, v6
        0x7E1E_0507, // v_readfirstlane_b32 s15, v7
        0x7E20_0508, // v_readfirstlane_b32 s16, v8
        0x7E22_0509, // v_readfirstlane_b32 s17, v9
        0x7E24_050A, // v_readfirstlane_b32 s18, v10
        0x7E26_050B, // v_readfirstlane_b32 s19, v11
        0xD4E2_0016,
        0x0002_080C, // v_cmp_eq_u64_e64 s[22:23], s[12:13], v[4:5]
        0xD4E2_0018,
        0x0002_0C0E, // v_cmp_eq_u64_e64 s[24:25], s[14:15], v[6:7]
        0x8796_1816, // s_and_b64 s[22:23], s[22:23], s[24:25]
        0xD4E2_0018,
        0x0002_1010, // v_cmp_eq_u64_e64 s[24:25], s[16:17], v[8:9]
        0x8796_1816, // s_and_b64 s[22:23], s[22:23], s[24:25]
        0xD4E2_0018,
        0x0002_1412, // v_cmp_eq_u64_e64 s[24:25], s[18:19], v[10:11]
        0x8796_1816, // s_and_b64 s[22:23], s[22:23], s[24:25]
        0xBE96_2416, // s_and_saveexec_b64 s[22:23], s[22:23]
        0xBF80_0000, // s_nop 0
        0xF09C_0F08,
        0x0043_0C02, // image_sample_lz v[12:15], v[2:3], s[12:19], s[8:11] dmask:0xf dim:SQ_RSRC_IMG_2D
        0xBF8C_3F70, // s_waitcnt vmcnt(0)
        0x89FE_167E, // s_xor_b64 exec, exec, s[22:23]
        0xBF89_FFE6, // s_cbranch_execnz -> 0x3c
        0xBEFE_0414, // s_mov_b64 exec, s[20:21]
        0xD834_0000,
        0x0000_0C00, // ds_write_b32 v0, v12
        0xBF81_0000, // s_endpgm
    ];

    /// The same waterfall over one of two image descriptors loaded from the
    /// table at s0, picked per lane by `v_cndmask_b32` (lanes below 16 take
    /// the first).
    pub(crate) const WATERFALL_PICKED_WAVE32: &[u32] = &[
        0xBE84_0300, // s_mov_b32 s4, s0
        0xBE85_0380, // s_mov_b32 s5, 0
        0xBE86_0301, // s_mov_b32 s6, s1
        0xBE87_0380, // s_mov_b32 s7, 0
        0xF40C_0602,
        0xFA00_0000, // s_load_dwordx8 s[24:31], s[4:5], 0x0
        0xF40C_0802,
        0xFA00_0020, // s_load_dwordx8 s[32:39], s[4:5], 0x20
        0xF408_0203,
        0xFA00_0000, // s_load_dwordx4 s[8:11], s[6:7], 0x0
        0x7E04_0D00, // v_cvt_f32_u32 v2, v0
        0x7E06_0302, // v_mov_b32 v3, v2
        0x7D88_0090, // v_cmp_gt_u32 vcc_lo, 16, v0
        0xBF8C_C07F, // s_waitcnt lgkmcnt(0)
        0x7E08_0218, // v_mov_b32 v4, s24
        0x0208_0820, // v_cndmask_b32 v4, s32, v4, vcc_lo
        0x7E0A_0219, // v_mov_b32 v5, s25
        0x020A_0A21, // v_cndmask_b32 v5, s33, v5, vcc_lo
        0x7E0C_021A, // v_mov_b32 v6, s26
        0x020C_0C22, // v_cndmask_b32 v6, s34, v6, vcc_lo
        0x7E0E_021B, // v_mov_b32 v7, s27
        0x020E_0E23, // v_cndmask_b32 v7, s35, v7, vcc_lo
        0x7E10_021C, // v_mov_b32 v8, s28
        0x0210_1024, // v_cndmask_b32 v8, s36, v8, vcc_lo
        0x7E12_021D, // v_mov_b32 v9, s29
        0x0212_1225, // v_cndmask_b32 v9, s37, v9, vcc_lo
        0x7E14_021E, // v_mov_b32 v10, s30
        0x0214_1426, // v_cndmask_b32 v10, s38, v10, vcc_lo
        0x7E16_021F, // v_mov_b32 v11, s31
        0x0216_1627, // v_cndmask_b32 v11, s39, v11, vcc_lo
        0xBE94_037E, // s_mov_b32 s20, exec_lo
        0x7E18_0504, // 0x7c: v_readfirstlane_b32 s12, v4
        0x7E1A_0505, // v_readfirstlane_b32 s13, v5
        0x7E1C_0506, // v_readfirstlane_b32 s14, v6
        0x7E1E_0507, // v_readfirstlane_b32 s15, v7
        0x7E20_0508, // v_readfirstlane_b32 s16, v8
        0x7E22_0509, // v_readfirstlane_b32 s17, v9
        0x7E24_050A, // v_readfirstlane_b32 s18, v10
        0x7E26_050B, // v_readfirstlane_b32 s19, v11
        0xD4E2_0015,
        0x0002_080C, // v_cmp_eq_u64_e64 s21, s[12:13], v[4:5]
        0xD4E2_0016,
        0x0002_0C0E, // v_cmp_eq_u64_e64 s22, s[14:15], v[6:7]
        0x8715_1615, // s_and_b32 s21, s21, s22
        0xD4E2_0016,
        0x0002_1010, // v_cmp_eq_u64_e64 s22, s[16:17], v[8:9]
        0x8715_1615, // s_and_b32 s21, s21, s22
        0xD4E2_0016,
        0x0002_1412, // v_cmp_eq_u64_e64 s22, s[18:19], v[10:11]
        0x8715_1615, // s_and_b32 s21, s21, s22
        0xBE95_3C15, // s_and_saveexec_b32 s21, s21
        0xBF80_0000, // s_nop 0
        0xF09C_0F08,
        0x0043_0C02, // image_sample_lz v[12:15], v[2:3], s[12:19], s[8:11] dmask:0xf dim:SQ_RSRC_IMG_2D
        0xBF8C_3F70, // s_waitcnt vmcnt(0)
        0x897E_157E, // s_xor_b32 exec_lo, exec_lo, s21
        0xBF89_FFE6, // s_cbranch_execnz -> 0x7c
        0xBEFE_0314, // s_mov_b32 exec_lo, s20
        0xD834_0000,
        0x0000_0C00, // ds_write_b32 v0, v12
        0xBF81_0000, // s_endpgm
    ];

    /// [`WATERFALL_FETCHED_WAVE32`] inside an outer loop that moves the
    /// descriptor to v[20:27] and back after each pass: the loop header's
    /// phis carry the loaded dwords unchanged.
    pub(crate) const WATERFALL_CARRIED_WAVE32: &[u32] = &[
        0xBE84_0300, // s_mov_b32 s4, s0
        0xBE85_0380, // s_mov_b32 s5, 0
        0xBE86_0301, // s_mov_b32 s6, s1
        0xBE87_0380, // s_mov_b32 s7, 0
        0x3402_0085, // v_lshlrev_b32 v1, 5, v0
        0xF408_0203,
        0xFA00_0000, // s_load_dwordx4 s[8:11], s[6:7], 0x0
        0x7E04_0D00, // v_cvt_f32_u32 v2, v0
        0x7E06_0302, // v_mov_b32 v3, v2
        0xDC38_8000,
        0x0404_0001, // global_load_dwordx4 v[4:7], v1, s[4:5]
        0xDC38_8010,
        0x0804_0001, // global_load_dwordx4 v[8:11], v1, s[4:5] offset:16
        0xBE97_0380, // s_mov_b32 s23, 0
        0xBF8C_0070, // s_waitcnt vmcnt(0) lgkmcnt(0)
        0xBE94_037E, // 0x3c: s_mov_b32 s20, exec_lo
        0x7E18_0504, // 0x40: v_readfirstlane_b32 s12, v4
        0x7E1A_0505, // v_readfirstlane_b32 s13, v5
        0x7E1C_0506, // v_readfirstlane_b32 s14, v6
        0x7E1E_0507, // v_readfirstlane_b32 s15, v7
        0x7E20_0508, // v_readfirstlane_b32 s16, v8
        0x7E22_0509, // v_readfirstlane_b32 s17, v9
        0x7E24_050A, // v_readfirstlane_b32 s18, v10
        0x7E26_050B, // v_readfirstlane_b32 s19, v11
        0xD4E2_0015,
        0x0002_080C, // v_cmp_eq_u64_e64 s21, s[12:13], v[4:5]
        0xD4E2_0016,
        0x0002_0C0E, // v_cmp_eq_u64_e64 s22, s[14:15], v[6:7]
        0x8715_1615, // s_and_b32 s21, s21, s22
        0xD4E2_0016,
        0x0002_1010, // v_cmp_eq_u64_e64 s22, s[16:17], v[8:9]
        0x8715_1615, // s_and_b32 s21, s21, s22
        0xD4E2_0016,
        0x0002_1412, // v_cmp_eq_u64_e64 s22, s[18:19], v[10:11]
        0x8715_1615, // s_and_b32 s21, s21, s22
        0xBE95_3C15, // s_and_saveexec_b32 s21, s21
        0xF09C_0F08,
        0x0043_0C02, // image_sample_lz v[12:15], v[2:3], s[12:19], s[8:11] dmask:0xf dim:SQ_RSRC_IMG_2D
        0xBF8C_3F70, // s_waitcnt vmcnt(0)
        0x897E_157E, // s_xor_b32 exec_lo, exec_lo, s21
        0xBF89_FFE7, // s_cbranch_execnz -> 0x40
        0xBEFE_0314, // s_mov_b32 exec_lo, s20
        0xD834_0000,
        0x0000_0C00, // ds_write_b32 v0, v12
        0x7E28_0304, // v_mov_b32 v20, v4
        0x7E2A_0305, // v_mov_b32 v21, v5
        0x7E2C_0306, // v_mov_b32 v22, v6
        0x7E2E_0307, // v_mov_b32 v23, v7
        0x7E30_0308, // v_mov_b32 v24, v8
        0x7E32_0309, // v_mov_b32 v25, v9
        0x7E34_030A, // v_mov_b32 v26, v10
        0x7E36_030B, // v_mov_b32 v27, v11
        0x7E08_0314, // v_mov_b32 v4, v20
        0x7E0A_0315, // v_mov_b32 v5, v21
        0x7E0C_0316, // v_mov_b32 v6, v22
        0x7E0E_0317, // v_mov_b32 v7, v23
        0x7E10_0318, // v_mov_b32 v8, v24
        0x7E12_0319, // v_mov_b32 v9, v25
        0x7E14_031A, // v_mov_b32 v10, v26
        0x7E16_031B, // v_mov_b32 v11, v27
        0x8017_8117, // s_add_u32 s23, s23, 1
        0xBF07_8417, // s_cmp_lg_u32 s23, 4
        0xBF85_FFD0, // s_cbranch_scc1 -> 0x3c
        0xBF81_0000, // s_endpgm
    ];

    /// The waterfall over a descriptor loaded in an if/else on EXEC: lanes
    /// below 16 load entry i of the table at s0, the others entry i at byte
    /// 1024; phis join the two (and the undefined VGPRs of lanes that skip
    /// the first branch).
    pub(crate) const WATERFALL_BRANCHES_WAVE32: &[u32] = &[
        0xBE84_0300, // s_mov_b32 s4, s0
        0xBE85_0380, // s_mov_b32 s5, 0
        0xBE86_0301, // s_mov_b32 s6, s1
        0xBE87_0380, // s_mov_b32 s7, 0
        0x3402_0085, // v_lshlrev_b32 v1, 5, v0
        0xF408_0203,
        0xFA00_0000, // s_load_dwordx4 s[8:11], s[6:7], 0x0
        0x7E04_0D00, // v_cvt_f32_u32 v2, v0
        0x7E06_0302, // v_mov_b32 v3, v2
        0x7D88_0090, // v_cmp_gt_u32 vcc_lo, 16, v0
        0xBE98_3C6A, // s_and_saveexec_b32 s24, vcc_lo
        0x8918_187E, // s_xor_b32 s24, exec_lo, s24
        0xBF88_0004, // s_cbranch_execz -> 0x44
        0xDC38_8000,
        0x0404_0001, // global_load_dwordx4 v[4:7], v1, s[4:5]
        0xDC38_8010,
        0x0804_0001, // global_load_dwordx4 v[8:11], v1, s[4:5] offset:16
        0xBE98_3D18, // 0x44: s_or_saveexec_b32 s24, s24
        0x897E_187E, // s_xor_b32 exec_lo, exec_lo, s24
        0xBF88_0004, // s_cbranch_execz -> 0x60
        0xDC38_8400,
        0x0404_0001, // global_load_dwordx4 v[4:7], v1, s[4:5] offset:1024
        0xDC38_8410,
        0x0804_0001, // global_load_dwordx4 v[8:11], v1, s[4:5] offset:1040
        0x887E_187E, // 0x60: s_or_b32 exec_lo, exec_lo, s24
        0xBF8C_0070, // s_waitcnt vmcnt(0) lgkmcnt(0)
        0xBE94_037E, // s_mov_b32 s20, exec_lo
        0x7E18_0504, // 0x6c: v_readfirstlane_b32 s12, v4
        0x7E1A_0505, // v_readfirstlane_b32 s13, v5
        0x7E1C_0506, // v_readfirstlane_b32 s14, v6
        0x7E1E_0507, // v_readfirstlane_b32 s15, v7
        0x7E20_0508, // v_readfirstlane_b32 s16, v8
        0x7E22_0509, // v_readfirstlane_b32 s17, v9
        0x7E24_050A, // v_readfirstlane_b32 s18, v10
        0x7E26_050B, // v_readfirstlane_b32 s19, v11
        0xD4E2_0015,
        0x0002_080C, // v_cmp_eq_u64_e64 s21, s[12:13], v[4:5]
        0xD4E2_0016,
        0x0002_0C0E, // v_cmp_eq_u64_e64 s22, s[14:15], v[6:7]
        0x8715_1615, // s_and_b32 s21, s21, s22
        0xD4E2_0016,
        0x0002_1010, // v_cmp_eq_u64_e64 s22, s[16:17], v[8:9]
        0x8715_1615, // s_and_b32 s21, s21, s22
        0xD4E2_0016,
        0x0002_1412, // v_cmp_eq_u64_e64 s22, s[18:19], v[10:11]
        0x8715_1615, // s_and_b32 s21, s21, s22
        0xBE95_3C15, // s_and_saveexec_b32 s21, s21
        0xF09C_0F08,
        0x0043_0C02, // image_sample_lz v[12:15], v[2:3], s[12:19], s[8:11] dmask:0xf dim:SQ_RSRC_IMG_2D
        0xBF8C_3F70, // s_waitcnt vmcnt(0)
        0x897E_157E, // s_xor_b32 exec_lo, exec_lo, s21
        0xBF89_FFE7, // s_cbranch_execnz -> 0x6c
        0xBEFE_0314, // s_mov_b32 exec_lo, s20
        0xD834_0000,
        0x0000_0C00, // ds_write_b32 v0, v12
        0xBF81_0000, // s_endpgm
    ];

    /// The waterfall inside an outer loop whose pass n reloads the
    /// descriptor of lanes below n from byte 1024 on: the loop header joins
    /// the first load with what the previous pass left.
    pub(crate) const WATERFALL_REPLACED_WAVE32: &[u32] = &[
        0xBE84_0300, // s_mov_b32 s4, s0
        0xBE85_0380, // s_mov_b32 s5, 0
        0xBE86_0301, // s_mov_b32 s6, s1
        0xBE87_0380, // s_mov_b32 s7, 0
        0x3402_0085, // v_lshlrev_b32 v1, 5, v0
        0xF408_0203,
        0xFA00_0000, // s_load_dwordx4 s[8:11], s[6:7], 0x0
        0x7E04_0D00, // v_cvt_f32_u32 v2, v0
        0x7E06_0302, // v_mov_b32 v3, v2
        0xDC38_8000,
        0x0404_0001, // global_load_dwordx4 v[4:7], v1, s[4:5]
        0xDC38_8010,
        0x0804_0001, // global_load_dwordx4 v[8:11], v1, s[4:5] offset:16
        0xBE97_0380, // s_mov_b32 s23, 0
        0xBF8C_0070, // 0x38: s_waitcnt vmcnt(0) lgkmcnt(0)
        0xBE94_037E, // s_mov_b32 s20, exec_lo
        0x7E18_0504, // 0x40: v_readfirstlane_b32 s12, v4
        0x7E1A_0505, // v_readfirstlane_b32 s13, v5
        0x7E1C_0506, // v_readfirstlane_b32 s14, v6
        0x7E1E_0507, // v_readfirstlane_b32 s15, v7
        0x7E20_0508, // v_readfirstlane_b32 s16, v8
        0x7E22_0509, // v_readfirstlane_b32 s17, v9
        0x7E24_050A, // v_readfirstlane_b32 s18, v10
        0x7E26_050B, // v_readfirstlane_b32 s19, v11
        0xD4E2_0015,
        0x0002_080C, // v_cmp_eq_u64_e64 s21, s[12:13], v[4:5]
        0xD4E2_0016,
        0x0002_0C0E, // v_cmp_eq_u64_e64 s22, s[14:15], v[6:7]
        0x8715_1615, // s_and_b32 s21, s21, s22
        0xD4E2_0016,
        0x0002_1010, // v_cmp_eq_u64_e64 s22, s[16:17], v[8:9]
        0x8715_1615, // s_and_b32 s21, s21, s22
        0xD4E2_0016,
        0x0002_1412, // v_cmp_eq_u64_e64 s22, s[18:19], v[10:11]
        0x8715_1615, // s_and_b32 s21, s21, s22
        0xBE95_3C15, // s_and_saveexec_b32 s21, s21
        0xF09C_0F08,
        0x0043_0C02, // image_sample_lz v[12:15], v[2:3], s[12:19], s[8:11] dmask:0xf dim:SQ_RSRC_IMG_2D
        0xBF8C_3F70, // s_waitcnt vmcnt(0)
        0x897E_157E, // s_xor_b32 exec_lo, exec_lo, s21
        0xBF89_FFE7, // s_cbranch_execnz -> 0x40
        0xBEFE_0314, // s_mov_b32 exec_lo, s20
        0xD834_0000,
        0x0000_0C00, // ds_write_b32 v0, v12
        0x7D88_0017, // v_cmp_gt_u32 vcc_lo, s23, v0
        0xBE98_3C6A, // s_and_saveexec_b32 s24, vcc_lo
        0xBF88_0004, // s_cbranch_execz -> 0xcc
        0xDC38_8400,
        0x0404_0001, // global_load_dwordx4 v[4:7], v1, s[4:5] offset:1024
        0xDC38_8410,
        0x0804_0001, // global_load_dwordx4 v[8:11], v1, s[4:5] offset:1040
        0x887E_187E, // 0xcc: s_or_b32 exec_lo, exec_lo, s24
        0x8017_8117, // s_add_u32 s23, s23, 1
        0xBF07_8417, // s_cmp_lg_u32 s23, 4
        0xBF85_FFD7, // s_cbranch_scc1 -> 0x38
        0xBF81_0000, // s_endpgm
    ];

    fn waterfall(words: &[u32], wave64: bool) -> Result<Resources, ShaderError> {
        let code: Vec<u8> = words.iter().flat_map(|w| w.to_le_bytes()).collect();
        let p = crate::ir::build(&code, wave64, 256).unwrap();
        track(&p, &[param(Table, 0, 1), param(Table, 1, 1)], HwStage::Cs)
    }

    #[test]
    fn descriptors_read_from_the_first_lane_of_per_lane_loads_are_traced_to_them() {
        for (words, wave64) in [
            (WATERFALL_FETCHED_WAVE32, false),
            (WATERFALL_FETCHED_WAVE64, true),
        ] {
            let r = waterfall(words, wave64).unwrap();
            let image = r
                .descriptors
                .iter()
                .find(|d| d.role == Role::Resource)
                .unwrap();
            let Source::Fetched { load, first: 0 } = image.source else {
                panic!("{:?}", image.source)
            };
            // The first of the two loads: entry v0 * 32 of the table at s0.
            let a = r.scalar.iter().find(|s| s.0 == load).unwrap().1;
            assert_eq!((a.param, a.offset), (0, 0));
            assert!(a.dynamic.is_some());
            // The sampler is the same for every lane.
            let sampler = r.descriptors.iter().find(|d| d.role == Role::Sampler);
            assert!(matches!(
                sampler.unwrap().source,
                Source::Loaded { first: 0, .. }
            ));
            // Four 64-bit compares of the first lane's dwords with the lane's
            // own; the eight reads become the lane's own dwords.
            assert_eq!(r.waterfall.compares.len(), 4);
            assert_eq!(r.waterfall.reads.len(), 8);
        }
    }

    #[test]
    fn descriptors_read_from_the_first_lane_of_a_per_lane_choice_are_traced_per_lane() {
        let r = waterfall(WATERFALL_PICKED_WAVE32, false).unwrap();
        let image = &r.descriptors[0];
        let Source::Lanes { args, .. } = &image.source else {
            panic!("{:?}", image.source)
        };
        // Lanes with their bit set take the first load (offset 0), the
        // others the second (0x20).
        let offsets: Vec<u32> = args
            .iter()
            .map(|a| match *a {
                Source::Loaded { load, first: 0 } => {
                    r.scalar.iter().find(|s| s.0 == load).unwrap().1.offset
                }
                ref a => panic!("{a:?}"),
            })
            .collect();
        assert_eq!(offsets, [0, 0x20]);
        assert_eq!(r.waterfall.compares.len(), 4);
    }

    #[test]
    fn descriptors_read_from_the_first_lane_of_buffer_loads_over_a_table_are_fetched() {
        let r = waterfall(&waterfall_buffer_wave32(), false).unwrap();
        let image = &r
            .descriptors
            .iter()
            .find(|d| d.role == Role::Resource && matches!(d.source, Source::Fetched { .. }))
            .unwrap();
        let Source::Fetched { load, first: 0 } = image.source else {
            panic!("{:?}", image.source)
        };
        let a = r.scalar.iter().find(|s| s.0 == load).unwrap().1;
        assert_eq!((a.param, a.offset, a.includes_base), (0, 0, false));
        assert!(a.dynamic.is_some());
        assert_eq!(r.waterfall.compares.len(), 4);
    }

    #[test]
    fn descriptors_read_from_the_first_lane_of_lds_fail_naming_it() {
        // The waterfall over descriptors each lane read from LDS:
        // ds_read_b128 v[4:7], v1 and ds_read_b128 v[8:11], v1 offset:16
        // in place of the global loads.
        let mut words = WATERFALL_FETCHED_WAVE32.to_vec();
        words[5..9].copy_from_slice(&[0xDBFC_0000, 0x0400_0001, 0xDBFC_0010, 0x0800_0001]);
        let err = waterfall(&words, false).unwrap_err();
        assert!(
            matches!(
                err,
                ShaderError::Unsupported {
                    what: "a descriptor read from the first lane of an LDS read",
                    ..
                }
            ),
            "{err:?}"
        );
    }

    /// The byte offset in the table of each per-lane load `s` picks from,
    /// per path (`C` for the join itself on the previous pass, `U` for
    /// unset VGPRs).
    fn paths(r: &Resources, s: &Source) -> String {
        match s {
            Source::Fetched { load, first: 0 } => {
                let a = r.scalar.iter().find(|x| x.0 == *load).unwrap().1;
                assert!(a.dynamic.is_some());
                format!("{}", a.offset)
            }
            Source::Paths { args, .. } => {
                let args: Vec<String> = args.iter().map(|a| paths(r, a)).collect();
                format!("[{}]", args.join(" "))
            }
            Source::Carried { .. } => "C".into(),
            Source::Unset => "U".into(),
            s => panic!("{s:?}"),
        }
    }

    #[test]
    fn descriptors_read_from_the_first_lane_of_a_loop_carried_phi_are_its_one_source() {
        let r = waterfall(WATERFALL_CARRIED_WAVE32, false).unwrap();
        let image = &r.descriptors[0];
        assert_eq!(paths(&r, &image.source), "0");
        assert_eq!(r.waterfall.compares.len(), 4);
        assert!(r.waterfall.unset.is_empty());
    }

    #[test]
    fn descriptors_read_from_the_first_lane_of_a_phi_are_traced_per_path() {
        let r = waterfall(WATERFALL_BRANCHES_WAVE32, false).unwrap();
        let image = &r.descriptors[0];
        // Past the else branch or through it. Before it, lanes that skipped
        // the then branch hold v[4:11] as no one set them: undefined, so
        // the then branch's load.
        assert_eq!(paths(&r, &image.source), "[0 1024]");
        assert_eq!(r.waterfall.unset.len(), 8);
        assert_eq!(r.waterfall.compares.len(), 4);
    }

    #[test]
    fn descriptors_replaced_in_a_loop_join_the_previous_pass() {
        let r = waterfall(WATERFALL_REPLACED_WAVE32, false).unwrap();
        let image = &r.descriptors[0];
        // From the entry, or from the previous pass: kept or reloaded.
        assert_eq!(paths(&r, &image.source), "[0 [C 1024]]");
        assert_eq!(r.waterfall.compares.len(), 4);
    }

    #[test]
    fn descriptors_joined_from_lds_fail_naming_it() {
        // The else branch reads its descriptor from LDS: ds_read_b128
        // v[4:7], v1 and ds_read_b128 v[8:11], v1 offset:16.
        let mut words = WATERFALL_BRANCHES_WAVE32.to_vec();
        assert_eq!(words[20], 0xDC38_8400);
        words[20..24].copy_from_slice(&[0xDBFC_0000, 0x0400_0001, 0xDBFC_0010, 0x0800_0001]);
        let err = waterfall(&words, false).unwrap_err();
        assert!(
            matches!(
                err,
                ShaderError::Unsupported {
                    what: "a descriptor read from the first lane of an LDS read",
                    ..
                }
            ),
            "{err:?}"
        );
    }
}
