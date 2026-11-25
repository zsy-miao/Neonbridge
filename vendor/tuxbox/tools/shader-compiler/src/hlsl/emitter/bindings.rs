//! Which root signature binding a descriptor in scalar registers refers to.
use super::*;

impl<'a> Emitter<'a> {
    /// Root-signature range type (0 SRV, 1 UAV, 2 CBV, 3 Sampler) of the descriptor table
    /// whose pointer lives in user SGPR `sgpr` at shader entry. The FSSM register table
    /// maps user SGPR (base + i) to root parameter i, with base 8 for graphics stages and
    /// 4 for compute (observed: PS s8/s9/s10 = params 0/1/2, VS s12 = param 4, CS s5/s7 =
    /// params 1/3). `None` when the root signature is unavailable or the SGPR is not a table.
    pub(in crate::hlsl) fn root_table(&self, sgpr: u8) -> Option<&[crate::meta::RootRange]> {
        // Root descriptor-table pointers arrive in consecutive user SGPRs: from s4 in compute
        // shaders and s8 in the other stages. Vertex shaders compiled to feed a geometry
        // shader ("MainForGS") start with `s_swappc_b64 s12, s12` and receive them from s19
        // 7 registers after the swapped PC register pair instead (observed: `s_swappc_b64
        // s12, s12` -> SRV table s19, CBV s20 in a55400ccab602283; `s_swappc_b64 s4, s4` ->
        // SRV s11, CBV s12 in 4d318a496eebf6d3).
        let chained = self.insns.iter().take(16).find(|i| i.enc == Enc::Sop1 && i.mnemonic == "s_swappc_b64").and_then(|i| match i.dst.first() {
            Some(Opnd::Sgpr(n)) => Some(*n as usize + 7),
            _ => None,
        });
        // Geometry shaders: SRV table in s22, CBV table in s23 (6a6bdf123db8b5b0).
        let base = match self.ex.stage {
            Stage::Vertex if sgpr == 8 && sampler_heap::vertex_s8_table(self.ex) => 8,
            Stage::Compute => 4,
            Stage::Geometry => 22,
            // In the NGG ABI s25/s26 are mesh-visible parameter tables 9/10.
            Stage::Mesh => 16,
            _ => chained.unwrap_or(8),
        };
        let idx = (sgpr as usize).checked_sub(base)?;
        match &self.ex.root_sig.as_ref()?.params.get(idx)?.kind {
            crate::meta::RootParamKind::Table(r) => Some(r),
            _ => None,
        }
    }

    /// Resolve the guest descriptor's physical table element to its shader register.
    /// Resources occupy 32-byte table elements; samplers occupy 16-byte elements.
    /// PS09 0x41 records give the exact offsets of mixed UAV and CBV ranges.
    pub(in crate::hlsl) fn table_binding(&mut self, sgpr: u8, offset: u32, stride: u32) -> Result<(Option<u32>, u32), String> {
        let (kind, slot, space) = self.table_binding_space(sgpr, offset, stride)?;
        if self.pixel_heap && kind.is_none() {
            return Err(format!("unmapped descriptor at s{sgpr} + {offset:#x} in a heap-indexing pixel shader"));
        }
        if let Some(kind) = kind {
            // Resources are named after their register. One number used again in another
            // space gets a name of its own.
            let free = (0..16).map(|n| slot + n * SPACE_ALIAS)
                .find(|name| self.binding_spaces.get(&(kind, *name)).is_none_or(|&used| used == space));
            let Some(name) = free.filter(|_| slot < SPACE_ALIAS) else {
                return Err(format!("descriptor class {kind} register {slot} is used in multiple spaces"));
            };
            self.binding_spaces.insert((kind, name), space);
            return Ok((Some(kind), name));
        }
        Ok((kind, slot))
    }

    /// The register a resource name stands for (see `table_binding`).
    pub(in crate::hlsl) fn register_of(&self, kind: u32, name: u32) -> u32 {
        let register = name % SPACE_ALIAS;
        if name >= SPACE_ALIAS && self.binding_spaces.contains_key(&(kind, name)) && self.binding_spaces.contains_key(&(kind, register)) {
            register
        } else {
            name
        }
    }

    // A console compiler can inline all four sampler words. When PSV names
    // exactly one sampler, its (register, space) must match the static root entry.
    pub(in crate::hlsl) fn static_sampler(&self) -> Option<(u32, u32)> {
        let mut samplers = self.ex.resources.iter().filter(|p| p.res_type == 1);
        let sampler = samplers.next()?;
        if samplers.next().is_some() || sampler.lower != sampler.upper {
            return None;
        }
        self.ex.root_sig.as_ref()?.static_samplers.iter()
            .any(|entry| entry[10] == sampler.lower && entry[11] == sampler.space && (entry[12] == 0 || (self.ex.stage == Stage::Pixel && entry[12] == 5)))
            .then_some((sampler.lower, sampler.space))
    }

    pub(in crate::hlsl) fn in_loop(&self, idx: usize) -> bool {
        self.loop_ranges.iter().any(|&(head, back)| (head..=back).contains(&idx))
    }

    pub(in crate::hlsl) fn space_suffix(&self, kind: u32, register: u32) -> String {
        match self.binding_spaces.get(&(kind, register)) {
            Some(space) if *space != 0 => format!(", space{space}"),
            _ => String::new(),
        }
    }

    pub(in crate::hlsl) fn table_binding_space(&self, sgpr: u8, offset: u32, stride: u32) -> Result<(Option<u32>, u32, u32), String> {
        if let Some(mesh) = &self.mesh_bindings {
            let binding = mesh.tables.get(&(sgpr, offset)).filter(|_| stride == 32)
                .ok_or_else(|| format!("unverified mesh descriptor at s{sgpr} + {offset:#x}"))?;
            return Ok((Some(binding.kind), binding.register, binding.space));
        }
        let index = offset / stride;
        let Some(ranges) = self.root_table(sgpr) else {
            if let Some(found) = self.rootless_fssm_descriptor(sgpr, offset, stride) {
                return Ok(found);
            }
            return Ok((None, index, 0));
        };
        // Existing FSSM single-range layouts may address beyond the declared range.
        if let [range] = ranges {
            if range.base_register == 0 && range.space == 0 && matches!(range.offset_in_table, 0 | u32::MAX) {
                // The runtime can append a reflected descriptor beyond a finite 1.1
                // table. Use that binding before extrapolating the single range.
                if index >= range.num && stride == 32 && self.ex.root_sig.as_ref().is_some_and(|r| r.version == 2) {
                    if let Some((kind, register, space)) = self.uncovered_fssm_descriptor(sgpr, offset) {
                        return Ok((Some(kind), register, space));
                    }
                }
                return Ok((Some(range.range_type), index, 0));
            }
        }
        let mut next = 0u64;
        for range in ranges {
            let base = if range.offset_in_table == u32::MAX { next } else { u64::from(range.offset_in_table) };
            next = base + u64::from(range.num);
            if (base..next).contains(&u64::from(index)) {
                if offset % stride != 0 { return Err("unaligned descriptor table offset".into()); }
                let register = range.base_register.checked_add((u64::from(index) - base) as u32).ok_or("descriptor register overflow")?;
                if sgpr == 8 && sampler_heap::index(self.ex, 9).is_some() &&
                    !sampler_heap::matches_table(self.ex, offset, range.range_type, register, range.space) {
                    return Err(format!("root table slot at {offset:#x} conflicts with the pixel stage physical binding"));
                }
                return Ok((Some(range.range_type), register, range.space));
            }
        }
        // The root table does not cover this 32-byte slot. Accept the load only
        // when one FSSM user record, its used-resource row and PSV name the same
        // register, and that register is not already declared elsewhere.
        if stride == 32 {
            if let Some((kind, register, space)) = self.uncovered_fssm_descriptor(sgpr, offset) {
                return Ok((Some(kind), register, space));
            }
            // Raster roots can omit console table slots entirely (PS_Composite reads s9 +
            // 0x540 past a 7-range table). Name the register the FSSM row and PSV agree on;
            // the runtime builds the host table from the same rows.
            if self.ex.stage != Stage::Compute {
                if let Some(found) = self.fssm_descriptor(sgpr, offset, stride) {
                    return Ok(found);
                }
            }
        }
        Err(format!("unmapped descriptor at s{sgpr} + {offset:#x}"))
    }

    /// Root-less programs (most RAGE shaders) carry no RTS0, so no root table names their
    /// descriptors. FSSM used-resource rows still give each descriptor's table SGPR
    /// (location bits 8..16), dword offset and register; PSV gives its space. Accept a
    /// load only when exactly one row sits at this SGPR and offset and exactly one PSV
    /// binding of the matching class names its register.
    pub(in crate::hlsl) fn rootless_fssm_descriptor(&self, sgpr: u8, offset: u32, stride: u32) -> Option<(Option<u32>, u32, u32)> {
        if self.ex.root_sig.is_some() {
            return None;
        }
        self.fssm_descriptor(sgpr, offset, stride)
    }

    pub(in crate::hlsl) fn fssm_descriptor(&self, sgpr: u8, offset: u32, stride: u32) -> Option<(Option<u32>, u32, u32)> {
        if stride != 32 || offset % 32 != 0 {
            return None;
        }
        let mut rows = tuxbox_dxbc::compute_table::records(&self.ex.fssm)?.filter(|&[class, pos, _, _, loc]| {
            matches!(class, 0x300 | 0x302 | 0x303) && (loc >> 8) & 0xff == u32::from(sgpr) && pos.checked_mul(4) == Some(offset)
        });
        let [class, _, register, _, _] = rows.next()?;
        if rows.next().is_some() {
            return None;
        }
        let (kind, types): (u32, &[u32]) = match class {
            0x300 => (0, &[3, 4, 5]),
            0x303 => (1, &[6, 7, 8]),
            _ => (2, &[2]),
        };
        let mut spaces = self.ex.resources.iter()
            .filter(|p| types.contains(&p.res_type) && p.lower <= register && register <= p.upper)
            .map(|p| p.space);
        let space = spaces.next()?;
        if spaces.next().is_some() {
            return None;
        }
        Some((Some(kind), register, space))
    }

    pub(in crate::hlsl) fn uncovered_fssm_descriptor(&self, sgpr: u8, offset: u32) -> Option<(u32, u32, u32)> {
        if self.ex.stage != Stage::Compute {
            return None;
        }
        let resources: Vec<[u32; 4]> = self
            .ex
            .resources
            .iter()
            .map(|resource| [resource.res_type, resource.space, resource.lower, resource.upper])
            .collect();
        tuxbox_dxbc::compute_table::uncovered_descriptor(
            &self.ex.fssm,
            &resources,
            5,
            sgpr,
            offset,
            self.ex.root_sig.as_ref()?,
        )
    }
}
