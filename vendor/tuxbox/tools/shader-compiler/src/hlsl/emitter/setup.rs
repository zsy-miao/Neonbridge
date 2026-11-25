//! Creating an emitter.
use super::*;

impl<'a> Emitter<'a> {
    pub(in crate::hlsl) fn one_word_vector_mask(&self) -> bool {
        self.wave32_compare_masks || self.single_wave32 || self.resolve_wave32
            || self.packed.as_ref().is_some_and(|p| p.abi == RootAbi::RegisterWave32)
    }

    pub(in crate::hlsl) fn new(ex: &'a ShaderExtract, insns: Vec<Insn>) -> Self {
        let has_mode = insns.iter().any(|i| matches!((i.enc, i.op), (Enc::Sopp, 0x24 | 0x25))
            || (i.enc == Enc::Sopk && matches!(i.op, 0x12 | 0x13 | 0x15) && i.raw[0] & 63 == 1));
        // A wave64 VALU mask occupies an even-aligned SGPR pair. An odd
        // VOP3 comparison destination proves one-word masks throughout the
        // guest program, including comparisons that target even registers.
        let wave32_compare_masks = insns.iter().any(|ins| {
            ins.enc == Enc::Vop3 && ins.mnemonic.starts_with("v_cmp_")
                && matches!(ins.dst.as_slice(), [Opnd::Sgpr(n)] if n & 1 != 0)
        });
        let ps_attrs = params::inputs(ex, &insns);
        Emitter {
            lds_tile: lds_tile::LdsTile::recognize(ex, &insns),
            packed: None,
            loop_depth: 0,
            ex,
            insns,
            res: HashMap::new(),
            cb_size: BTreeMap::new(),
            zero_buffer_offsets: Default::default(),
            ordinary_bit_sites: Default::default(),
            smem_constants: Default::default(),
            tex: BTreeMap::new(),
            samp: BTreeSet::new(),
            samp_cmp: BTreeSet::new(),
            uav: BTreeMap::new(),
            sbuf: BTreeMap::new(),
            reflected_strides: BTreeSet::new(),
            persistent_queue: false,
            raw: BTreeSet::new(),
            raw_byte_index: BTreeSet::new(),
            tbuf: BTreeSet::new(),
            tbuf_uint: BTreeSet::new(),
            tbuf_ty: BTreeMap::new(),
            img_ty: BTreeMap::new(),
            uses_heap_sampler: false,
            resource_heap_base: sampler_heap::resource_base(ex).or_else(|| compute_heap_base(ex)).or_else(|| pixel_heap_base(ex, 5)),
            compute_heap: sampler_heap::resource_base(ex).is_none() && (compute_heap_base(ex).is_some() || pixel_heap_base(ex, 5).is_some()),
            sampler_heap_base: compute_heap_base(ex).and_then(|_| compute_user_sgpr(ex, 6)).or_else(|| pixel_heap_base(ex, 6)),
            pixel_heap: pixel_heap_base(ex, 5).is_some() || pixel_heap_base(ex, 6).is_some(),
            table_samplers: BTreeSet::new(),
            heap_load_ty: HashMap::new(),
            heap_sampler_slots: BTreeSet::new(),
            heap_map: ex.root_sig.as_ref().is_some_and(|root| root.params.iter().any(|p| matches!(p.kind,
                crate::meta::RootParamKind::Descriptor { kind: 3, register: 0, space: 32766 }))),
            private_samplers: ex.root_sig.as_ref().filter(|r| matches!(ex.stage, Stage::Compute | Stage::Pixel)
                // Literal samplers in heap-indexing compute shaders are recovered the same
                // way; samplers read from the sampler heap stay unresolved and fail.
                && ex.required_features.is_some_and(|flags| flags & (3 << 25) == 0 || ex.stage == Stage::Compute)
                && r.static_samplers.iter().any(|s| s[11] == 32765))
                .and_then(|r| {
                    let recovered = tuxbox_dxbc::inline_sampler::sites(&ex.isa, if ex.stage == Stage::Pixel { 5 } else { 0 })?;
                    (recovered.descriptors == r.static_samplers).then_some(recovered.sites)
                })
                // Root-less programs that declare several samplers: the literal words cannot
                // be matched to the title's registers here, so use the private slots that
                // inline_sampler::sites derives from the ISA alone (space 32765). The runtime
                // appends the same descriptors to the host root signature.
                .or_else(|| ((ex.root_sig.is_none()
                    && ex.resources.iter().filter(|r| r.res_type == 1).count() > 1)
                    // A root with no static samplers: the runtime recovers the private ones
                    // (compute_table.rs, inline_sampler::private) from the same sites.
                    || (matches!(ex.stage, Stage::Compute | Stage::Pixel)
                        && ex.root_sig.as_ref().is_some_and(|r| r.static_samplers.is_empty())
                        && ex.resources.iter().any(|r| r.res_type == 1)))
                    .then(|| {
                        let visibility = if ex.stage == Stage::Pixel { 5 } else { 0 };
                        if ex.root_sig.is_none() { tuxbox_dxbc::inline_sampler::literal_sites(&ex.isa, visibility) }
                        else { tuxbox_dxbc::inline_sampler::sites(&ex.isa, visibility) }
                    })
                    .flatten()
                    .map(|recovered| recovered.sites))
                .unwrap_or_default(),
            binding_spaces: BTreeMap::new(),
            mesh_bindings: None,
            compute_derivatives: false,
            epilog_pack: HashMap::new(),
            epilog_exp: HashMap::new(),
            cur_idx: 0,
            sbuf_flat: BTreeSet::new(),
            is_cs: ex.stage == Stage::Compute,
            is_gs: ex.stage == Stage::Geometry,
            uses_lds: false,
            uses_code: false,
            vreg_array: false,
            multi_sets: Vec::new(),
            uav_uint: BTreeSet::new(),
            wide_visibility_uavs: BTreeSet::new(),
            micropoly_vote: None,
            view_cull: None,
            required_wave_size: None,
            reduction_execz_guards: BTreeSet::new(),
            lockstep: None,
            wave_bounded_latches: BTreeSet::new(),
            portable_wave64: false,
            msel: HashMap::new(),
            bsel: HashMap::new(),
            dsel_loads: HashMap::new(),
            dsel_runtime: HashMap::new(),
            dsel_moves: HashMap::new(),
            live: Vec::new(),
            uses_quad: false,
            compute_swizzle: false,
            vote_defs: BTreeSet::new(),
            vote_uses: BTreeMap::new(),
            exec_snapshots: Default::default(),
            reflected_lds_words: None,
            hoisted_swizzle: None,
            indexed_lane: false,
            single_wave32: false,
            resolve_wave32: false,
            wave32_compare_masks,
            light_grid: false,
            page_waves: false,
            wave_cull: false,
            own_lane_reads: BTreeSet::new(),
            chosen_samplers: BTreeSet::new(),
            loop_ranges: Vec::new(),
            fspx_wave: None,
            hair_multisample: false,
            hoist_wave_exchanges: false,
            regs: if has_mode { BTreeSet::from(["tb_fp_mode".into()]) } else { BTreeSet::new() },
            body: String::new(),
            heap_locals: BTreeSet::new(),
            heap_bound: BTreeSet::new(),
            tmp: 0,
            // PS attribute slot k = the k-th input *register* that carries an interpolant and is
            // read. A register counts as read if any element packed into it is read — including
            // a system value sharing it (observed: PARTICLE_RANDOM unread + SV_IsFrontFace read
            // in the same register still occupies an attribute slot; d26fd88ff8712f9e).
            pulled_attrs: BTreeSet::new(),
            pixel_wave: false,
            dispatched: false,
            pixel_sum: false,
            flat_attrs: BTreeSet::new(),
            ps_in_interp: InputInterp::default(),
            out_interp: InputInterp::default(),
            ps_attrs,
            out_params: params::outputs(ex, &ex.output_sig),
        }
    }
}
