//! Some Scarlett compute shaders load root descriptors and inline constants from one
//! argument block pointed to by s[0:1]. Track the origin of the descriptor words so
//! MUBUF operations can use desktop root SRVs/UAVs without exposing a GPU address.
mod copy_loop;
mod fspx;
pub(in crate::hlsl) use fspx::convolution_layout;
mod ps09_depth;
pub(super) mod ps09_raster;
use super::*;
use crate::meta::RootParamKind;

#[derive(Clone, Copy, PartialEq, Eq, Debug, PartialOrd, Ord)]
struct Binding {
    slot: u32,
    uav: bool,
}

impl Binding {
    fn register(self) -> String {
        format!("{}{}", if self.uav { "u" } else { "t" }, self.slot)
    }
}

#[derive(Clone, Copy, PartialEq, Eq)]
enum Word {
    Data,
    Immediate(u32),
    PointerLo,
    PointerHi,
    AddressLo(Binding),
    AddressHi(Binding, u32), // descriptor stride in bytes
    PredicateLo,
    PredicateHi,
    SavedExecLo,
    SavedExecHi,
    BallotBits,
    Unavailable,
}

impl Word {
    fn opaque(self) -> bool {
        !matches!(self, Self::Data | Self::Immediate(_))
    }
}

#[derive(Clone, Copy)]
enum Argument {
    Descriptor(Word),
    Constant(u32, u32), // shader register and dword offset
}

#[derive(Default, PartialEq, Eq)]
pub(super) enum RootAbi {
    #[default]
    ArgumentBlock,
    RegisterWave32,
    RegisterWave64,
    Ps09Raster,
}

#[derive(Default)]
pub(super) struct PackedRoot {
    pub(super) abi: RootAbi,
    loads: BTreeMap<usize, Vec<Option<(u32, u32)>>>,
    raw_scalar: BTreeMap<usize, (Binding, u32, u8)>,
    raw_offsets: BTreeMap<usize, u8>,
    coherent_loads: BTreeSet<usize>,
    pub(super) bounded_backedges: BTreeSet<usize>,
    pub(super) register_wave_size: Option<u32>,
    pub(super) ballot_nonempty: BTreeMap<usize, u8>,
    pub(super) wide_images: BTreeSet<usize>,
    images: BTreeMap<usize, (Binding, u32, Option<u32>)>,
    buffers: BTreeMap<usize, (Binding, u32)>,
    entry_constants: Vec<(u8, u32, u32)>, // SGPR, cbuffer, dword
    uint_srvs: BTreeSet<u32>,
    // Publish/cohere UAV data around an inter-workgroup ticket.
    pub(super) device_sync_before: BTreeSet<usize>,
    pub(super) device_sync_barriers: BTreeSet<usize>,
    pub(super) copy_backedges: BTreeSet<usize>,
    pub(super) scan_width: Option<u32>,
    pub(super) lane_masks: BTreeSet<usize>,
    pub(super) ballots: BTreeSet<usize>,
}

pub(super) fn matches_program(isa: &[u8], size: usize, fingerprint: u64) -> bool {
    isa.len() == size && isa.iter().fold(0xcbf29ce484222325u64, |hash, &b| {
        (hash ^ u64::from(b)).wrapping_mul(0x100000001b3)
    }) == fingerprint
}

type State = [Word; 106];

fn value(state: &State, op: Opnd) -> Word {
    match op {
        Opnd::Sgpr(n) => state.get(n as usize).copied().unwrap_or(Word::Unavailable),
        Opnd::ConstInt(i) => Word::Immediate(i as u32),
        Opnd::Literal(w) => Word::Immediate(w),
        _ => Word::Data,
    }
}

fn merge(state: &mut State, other: &State) {
    for (a, &b) in state.iter_mut().zip(other) {
        if *a != b {
            *a = if a.opaque() || b.opaque() {
                Word::Unavailable
            } else {
                Word::Data
            };
        }
    }
}

// Recognize paired consumers before either register can be repurposed. Only
// independent scheduled instructions from the observed ballot idiom may intervene.
fn paired_ballot_use(compare: &Insn, tail: &[Insn]) -> bool {
    let [Opnd::Sgpr(lo)] = compare.dst[..] else {
        return false;
    };
    if lo % 2 != 0 || lo >= 105 || !matches!(compare.src[..], [Opnd::ConstInt(0), Opnd::Vgpr(_), Opnd::Sgpr(0)]) {
        return false;
    }
    let pair = [Opnd::Sgpr(lo), Opnd::Sgpr(lo + 1)];
    let mut low_use = None;
    for i in tail {
        if i.dst.iter().any(|o| pair.contains(o)) {
            break;
        }
        match i.mnemonic.as_str() {
            "v_cmp_ne_i32"
                if matches!(i.dst[..], [Opnd::Sgpr(d)] if d % 2 == 0 && d != lo)
                    && matches!(i.src[..], [Opnd::ConstInt(0), Opnd::Vgpr(_), Opnd::Sgpr(0)]) =>
            {
                continue
            }
            "v_lshlrev_b32" | "v_and_b32" | "v_or_b32" | "v_xor_b32" | "s_not_b32" | "buffer_load_dword" => {}
            _ => break,
        }
        if !i.src.iter().any(|o| pair.contains(o)) {
            continue;
        }
        if !(i.mnemonic == "s_not_b32" || i.mnemonic == "v_xor_b32") {
            break;
        }
        if i.src.first() == Some(&pair[0]) && low_use.is_none() {
            low_use = Some((i.mnemonic.as_str(), &i.src[1..]));
        } else {
            return i.src.first() == Some(&pair[1]) && low_use == Some((i.mnemonic.as_str(), &i.src[1..]));
        }
    }
    false
}

impl PackedRoot {
    pub(super) fn analyze(
        ex: &ShaderExtract,
        insns: &[Insn],
        scan: Option<&wave_scan::Scan>,
    ) -> Result<Option<Self>, String> {
        if ex.fspx_config.is_some() { return Self::fspx(ex, insns).map(Some); }
        let Some(root) = &ex.root_sig else { return Ok(None) };
        if ex.stage == Stage::Pixel {
            return ps09_depth::bindings(ex, root, insns);
        }
        if ex.stage == Stage::Compute {
            if let Some(plan) = ps09_raster::bindings(ex, root, insns)? { return Ok(Some(plan)); }
        }
        // This executable layout is corroborated by PSV thread dimensions and the
        // compiler prologue which keeps the low eight bits of the argument pointer’s high word.
        let mut prologue = insns
            .iter()
            .filter(|i| !matches!(i.mnemonic.as_str(), "v_nop" | "s_nop" | "s_inst_prefetch"));
        let Some(first) = prologue.next() else { return Ok(None) };
        let masks_high = |i: &Insn| i.mnemonic == "s_and_b32" && i.src == [Opnd::Sgpr(1), Opnd::Literal(255)];
        let copied_pair = matches!(first.dst.as_slice(), [Opnd::Sgpr(d)] if d % 2 == 0 && *d < 105
            && first.mnemonic == "s_mov_b32" && first.src == [Opnd::Sgpr(0)]
            && prologue.next().is_some_and(|i| masks_high(i) && i.dst == [Opnd::Sgpr(d + 1)]));
        if ex.stage != Stage::Compute
            || root.flags != 0x0300_0000
            || ex.psv_threads.is_none()
            || !(masks_high(first) || copied_pair)
        {
            return Ok(None);
        }
        let mut args = Vec::new();
        for p in &root.params {
            match p.kind {
                RootParamKind::Descriptor {
                    kind: k @ (3 | 4),
                    register,
                    space: 0,
                } => {
                    let b = Binding {
                        slot: register,
                        uav: k == 4,
                    };
                    args.extend([
                        Argument::Descriptor(Word::AddressLo(b)),
                        Argument::Descriptor(Word::AddressHi(b, 0)),
                    ]);
                }
                RootParamKind::Constants {
                    register,
                    space: 0,
                    num32,
                } if (1..=64).contains(&num32) => {
                    args.extend((0..num32).map(|k| Argument::Constant(register, k)));
                }
                _ => return Err("unsupported packed compute root parameter".into()),
            }
            if args.len() > 64 {
                return Err("packed root arguments exceed 64 dwords".into());
            }
        }
        let scan_width = scan.map(|s| s.width);
        let mut plan = Self {
            scan_width,
            ..Self::default()
        };
        let mut state = [Word::Unavailable; 106];
        state[4] = Word::Data; // Group ID X in the observed one-dimensional dispatch layout.
        state[0] = Word::PointerLo;
        state[1] = Word::PointerHi;
        let mut joins = BTreeMap::<usize, State>::new();
        let mut falls = true;
        let mut branched = false;
        // The wave64 compiler uses local X modulo 64 as the lane index, saves
        // both EXEC words, and consumes both words of an explicit compare as data.
        let saves_exec_pair = insns
            .iter()
            .any(|i| i.mnemonic == "s_and_saveexec_b64" && i.src == [Opnd::VccLo]);
        let wave64 = matches!(ex.psv_threads, Some([x, 1, 1]) if x > 0 && x % 64 == 0)
            && insns
                .iter()
                .any(|i| i.mnemonic == "v_and_b32" && i.src == [Opnd::ConstInt(63), Opnd::Vgpr(0)]);
        let mut resume = 0;
        for (idx, ins) in insns.iter().enumerate() {
            if idx < resume { continue; }
            if scan.is_some_and(|s| s.subtotal.contains(&idx)) {
                // This validated region only reads local scalars and VGPRs.
                // The first wave skips the loop, so its surviving scalar words
                // cannot serve as root descriptors on all paths after the join.
                if let Some(Opnd::Sgpr(n)) = ins.dst.first() {
                    state[*n as usize] = Word::Unavailable;
                    if ins.mnemonic.ends_with("_b64") {
                        state[*n as usize + 1] = Word::Unavailable;
                    }
                }
                continue;
            }
            if let Some(incoming) = joins.remove(&ins.offset) {
                if falls {
                    merge(&mut state, &incoming);
                } else {
                    state = incoming;
                }
                falls = true;
            }
            if !falls {
                if ins.mnemonic == "s_nop" { continue; }
                // The common HLSL emitter visits unreachable blocks too.
                return Err("unreachable block in packed compute shader".into());
            }
            if !plan.copy_backedges.is_empty() && ins.mnemonic == "s_cmp_eq_u32" {
                if let Some(end) = plan.copy_tail_mask(idx, insns, &args, &mut state, &joins)? {
                    resume = end + 1;
                    continue;
                }
            }
            if ins.mnemonic == "s_branch" && scan.is_none() {
                if let Some(end) = plan.copy_loop(idx, insns, &args, &mut state, &joins)? {
                    resume = end + 1;
                    branched = true;
                    continue;
                }
            }
            if ins.enc == Enc::Sopp && Emitter::branch_cond(ins.op).is_some() {
                branched = true;
                let target = ins.offset as i64 + 4 + 4 * ins.aux.imm;
                if target <= ins.offset as i64 {
                    return Err("backward branch in packed compute shader".into());
                }
                joins
                    .entry(target as usize)
                    .and_modify(|prior| merge(prior, &state))
                    .or_insert(state);
                falls = ins.op != 2;
            } else if ins.mnemonic == "s_endpgm" {
                falls = false;
            }
            if scan_width == Some(64)
                && ins.mnemonic == "v_cmp_ne_i32"
                && matches!(ins.dst[..], [Opnd::Sgpr(n)] if n < 105 && n % 2 == 0)
                && matches!(ins.src[..], [Opnd::ConstInt(0), Opnd::Vgpr(_), Opnd::Sgpr(0)])
            {
                // Convergence and the guest wave width were checked by wave_scan.
                plan.ballots.insert(idx);
            } else if scan_width.is_none()
                && saves_exec_pair
                && ins.mnemonic.starts_with("v_cmp_")
                && matches!(ins.dst.first(), Some(Opnd::Sgpr(_)))
            {
                if !wave64 || branched || ins.mnemonic != "v_cmp_ne_i32" || !paired_ballot_use(ins, &insns[idx + 1..]) {
                    return Err(format!("unresolved wave ballot at {:#x}", ins.offset));
                }
                plan.ballots.insert(idx);
            }
            if ins.mnemonic == "s_barrier" && branched && scan_width.is_none() {
                // Forward branches that skip this barrier may only retire at the
                // final ENDPGM. emit_body reconverges those lanes at the host barrier.
                let last = insns.last().unwrap();
                if joins.is_empty() || !joins.keys().all(|&off| {
                    (off == last.offset && last.mnemonic == "s_endpgm") || off == last.offset + last.dwords * 4
                }) {
                    return Err("packed group barrier after a branch".into());
                }
            }
            if scan_width.is_some() && matches!(ins.mnemonic.as_str(), "s_cselect_b32" | "s_cselect_b64") {
                // An ordinary scalar compare established SCC. Vector addition leaves it intact.
                let cmp = insns[..idx]
                    .iter()
                    .rev()
                    .find(|i| !matches!(i.mnemonic.as_str(), "v_add_nc_u32" | "v_sub_nc_u32"));
                if !cmp.is_some_and(|i| i.mnemonic == "s_cmp_eq_u32") || branched {
                    return Err("unresolved SCC in packed wave scan".into());
                }
            }
            plan.step(idx, ins, &args, &mut state)
                .map_err(|e| format!("packed roots at {:#x} ({}): {e}", ins.offset, ins.mnemonic))?;
        }
        if !joins.is_empty() {
            return Err("branch target outside packed shader".into());
        }
        Ok(Some(plan))
    }

    // Track the representation of each SGPR mask. Scalar integer operations
    // consume physical ballot words, while EXEC uses a per-invocation predicate.
    fn scan_operand(
        &mut self,
        idx: usize,
        ins: &Insn,
        width: u32,
        dst: Option<usize>,
        state: &mut State,
    ) -> Result<bool, &'static str> {
        let m = ins.mnemonic.as_str();
        let s = &ins.src;
        if m == "v_cndmask_b32" && ins.enc == Enc::Vop3 {
            let Some(Opnd::Sgpr(n)) = s.get(2) else {
                return Err("missing scan lane mask");
            };
            if !matches!(value(state, s[2]), Word::Immediate(_) | Word::BallotBits)
                || (width == 64
                    && (*n % 2 != 0
                        || !matches!(state.get(*n as usize + 1), Some(Word::Immediate(_) | Word::BallotBits))))
            {
                return Err("unresolved physical scan lane mask");
            }
            self.lane_masks.insert(idx);
        }
        if m == "s_movk_i32" {
            state[dst.ok_or("invalid immediate destination")?] = Word::Immediate(ins.aux.imm as i16 as i32 as u32);
            return Ok(true);
        }
        if width == 64 && m == "s_bcnt1_i32_b64" {
            if !matches!(s[..], [Opnd::Sgpr(n)] if n % 2 == 0 && state.get(n as usize..n as usize+2) == Some(&[Word::BallotBits;2]))
            {
                return Err("unresolved scan ballot count");
            }
            state[dst.ok_or("invalid count destination")?] = Word::Data;
            return Ok(true);
        }
        if width == 64 && m == "s_mov_b64" && s == &[Opnd::ExecLo] {
            let d = dst
                .filter(|d| d % 2 == 0 && *d < 105)
                .ok_or("invalid EXEC destination")?;
            state[d..d + 2].copy_from_slice(&[Word::SavedExecLo, Word::SavedExecHi]);
            return Ok(true);
        }
        if width == 64 && m == "v_cmp_ne_u32" && matches!(s[..], [Opnd::Vgpr(_), Opnd::Vgpr(_), Opnd::Sgpr(0)]) {
            if let Some(d) = dst.filter(|d| d % 2 == 0 && *d < 105) {
                state[d..d + 2].copy_from_slice(&[Word::PredicateLo, Word::PredicateHi]);
                return Ok(true);
            }
        }
        let wide = width == 64;
        let select = if wide { "s_cselect_b64" } else { "s_cselect_b32" };
        let and = if wide { "s_and_b64" } else { "s_and_b32" };
        let save = if wide {
            "s_and_saveexec_b64"
        } else {
            "s_and_saveexec_b32"
        };
        if m == select
            || ((m == and || (wide && m == "s_or_b64")) && s.first() == Some(&Opnd::VccLo))
            || (m == save && s != &[Opnd::VccLo])
        {
            let d = dst
                .filter(|d| !wide || (d % 2 == 0 && *d < 105))
                .ok_or("invalid scan predicate register")?;
            if m == select {
                if s != &[Opnd::ExecLo, Opnd::ConstInt(0)] {
                    return Err("unsupported scan predicate selection");
                }
            } else {
                let Some(Opnd::Sgpr(n)) = s.last() else {
                    return Err("missing scan predicate");
                };
                if state.get(*n as usize) != Some(&Word::PredicateLo)
                    || (wide && (*n % 2 != 0 || state.get(*n as usize + 1) != Some(&Word::PredicateHi)))
                {
                    return Err("unknown scan predicate");
                }
            }
            state[d] = if m == save {
                Word::SavedExecLo
            } else {
                Word::PredicateLo
            };
            if wide {
                state[d + 1] = if m == save {
                    Word::SavedExecHi
                } else {
                    Word::PredicateHi
                };
            }
            return Ok(true);
        }
        if dst.is_some() && m.starts_with("v_cmp_") && !self.ballots.contains(&idx) {
            return Err("unsupported scan comparison mask");
        }
        Ok(false)
    }

    fn step(&mut self, idx: usize, ins: &Insn, args: &[Argument], state: &mut State) -> Result<(), &'static str> {
        let m = ins.mnemonic.as_str();
        if m == "v_nop" {
            return Ok(());
        }
        let s = &ins.src;
        let dst = match ins.dst.first() {
            Some(Opnd::Sgpr(n)) => Some(*n as usize),
            _ => None,
        };
        match ins.enc {
            Enc::Smem => {
                let Some(Opnd::Sgpr(base)) = s.first() else {
                    return Err("bad scalar load base");
                };
                if m == "s_buffer_load_dword" && ins.raw[0] & 0xffff_e000 == 0xf420_0000
                    && ins.raw[1] & 0xffe0_0003 == 0xfa00_0000 && ins.aux.imm >= 0
                {
                    if let Some([Word::AddressLo(lo), Word::AddressHi(hi, _), Word::Immediate(u32::MAX), Word::Immediate(0x1101_4fac)]) =
                        state.get(*base as usize..*base as usize + 4)
                    {
                        if lo == hi && !lo.uav {
                            self.raw_scalar.insert(idx, (*lo, ins.aux.imm as u32, 1));
                            state[dst.ok_or("bad scalar load destination")?] = Word::Data;
                            return Ok(());
                        }
                    }
                }
                if ins.op > 4
                    || state.get(*base as usize..*base as usize + 2) != Some(&[Word::PointerLo, Word::PointerHi])
                    || ins.raw.get(1).ok_or("truncated scalar load")? >> 25 != 0x7d
                    || ins.aux.imm < 0
                    || ins.aux.imm % 4 != 0
                {
                    return Err("unsupported root argument load");
                }
                let first = ins.aux.imm as usize / 4;
                if first >= args.len() {
                    return Err("root argument offset outside signature");
                }
                let d = dst.ok_or("bad scalar load destination")?;
                let words = state
                    .get_mut(d..d + (1 << ins.op))
                    .ok_or("scalar load beyond register file")?;
                let mut loads = Vec::new();
                for (k, word) in words.iter_mut().enumerate() {
                    let arg = args.get(first + k);
                    *word = match arg {
                        Some(Argument::Constant(..)) => Word::Data,
                        Some(Argument::Descriptor(w)) => *w,
                        None => Word::Unavailable, // unused tail of a vectorized constant load
                    };
                    loads.push(match arg {
                        Some(Argument::Constant(reg, off)) => Some((*reg, *off)),
                        _ => None,
                    });
                }
                self.loads.insert(idx, loads);
                return Ok(());
            }
            Enc::Mubuf => {
                let Some(Opnd::Sgpr(rs)) = s.get(1) else {
                    return Err("bad MUBUF descriptor");
                };
                let Some(
                    [Word::AddressLo(lo), Word::AddressHi(hi, stride), Word::Immediate(u32::MAX), Word::Immediate(0x1101_4fac)],
                ) = state.get(*rs as usize..*rs as usize + 4)
                else {
                    return Err("unresolved or unsupported buffer descriptor");
                };
                if lo != hi
                    || ins.raw[0] & (1 << 16) != 0
                    || ins.raw.get(1).ok_or("truncated buffer instruction")? & (1 << 23) != 0
                    || !matches!(s.get(2), Some(Opnd::ConstInt(_) | Opnd::Sgpr(_)))
                    || value(state, s[2]).opaque()
                    || ins.aux.mask & 3 == 3
                {
                    return Err("unsupported root buffer addressing");
                }
                if !(m.starts_with("buffer_load_dword") || (lo.uav && m.starts_with("buffer_store_dword"))) {
                    return Err("unsupported root buffer operation");
                }
                self.buffers.insert(idx, (*lo, *stride));
                return Ok(());
            }
            Enc::Mtbuf | Enc::Flat | Enc::Mimg => return Err("unsupported packed resource access"),
            _ => {}
        }
        if let Some(width) = self.scan_width {
            if self.scan_operand(idx, ins, width, dst, state)? {
                return Ok(());
            }
        }
        if m == "s_and_saveexec_b64" && s == &[Opnd::VccLo] {
            let d = dst
                .filter(|d| d % 2 == 0 && *d < 105)
                .ok_or("invalid saved EXEC pair")?;
            state[d..d + 2].copy_from_slice(&[Word::SavedExecLo, Word::SavedExecHi]);
            return Ok(());
        }
        if m == "s_mov_b64" && ins.dst == [Opnd::ExecLo] {
            if matches!(s[..], [Opnd::Sgpr(n)] if n % 2 == 0 && state.get(n as usize..n as usize + 2)
                == Some(&[Word::SavedExecLo, Word::SavedExecHi]))
            {
                return Ok(());
            }
            return Err("unresolved saved EXEC pair");
        }
        if self.ballots.contains(&idx) {
            let d = dst.ok_or("invalid ballot destination")?;
            state[d..d + 2].fill(Word::BallotBits);
            return Ok(());
        }
        // Only single-word ALU operands have provenance here. SCC can depend on
        // the true pointer high bits after masking, and VOP3 carry instructions
        // can write a second scalar destination that the decoder leaves implicit.
        if m.split('_')
            .any(|part| matches!(part, "b64" | "f64" | "u64" | "i64" | "co"))
            || s.contains(&Opnd::Scc)
        {
            return Err("implicit register pair, carry, or SCC in packed shader");
        }
        match ins.enc {
            Enc::Sop1
                if !matches!(
                    m,
                    "s_mov_b32"
                        | "s_bitset1_b32"
                        | "s_not_b32"
                        | "s_brev_b32"
                        | "s_bcnt1_i32_b32"
                        | "s_ff1_i32_b32"
                        | "s_abs_i32"
                ) =>
            {
                return Err("unsupported scalar operation on packed roots")
            }
            Enc::Sop2 if matches!(m, "s_cselect_b32" | "s_addc_u32" | "s_subb_u32") => {
                return Err("SCC may depend on a root address")
            }
            Enc::Sopk => return Err("unsupported implicit scalar operand on packed roots"),
            Enc::Sopp
                if !matches!(
                    m,
                    "s_barrier"
                        | "s_branch"
                        | "s_cbranch_execz"
                        | "s_cbranch_execnz"
                        | "s_endpgm"
                        | "s_nop"
                        | "s_waitcnt"
                        | "s_inst_prefetch"
                ) =>
            {
                return Err("unsupported packed control flow")
            }
            _ => {}
        }
        if let Some(d) = dst {
            let source = s.first().map(|o| value(state, *o));
            let rhs = s.get(1).map(|o| value(state, *o));
            let special = match (m, source, rhs) {
                ("s_mov_b32", Some(Word::SavedExecLo | Word::SavedExecHi), _) => None,
                ("s_mov_b32", v, _) => v,
                ("s_not_b32", Some(Word::BallotBits), _) => Some(Word::BallotBits),
                ("s_and_b32", Some(Word::PointerHi), Some(Word::Immediate(255))) => Some(Word::PointerHi),
                ("s_bitset1_b32", Some(Word::Immediate(bit @ 16..=29)), _) => match state[d] {
                    Word::AddressHi(b, old) => Some(Word::AddressHi(b, old | (1 << (bit - 16)))),
                    _ => None,
                },
                ("s_or_b32", Some(Word::AddressHi(b, old)), Some(Word::Immediate(bits))) if bits & !0x3fff0000 == 0 => {
                    Some(Word::AddressHi(b, old | (bits >> 16)))
                }
                _ => None,
            };
            if m == "s_bitset1_b32" && special.is_none() && state[d].opaque() {
                return Err("unsupported mutation of a root address");
            }
            if let Some(w) = special {
                state[d] = w;
                return Ok(());
            }
        }
        // Unused trailing VOP3 source fields encode s0. Only inspect actual operands.
        let sources = if ins.enc == Enc::Vop3
            && (self.lane_masks.contains(&idx)
                || ins.op < 0x100
                || matches!(
                    m,
                    "v_add_f32"
                        | "v_sub_f32"
                        | "v_mbcnt_lo_u32_b32"
                        | "v_mbcnt_hi_u32_b32"
                        | "v_bcnt_u32_b32"
                        | "v_mul_legacy_f32"
                        | "v_mul_hi_u32"
                        | "v_mul_lo_u32"
                )) {
            &s[..2.min(s.len())]
        } else {
            s.as_slice()
        };
        if sources.iter().any(|o| match value(state, *o) {
            Word::BallotBits => {
                !(matches!(m, "v_xor_b32" | "v_and_b32")
                    || (self.scan_width.is_some()
                        && matches!(m, "v_mbcnt_lo_u32_b32" | "v_mbcnt_hi_u32_b32")
                        && s.first() == Some(o)))
            }
            w => w.opaque(),
        }) {
            return Err("root address or padding used as shader data");
        }
        if let Some(d) = dst {
            state[d] = Word::Data;
        }
        Ok(())
    }

    pub(super) fn init_args(&self, h: &mut String) {
        for &(sgpr, reg, off) in &self.entry_constants {
            writeln!(h, "    s{sgpr} = cb{reg}[{}].{};", off / 4, XYZW[off as usize % 4]).unwrap();
        }
    }

    pub(super) fn declarations(&self, h: &mut String) {
        if !self.wide_images.is_empty() { h.push_str("RWTexture2D<uint64_t> wide_u10 : register(u10);\n"); }
        for binding in self.buffers.values().map(|(b, _)| *b).chain(self.raw_scalar.values().map(|(b, _, _)| *b)).collect::<BTreeSet<_>>() {
            let reg = binding.register();
            let coherent = self.coherent_loads.iter().any(|idx| self.buffers[idx].0 == binding);
            let rw = if coherent { "globallycoherent RW" } else if binding.uav { "RW" } else { "" };
            writeln!(h, "{rw}ByteAddressBuffer root_{reg} : register({reg});").unwrap();
        }
    }
}

impl Emitter<'_> {
    pub(super) fn resolve_packed(&mut self, scan: Option<&wave_scan::Scan>) -> Result<bool, String> {
        self.packed = PackedRoot::analyze(self.ex, &self.insns, scan)?;
        let Some(ref plan) = self.packed else { return Ok(false) };
        for &(_, reg, off) in &plan.entry_constants {
            let size = self.cb_size.entry(reg).or_default();
            *size = (*size).max(off + 1);
        }
        for &reg in &plan.uint_srvs { self.img_ty.insert((false, reg), 1); }
        for &(reg, off) in plan.loads.values().flatten().flatten() {
            let size = self.cb_size.entry(reg).or_default();
            *size = (*size).max(off + 1);
        }
        for (&idx, &(binding, dim, sampler)) in &plan.images {
            let slot = binding.slot;
            let (map, res) = if binding.uav { (&mut self.uav, Res::Uav(slot, dim)) } else { (&mut self.tex, Res::Tex(slot, dim)) };
            if map.insert(slot, dim).is_some_and(|old| old != dim) { return Err("inconsistent FSPX texture dimensions".into()); }
            if binding.uav && self.insns[idx].mnemonic.starts_with("image_atomic_") { self.uav_uint.insert(slot); }
            let mut resources = vec![res];
            if let Some(s) = sampler { self.samp.insert(s); resources.push(Res::Samp(s)); }
            self.res.insert(idx, resources);
        }
        Ok(true)
    }

    pub(super) fn emit_packed_load(&mut self, idx: usize, ins: &Insn) -> Result<(), String> {
        if let Some(&(binding, offset, n)) = self.packed.as_ref().and_then(|p| p.raw_scalar.get(&idx)) {
            let d = sreg_num(&ins.dst[0]).ok_or("invalid raw scalar destination")?;
            let base = if let Some(&sgpr) = self.packed.as_ref().and_then(|p| p.raw_offsets.get(&idx)) {
                let operand = sreg_opnd(sgpr);
                self.touch(&operand);
                let temp = self.tmp;
                self.tmp += 1;
                // The offset SGPR can also be a load destination.
                self.line(&format!("uint base{temp} = {};", r(&operand)?));
                format!("base{temp} + ")
            } else {String::new()};
            for k in 0..n {
                self.ws(&sreg_opnd(d + k), &format!("root_{}.Load({base}{}u)", binding.register(), offset + 4 * u32::from(k)))?;
            }
            return Ok(());
        }
        let words = self
            .packed
            .as_ref()
            .and_then(|p| p.loads.get(&idx))
            .ok_or("unresolved packed argument load")?
            .clone();
        let d = sreg_num(&ins.dst[0]).ok_or("bad packed load destination")?;
        for (k, w) in words.into_iter().enumerate() {
            let v = w.map_or_else(
                || "0u".to_string(),
                |(reg, off)| format!("cb{reg}[{}].{}", off / 4, XYZW[off as usize % 4]),
            );
            self.ws(&sreg_opnd(d + k as u8), &v)?;
        }
        Ok(())
    }

    pub(super) fn emit_packed_buffer(&mut self, idx: usize, ins: &Insn) -> Result<(), String> {
        let &(binding, stride) = self
            .packed
            .as_ref()
            .and_then(|p| p.buffers.get(&idx))
            .ok_or("unresolved packed buffer")?;
        let atomic_add = ins.mnemonic == "buffer_atomic_add";
        let n = if atomic_add { 1 } else { Self::mubuf_dwords(&ins.mnemonic).ok_or("bad root buffer width")? as u8 };
        let Opnd::Vgpr(vd) = ins.dst[0] else {
            return Err("bad root buffer data register".into());
        };
        let t = self.tmp;
        self.tmp += 1;
        let offset = ins.aux.imm;
        let address = match ins.aux.mask & 3 {
            0 => format!("{offset}u"),
            1 => format!("{offset}u + {}", r(&ins.src[0])?),
            2 => format!("{offset}u + {stride}u * {}", r(&ins.src[0])?),
            _ => return Err("unsupported root buffer addressing".into()),
        };
        self.touch(&ins.src[2]);
        self.line(&format!("uint t{t}addr = {address} + {};", r(&ins.src[2])?));
        let resource = format!("root_{}", binding.register());
        let suffix = if n == 1 { String::new() } else { n.to_string() };
        if atomic_add {
            self.line(&format!("if (exec_lo & 1u) {{ DeviceMemoryBarrier(); {resource}.InterlockedAdd(t{t}addr, v{vd}); }}"));
        } else if ins.mnemonic.starts_with("buffer_load") {
            self.line("if (exec_lo & 1u) {");
            if self.packed.as_ref().is_some_and(|plan| plan.coherent_loads.contains(&idx)) {
                self.line("DeviceMemoryBarrier();");
            }
            self.line(&format!("uint{suffix} t{t}load = {resource}.Load{suffix}(t{t}addr);"));
            for k in 0..n {
                let sw = if n == 1 {
                    String::new()
                } else {
                    format!(".{}", XYZW[k as usize])
                };
                self.ws(
                    &Opnd::Vgpr(vd.checked_add(k).ok_or("root load exceeds VGPRs")?),
                    &format!("t{t}load{sw}"),
                )?;
            }
            self.line("}");
        } else {
            let mut words = Vec::new();
            for k in 0..n {
                let v = Opnd::Vgpr(vd.checked_add(k).ok_or("root store exceeds VGPRs")?);
                self.touch(&v);
                words.push(r(&v)?);
            }
            self.line(&format!(
                "if (exec_lo & 1u) {resource}.Store{suffix}(t{t}addr, uint{suffix}({}));",
                words.join(", ")
            ));
        }
        Ok(())
    }
}
