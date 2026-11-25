//! Finding the resource each memory instruction uses.
use super::*;

impl<'a> Emitter<'a> {
    /// Classify descriptor loads and bind them to HLSL registers.
    ///
    /// Code is not laid out in execution order (blocks are reached through backward
    /// branches), so the symbolic SGPR state is computed as a forward dataflow fixpoint over
    /// the control-flow graph: state = (known symbolic values, set of written SGPRs), join =
    /// keep only values every predecessor agrees on / union of written sets. A final pass
    /// then walks each block with its entry state and records the resources.
    pub(in crate::hlsl) fn resolve_resources(&mut self) -> Result<(), String> {
        type State = (HashMap<u16, Sym>, BTreeSet<u16>);
        let n = self.insns.len();
        if n == 0 {
            return Ok(());
        }
        let off2idx: HashMap<usize, usize> = self.insns.iter().enumerate().map(|(i, x)| (x.offset, i)).collect();
        // Block leaders and per-instruction branch targets.
        let mut leaders: BTreeSet<usize> = BTreeSet::new();
        leaders.insert(0);
        let mut tgt: HashMap<usize, usize> = HashMap::new();
        for (i, ins) in self.insns.iter().enumerate() {
            if ins.enc != Enc::Sopp {
                continue;
            }
            if Self::branch_cond(ins.op).is_some() {
                let t = ins.offset as i64 + 4 + ins.aux.imm * 4;
                if let Some(&ti) = off2idx.get(&(t as usize)) {
                    tgt.insert(i, ti);
                    leaders.insert(ti);
                }
                leaders.insert(i + 1);
            } else if ins.op == 1 {
                leaders.insert(i + 1);
            }
        }
        let starts: Vec<usize> = leaders.into_iter().filter(|&s| s < n).collect();
        let block_of = |i: usize| starts.partition_point(|&s| s <= i) - 1;
        let ranges: Vec<std::ops::Range<usize>> =
            (0..starts.len()).map(|b| starts[b]..starts.get(b + 1).copied().unwrap_or(n)).collect();
        let succs: Vec<Vec<usize>> = ranges
            .iter()
            .map(|r| {
                let last = r.end - 1;
                let ins = &self.insns[last];
                let mut v = Vec::new();
                let is_sopp = ins.enc == Enc::Sopp;
                if let Some(&t) = tgt.get(&last) {
                    v.push(block_of(t));
                }
                let falls = !(is_sopp && (ins.op == 1 || ins.op == 2));
                if falls && r.end < n {
                    v.push(block_of(r.end));
                }
                v
            })
            .collect();

        // Both passes read the same decoded program. Snapshot it once for all blocks.
        let program = self.insns.clone();
        // Direct branches have explicit graph edges. Preserve the previous
        // behavior for other control transfers, not modeled by this graph.
        let track_heap_origin = (self.resource_heap_base.is_some() || self.sampler_heap_base.is_some())
            && program.iter().enumerate().all(|(i, ins)| {
            if ins.enc == Enc::Sopp && Self::branch_cond(ins.op).is_some() {
                tgt.contains_key(&i)
            } else {
                !["branch", "setpc", "swappc", "call", "rfe", "trap"]
                    .iter().any(|name| ins.mnemonic.contains(name))
            }
        }) && n <= (usize::MAX >> 17);
        // Fixpoint (analysis only: recordings are rolled back afterwards).
        let saved = (
            self.res.clone(),
            self.cb_size.clone(),
            self.tex.clone(),
            self.samp.clone(),
            self.samp_cmp.clone(),
            self.uav.clone(),
            self.sbuf.clone(),
            self.tbuf.clone(),
            self.tbuf_uint.clone(),
        );
        let mut first_err: Option<String> = None;
        let mut states: Vec<Option<State>> = vec![None; ranges.len()];
        let mut entry = HashMap::new();
        if let Some(mesh) = &self.mesh_bindings {
            for &register in mesh.raw.keys() {
                entry.insert(register.into(), Sym::MeshRawAddress { register, word: 0 });
                entry.insert(u16::from(register) + 1, Sym::MeshRawAddress { register, word: 1 });
            }
        }
        if self.pixel_heap {
            lane_reads::entry(&mut entry);
        }
        states[0] = Some((entry, BTreeSet::new()));
        let mut work = vec![0usize];
        let mut budget = 20_000usize;
        while let Some(b) = work.pop() {
            if budget == 0 {
                return Err("descriptor dataflow did not converge".into());
            }
            budget -= 1;
            let (mut sym, mut written) = states[b].clone().unwrap();
            if let Err(e) = self.res_block(&program, &mut sym, &mut written, ranges[b].clone(), track_heap_origin) {
                if std::env::var_os("TB_DFDEBUG").is_some() {
                    eprintln!("block {b} analysis error: {e}");
                }
                // Unknown effect: forget everything and treat every SGPR as written. Keep
                // the first failure — it is the root cause if the recording pass fails too.
                first_err.get_or_insert(e);
                sym.clear();
                written = (0u16..=105).collect();
            }
            for &s in &succs[b] {
                let changed = match &mut states[s] {
                    slot @ None => {
                        let mut next = sym.clone();
                        if track_heap_origin {
                            heap_origins::canonicalize(&mut next, s);
                        }
                        *slot = Some((next, written.clone()));
                        true
                    }
                    Some((osym, owr)) => {
                        let before = (osym.clone(), owr.len());
                        let heap = track_heap_origin.then(|| heap_origins::intersection(osym, &sym, s));
                        let keys: Vec<u16> = osym.keys().copied().collect();
                        for k in keys {
                            let a = osym[&k];
                            if track_heap_origin && heap_origins::tracked(a) {
                                osym.remove(&k);
                                continue;
                            }
                            match sym.get(&k).copied() {
                                Some(b) if a == b => {}
                                Some(b) => match self.join_desc(a, b) {
                                    Some(j) => {
                                        osym.insert(k, j);
                                    }
                                    None => {
                                        osym.remove(&k);
                                    }
                                },
                                // Known on one path only. A descriptor stays (see below);
                                // anything else is dropped.
                                None if matches!(a, Sym::Desc { .. } | Sym::Multi { .. } | Sym::Table(_)) => {}
                                None => {
                                    osym.remove(&k);
                                }
                            }
                        }
                        // Optimistic join for descriptors: a register that holds a descriptor
                        // on one incoming path and nothing known on the other is treated as
                        // holding the descriptor. Compiled code loads descriptors inside
                        // blocks that are skipped only when no lane is active
                        // (`s_cbranch_execz`), then uses them after the join; on the skipped
                        // path the result is unused. A later use as a descriptor would be
                        // invalid anyway if the register held something else.
                        for (k, v) in sym.iter() {
                            if matches!(v, Sym::Desc { .. } | Sym::Multi { .. } | Sym::Table(_)) && !osym.contains_key(k) {
                                osym.insert(*k, *v);
                            }
                        }
                        if let Some(heap) = heap {
                            osym.extend(heap);
                        }
                        owr.extend(written.iter().copied());
                        before != (osym.clone(), owr.len())
                    }
                };
                if changed && !work.contains(&s) {
                    work.push(s);
                }
            }
        }
        (self.res, self.cb_size, self.tex, self.samp, self.samp_cmp, self.uav, self.sbuf, self.tbuf, self.tbuf_uint) = saved;

        // TB_DFDEBUG=<sgpr>: print that register's symbol at every block entry (diagnostics).
        if let Some(reg) = std::env::var("TB_DFDEBUG").ok().and_then(|v| v.parse::<u16>().ok()) {
            for (b, r) in ranges.iter().enumerate() {
                let s = states[b].as_ref().map(|(sym, wr)| (sym.get(&reg).copied(), wr.contains(&reg)));
                eprintln!("block {b} @{:#x}..: s{reg} = {s:?} succs={:?}", self.insns[r.start].offset, succs[b]);
            }
            eprintln!("first analysis error: {first_err:?}");
        }
        self.msel.clear();
        self.bsel.clear();
        self.dsel_loads.clear();
        self.dsel_runtime.clear();
        self.dsel_moves.clear();
        self.raw_byte_index.clear();
        // Recording pass with the converged entry states.
        for (b, r) in ranges.iter().enumerate() {
            if let Some((mut sym, mut written)) = states[b].clone() {
                self.res_block(&program, &mut sym, &mut written, r.clone(), track_heap_origin).map_err(|e| first_err.clone().unwrap_or(e))?;
            }
        }
        Ok(())
    }

    /// Shaders may read only one field of a padded instance or view record.
    /// Array indexing and byte-offset loads both need the complete native stride.
    pub(in crate::hlsl) fn resolve_structured_strides(&mut self) {
        let keys: Vec<_> = self.sbuf.keys().copied().collect();
        for (uav, slot) in keys {
            let kind = u32::from(uav);
            let space = self.binding_spaces.get(&(kind, slot)).copied().unwrap_or(0);
            let words = self.table_structured_stride(slot, space, uav).or_else(|| {
                if space != 0 || self.native_binding(uav, slot, space) != (slot, space) { return None; }
                fssm_bindings::stride_words(&self.ex.fssm, &self.ex.resources, self.ex.stage, slot, uav)
            });
            if let Some(words) = words {
                self.sbuf.insert((uav, slot), words);
                self.reflected_strides.insert((uav, slot));
            }
        }
    }

    /// The physical table slot can differ from the resource register in a mixed
    /// root. Require the FSSM offset, root range, and structured PSV binding to agree.
    pub(in crate::hlsl) fn table_structured_stride(&self, slot: u32, space: u32, uav: bool) -> Option<u32> {
        if let Some(mesh) = &self.mesh_bindings {
            return mesh.tables.values().find(|b| (b.kind, b.register, b.space) == (u32::from(uav), slot, space))?.stride_words;
        }
        let class = if uav { 0x303 } else { 0x300 };
        let slot = self.register_of(u32::from(uav), slot);
        let psv_type = if uav { 8 } else { 5 };
        // The used-resource rows and PSV keep the compiler's own numbering.
        let (native, native_space) = self.native_binding(uav, slot, space);
        if !self.ex.resources.iter().any(|r| (r.res_type, r.space, r.lower, r.upper) == (psv_type, native_space, native, native)) {
            return None;
        }
        let mut rows = tuxbox_dxbc::compute_table::records(&self.ex.fssm)?
            .filter(|&[c, _, r, _, loc]| c == class && r == native && self.table_locations().iter().any(|&(_, l)| l == loc));
        let [_, offset, _, stride, location] = rows.next()?;
        let &(sgpr, _) = self.table_locations().iter().find(|&&(_, l)| l == location)?;
        if rows.next().is_some() || offset % 8 != 0 || !(4..=2048).contains(&stride) || stride % 4 != 0 {
            return None;
        }
        let binding = self.table_binding_space(sgpr, offset.checked_mul(4)?, 32).ok()?;
        (binding == (Some(u32::from(uav)), slot, space)).then_some(stride / 4)
    }

    /// User SGPR and used-resource location of each descriptor table.
    pub(in crate::hlsl) fn table_locations(&self) -> &'static [(u8, u32)] {
        match self.ex.stage {
            Stage::Compute => &[(4, 0x404), (5, 0x10504)],
            Stage::Vertex | Stage::Pixel => &[(8, 0x804)],
            _ => &[],
        }
    }

    /// The register and space the shader's own metadata gives a resource bound at `slot`.
    /// An embedded root signature can number its ranges independently of the shader;
    /// the table position is what both sides share.
    pub(in crate::hlsl) fn native_binding(&self, uav: bool, slot: u32, space: u32) -> (u32, u32) {
        let class = if uav { 0x303 } else { 0x300 };
        let slot = self.register_of(u32::from(uav), slot);
        if self.mesh_bindings.is_some() || self.ex.root_sig.is_none() { return (slot, space); }
        let Some(rows) = tuxbox_dxbc::compute_table::records(&self.ex.fssm) else { return (slot, space) };
        let mut found = rows.filter(|&[c, _, _, _, _]| c == class).filter_map(|[_, offset, register, _, location]| {
            let &(sgpr, _) = self.table_locations().iter().find(|&&(_, l)| l == location)?;
            let bound = self.table_binding_space(sgpr, offset.checked_mul(4)?, 32).ok()?;
            (bound == (Some(u32::from(uav)), slot, space)).then_some(register)
        });
        let (Some(native), None) = (found.next(), found.next()) else { return (slot, space) };
        let types = if uav { 6..=8 } else { 3..=5 };
        let mut spaces = self.ex.resources.iter()
            .filter(|p| types.contains(&p.res_type) && p.lower <= native && native <= p.upper)
            .map(|p| p.space);
        match (spaces.next(), spaces.next()) {
            (Some(space), None) => (native, space),
            // The same register in two spaces: only an unchanged number is unambiguous.
            _ => (slot, space),
        }
    }

    /// Declared element size, in dwords, of a structured buffer.
    ///
    /// The stride is a property of the buffer view, not of the shader: on the console the
    /// hardware multiplies the element index by the descriptor's stride. D3D12 instead takes
    /// the stride from the HLSL element type (observed: a translation declaring a 12-byte
    /// element for the 16-byte primitive-data buffer read the wrong rows), so the declared
    /// element has to be the real one. Use a validated native stride when available. For
    /// shaders without one, the fallback uses observed access extents. What the PC build
    /// of the same game shows for 801 buffers bound at the same register: stride 16 for
    /// every buffer the Xbox code touches with extents of 8..28 bytes, 64 for extents of 64.
    /// Hence: extents of 8..28 bytes -> 16-byte elements (larger offsets address following
    /// elements), anything larger rounded up to a multiple of 16. A buffer only ever read or
    /// written one dword at offset 0 keeps a 4-byte element: ScatterCopyCS reads its index
    /// buffer that way and its view really has stride 4 (forcing 16 scrambled the per-object
    /// data it uploads); no 16-byte buffer in the comparison was touched like that.
    /// A heuristic (TECHNICAL_DEBT).
    pub(in crate::hlsl) fn sbuf_words(&self, key: (bool, u32)) -> u32 {
        let raw = self.sbuf.get(&key).copied().unwrap_or(1).max(1);
        if self.persistent_queue || self.micropoly_vote.is_some() || self.reflected_strides.contains(&key) {
            return raw;
        }
        // Unreal's instance-culling work item packs two uints per record. Both
        // the single-instance and general permutations index those words using
        // the descriptor's 8-byte stride. Rounding this SRV to a float4 skips
        // every other work item and leaves part of the visible-instance list unwritten.
        if raw == 2 && !key.0 && !self.sbuf_flat.contains(&key)
            && self.ex.entry.as_deref() == Some("InstanceCullBuildInstanceIdBufferCS")
        {
            return 2;
        }
        match raw {
            1 => 1,
            2..=7 => 4,
            _ => raw.div_ceil(4) * 4,
        }
    }

    /// Raw SRV behind a root descriptor whose GPU address arrives in user SGPR `sgpr`.
    pub(in crate::hlsl) fn root_raw_srv(&mut self, sgpr: u8) -> Option<Res> {
        let mut rows = tuxbox_dxbc::compute_table::records(&self.ex.fssm)?
            .filter(|&[class, _, _, _, loc]| class == 0x100 && (loc >> 8) & 0xff == u32::from(sgpr));
        let [_, _, register, _, _] = rows.next()?;
        if rows.next().is_some() {
            return None;
        }
        let mut raw = self.ex.resources.iter().filter(|p| p.res_type == 4 && p.lower == register && p.upper == register);
        let space = raw.next()?.space;
        if raw.next().is_some() {
            return None;
        }
        if self.binding_spaces.insert((0, register), space).is_some_and(|old| old != space) {
            return None;
        }
        self.raw_binding(register, false)
    }

    pub(in crate::hlsl) fn raw_binding(&mut self, slot: u32, is_uav: bool) -> Option<Res> {
        let ty = if is_uav { 7 } else { 4 };
        let space = self.binding_spaces.get(&(u32::from(is_uav), slot)).copied().unwrap_or(0);
        let (native, space) = self.native_binding(is_uav, slot, space);
        if self.ex.resources.iter().any(|p| p.res_type == ty && p.space == space && p.lower <= native && native <= p.upper) {
            self.raw.insert((is_uav, slot));
            Some(Res::Raw(slot, is_uav))
        } else {
            None
        }
    }

    /// Resource a MUBUF instruction addresses when its descriptor is `slot` of `table`
    /// (also records declaration requirements).
    pub(in crate::hlsl) fn mubuf_res(&mut self, ins: &Insn, offset: u32, table: u8) -> Result<Res, String> {
        let (kind, slot) = self.table_binding(table, offset, 32)?;
        let is_uav = match kind {
            Some(0) => false,
            Some(1) => true,
            Some(2) => {
                // Per-lane indexed read of a constant buffer (e.g. cbuffer arrays indexed by
                // a varying value): size the declaration to the maximum.
                self.cb_size.insert(slot, 4096 * 4);
                return Ok(Res::CbDyn(slot));
            }
            other => return Err(format!("mubuf at {:#x}: buffer in table of kind {other:?}", ins.offset)),
        };
        if let Some(raw) = self.raw_binding(slot, is_uav) {
            return Ok(raw);
        }
        if ins.mnemonic.contains("_atomic_") {
            if !is_uav {
                return Err(format!("mubuf at {:#x}: atomic on a non-UAV buffer", ins.offset));
            }
            // PSV0 tells typed (6) from structured (8) UAVs. Atomics need a uint element, so
            // a typed UAV used atomically is declared RWBuffer<uint>.
            let (native, space) = self.native_binding(true, slot, self.binding_spaces.get(&(1, slot)).copied().unwrap_or(0));
            let structured = self.ex.resources.iter().any(|p| p.res_type == 8 && p.space == space && p.lower <= native && native <= p.upper);
            if structured {
                let e = self.sbuf.entry((true, slot)).or_insert(0);
                *e = (*e).max(ins.aux.imm as u32 / 4 + 1);
                return Ok(Res::Buf(slot, true));
            }
            self.tbuf.insert((true, slot));
            self.tbuf_uint.insert(slot);
            return Ok(Res::TBuf(slot, true));
        }
        if ins.mnemonic.contains("_format_") {
            // Typed buffer (Buffer<T> / RWBuffer<T>), addressed by element index.
            self.tbuf.insert((is_uav, slot));
            return Ok(Res::TBuf(slot, is_uav));
        }
        let ndw = Self::mubuf_dwords(&ins.mnemonic).ok_or_else(|| format!("mubuf op {} unsupported", ins.mnemonic))?;
        let end = ins.aux.imm as u32 / 4 + ndw;
        let e = self.sbuf.entry((is_uav, slot)).or_insert(0);
        *e = (*e).max(end);
        Ok(Res::Buf(slot, is_uav))
    }

    /// Candidate descriptors `(table, offset)` of a symbol that is dword 0 of a descriptor.
    pub(in crate::hlsl) fn desc_candidates(&self, s: Option<&Sym>) -> Option<Vec<(u8, u32)>> {
        match s {
            Some(Sym::Desc { table, off, k: 0 }) => Some(vec![(*table, *off)]),
            Some(Sym::Multi { set, k: 0 }) => self.multi_sets.get(*set as usize).cloned(),
            _ => None,
        }
    }

    /// Join of two different descriptor symbols at a control-flow merge: the union of
    /// their candidates when both are the same dword of some descriptor, else nothing.
    pub(in crate::hlsl) fn join_desc(&mut self, a: Sym, b: Sym) -> Option<Sym> {
        let parts = |s: Sym, sets: &Vec<Vec<(u8, u32)>>| -> Option<(Vec<(u8, u32)>, u32)> {
            match s {
                Sym::Desc { table, off, k } => Some((vec![(table, off)], k)),
                Sym::Multi { set, k } => sets.get(set as usize).map(|v| (v.clone(), k)),
                _ => None,
            }
        };
        let (mut ca, ka) = parts(a, &self.multi_sets)?;
        let (cb, kb) = parts(b, &self.multi_sets)?;
        if ka != kb {
            return None;
        }
        ca.extend(cb);
        ca.sort();
        ca.dedup();
        if ca.len() > 16 {
            return None; // not a small "one of N" selection
        }
        let set = match self.multi_sets.iter().position(|v| *v == ca) {
            Some(i) => i,
            None => {
                self.multi_sets.push(ca);
                self.multi_sets.len() - 1
            }
        };
        Some(Sym::Multi { set: set as u16, k: ka })
    }

    // Some Xbox vertex fetch programs assemble a raw buffer descriptor from
    // a root GPU address, a byte stride of one and an unbounded record count.
    // Recognize the complete four-word construction and require a unique SRV.
    pub(in crate::hlsl) fn direct_raw_descriptor(&mut self, instructions: &[Insn], written: &BTreeSet<u16>) -> Option<(u16, u32)> {
        let [format, size, low, high] = instructions else { return None; };
        if [format, size, low].iter().any(|op| op.mnemonic != "s_mov_b32")
            || format.src != [Opnd::Literal(0x11014fac)]
            || size.src != [Opnd::ConstInt(-1)] { return None; }
        let [Opnd::Sgpr(base)] = low.dst[..] else { return None; };
        let [Opnd::Sgpr(root)] = low.src[..] else { return None; };
        if base > 124 || root > 126 || written.contains(&u16::from(root)) || written.contains(&(u16::from(root)+1))
            || format.dst != [Opnd::Sgpr(base+3)] || size.dst != [Opnd::Sgpr(base+2)]
            || high.dst != [Opnd::Sgpr(base+1)] || !raw_index::sets_byte_stride(high, root+1) { return None; }
        let [binding] = &self.ex.resources[..] else { return None; };
        if binding.res_type != 4 || binding.lower != binding.upper { return None; }
        let slot = binding.lower;
        self.binding_spaces.insert((0, slot), binding.space);
        self.raw.insert((false,slot));
        Some((u16::from(base),slot))
    }

    pub(in crate::hlsl) fn mesh_raw_descriptor(&mut self, sym: &HashMap<u16, Sym>, base: u16) -> Result<Option<Res>, String> {
        let Some(Sym::MeshRawAddress { register, word: 0 }) = sym.get(&base).copied() else {
            return Ok(None);
        };
        if sym.get(&(base + 1)) != Some(&Sym::MeshRawAddress { register, word: 2 })
            || sym.get(&(base + 2)) != Some(&Sym::InlineWord(u32::MAX))
            || sym.get(&(base + 3)) != Some(&Sym::InlineWord(0x11014fac))
        {
            return Err(format!("unverified mesh root SRV descriptor at s{base}"));
        }
        let (slot, space) = self.mesh_bindings.as_ref().and_then(|mesh| mesh.raw.get(&register)).copied()
            .ok_or("mesh root SRV missing from signature")?;
        if self.binding_spaces.insert((0, slot), space).is_some_and(|previous| previous != space) {
            return Err(format!("mesh SRV t{slot} is used in multiple spaces"));
        }
        self.raw.insert((false, slot));
        Ok(Some(Res::Raw(slot, false)))
    }
}
