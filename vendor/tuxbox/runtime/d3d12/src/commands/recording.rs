//! State changes and checks shared by the recorded commands.
use super::*;

impl Recording {
    pub(super) fn track_texture(accesses: &mut Vec<Arc<Texture>>, texture: &Arc<Texture>) -> Result<(), HRESULT> {
        if !accesses.iter().any(|known| Arc::ptr_eq(known, texture)) {
            accesses.try_reserve(1).map_err(|_| E_OUTOFMEMORY)?;
            accesses.push(Arc::clone(texture));
        }
        Ok(())
    }

    pub(super) fn prepare_copy(
        &mut self,
        destination: Option<&Arc<Texture>>,
        source: Option<&Arc<Texture>>,
    ) -> Result<(), HRESULT> {
        for (accesses, texture) in [
            (&mut self.placed_reads, source),
            (&mut self.placed_writes, destination),
        ] {
            if let Some(texture) = texture {
                Self::track_texture(accesses, texture)?;
            }
        }
        Ok(())
    }

    /// Leave out one command TuxBox cannot express. Failing the list instead
    /// would also drop its other work, including the fence writes other queues
    /// wait for, and a stalled queue trips the title's GPU watchdog.
    pub(super) fn skip(what: &str, error: HRESULT) {
        static COUNT: std::sync::atomic::AtomicU64 = std::sync::atomic::AtomicU64::new(0);
        let count = COUNT.fetch_add(1, Ordering::Relaxed) + 1;
        if count <= 16 || count.is_power_of_two() {
            crate::diagnostics::log(&format!(
                "skipped {what} ({:#010x}, #{count}, guest caller {:#x})",
                error.0, crate::survey::title_rva().unwrap_or(0),
            ));
        }
    }

    pub(super) fn compute_dispatch(&mut self, list: &List, dispatch: impl FnOnce()) {
        let null_usage = self.bindings.null_usage.clone();
        let dispatch = move || {
            dispatch();
            if let Some(tag) = &null_usage {
                crate::diagnostics::display::record_null(tag, false);
            }
        };
        if let Some(table) = self.bindings.reflected_table::<true>() {
            if let Err(error) = self.bindings.with_reflected_table::<true>(
                table, list, self.descriptor_arena.as_ref(), &mut self.descriptor_leases,
                self.resource_heap.as_ref(), &mut self.sampled_heaps, &self.resources, dispatch,
            ) {
                Self::skip("compute dispatch with a reflected table", error.code());
            }
            return;
        }
        if self.descriptor_arena.is_some() && !self.bindings.supports_paged_draw::<true>() {
            Self::skip("compute dispatch: pipeline has no paged descriptor layout", E_NOTIMPL);
            return;
        }
        if let Err(error) = self.bindings.refresh_inline_tables::<true>(self.descriptor_arena.as_ref(), &mut self.descriptor_leases) {
            Self::skip("compute dispatch: descriptor table refresh", error.code());
            return;
        }
        self.sample_tables::<true>();
        let Some(root) = self.bindings.scene_root() else {
            dispatch();
            return;
        };
        let result = self.bindings.scene_resource(list)
            .and_then(|scene| {
                if let Some([instances, normals]) = &scene.geometry {
                    raytracing::retain(self, &[&scene.scene.0, &instances.0, &normals.0])?;
                } else {
                    raytracing::retain(self, &[&scene.scene.0])?;
                }
                Ok(scene)
            })
            .and_then(|scene| {
                let va = scene.scene.1;
                let geometry = scene.geometry.as_ref().map(|[instances, normals]| [instances.1, normals.1]);
                if scene.recheck_on_submit()
                    && !matches!(self.scene_commands.last(), Some(raytracing::SceneCommand::Check(last)) if *last == scene)
                {
                    self.scene_commands.try_reserve(1).map_err(|_| E_OUTOFMEMORY)?;
                    self.scene_commands.push(raytracing::SceneCommand::Check(scene));
                }
                unsafe { self.bindings.dispatch_scene(&list.native, root, va, geometry, dispatch) };
                crate::diagnostics::log_once(&root.program.first_dispatch, || {
                    format!("recorded inline compute shader {} dispatch with scene GPU VA {va:#x}", root.program.source_hash)
                });
                Ok(())
            });
        if let Err(error) = result {
            Self::skip("compute dispatch: inline ray scene binding", error.code());
        }
    }

    pub(super) fn draw(&mut self, list: &List, rasterizes: bool, draw: impl FnOnce()) {
        let null_usage = self.bindings.null_usage.clone();
        let display_bound = self.display_bound;
        let draw = move || {
            draw();
            if rasterizes {
                crate::diagnostics::display::draw(display_bound, null_usage.is_some());
                if let Some(tag) = &null_usage {
                    crate::diagnostics::display::record_null(tag, display_bound);
                }
            }
        };
        if rasterizes {
            for target in &self.bound_placed_targets {
                if let Err(error) = Self::track_texture(&mut self.placed_writes, target) {
                    self.error = error;
                    return;
                }
            }
        }
        let trace = |state: &mut Self| {
            if let Some(trace) = &state.bindings.draw_trace {
                trace.recorded(|| state.bindings.raster_inputs(state.resource_heap.as_ref(), state.descriptor_arena.as_deref(), state.display_bound));
                if let Some(target) = &state.trace_target
                    && let Some(label) = trace.sample_label()
                    && let Some(sample) = crate::present::sampler::record(
                        &list.native, target, D3D12_RESOURCE_STATE_RENDER_TARGET,
                    )
                {
                    state.trace_samples.push((label, sample));
                }
            }
        };
        if let Some(table) = self.bindings.reflected_table::<false>() {
            if self.stencil_slot.is_some() {
                Self::skip("draw: stencil export with a reflected table", E_NOTIMPL);
                return;
            }
            if let Some(binding) = table.index_map {
                // These PSOs have one descriptor table followed by one root
                // constants parameter. The native ISA selects its scalar
                // buffer with a word (and optional mask) from that parameter.
                let descriptor = self.bindings.graphics_constant(1, binding.scalar_word)
                    .and_then(|word| self.resource_heap.as_ref()?.scalar_descriptor(
                        word & binding.scalar_mask, self.descriptor_arena.as_deref(),
                    ));
                let Some(descriptor) = descriptor else {
                    Self::skip("draw: no scalar descriptor for the indexed load", E_INVALIDARG);
                    return;
                };
                if self.scalar_descriptors.try_reserve(1).is_err() {
                    self.error = E_OUTOFMEMORY;
                    return;
                }
                self.scalar_descriptors.insert(descriptor);
                if let Some(arena) = &self.descriptor_arena
                    && !self.indexed_arenas.iter().any(|used| Arc::ptr_eq(used, arena))
                {
                    if self.indexed_arenas.try_reserve(1).is_err() {
                        self.error = E_OUTOFMEMORY;
                        return;
                    }
                    self.indexed_arenas.push(arena.clone());
                }
            }
            if let Err(error) = self.bindings.with_reflected_table::<false>(
                table, list, self.descriptor_arena.as_ref(), &mut self.descriptor_leases,
                self.resource_heap.as_ref(), &mut self.sampled_heaps, &self.resources, draw,
            ) {
                Self::skip("draw with a reflected table", error.code());
            } else { trace(self); }
            return;
        }
        if self.descriptor_arena.is_some() && !self.bindings.supports_paged_draw::<false>() {
            Self::skip("draw: pipeline has no paged descriptor layout", E_NOTIMPL);
            return;
        }
        if let Err(error) = self.bindings.refresh_inline_tables::<false>(self.descriptor_arena.as_ref(), &mut self.descriptor_leases) {
            Self::skip("draw: descriptor table refresh", error.code());
            return;
        }
        self.sample_tables::<false>();
        if let Some(slot) = self.stencil_slot
            && self.dsv_bound
        {
            let status = self
                .stencil_targets
                .as_ref()
                .ok_or_else(|| windows::core::Error::from(E_NOTIMPL))
                .and_then(|t| t.draw(&list.native, slot, draw));
            if let Err(e) = status {
                Self::skip("draw: stencil export", e.code());
            } else { trace(self); }
        } else {
            draw();
            trace(self);
        }
    }

    pub(super) fn sample_tables<const COMPUTE: bool>(&mut self) {
        if let Some(heap) = &self.resource_heap
            && self.bindings.sample_tables::<COMPUTE>(heap, &mut self.sampled_heaps).is_none()
        {
            self.sample_heap();
        }
    }

    pub(super) fn sample_heap(&mut self) {
        if let Some(heap) = &self.resource_heap {
            descriptors::include_range(&mut self.sampled_heaps, heap.cpu.clone());
        }
    }

    pub(super) fn can_record(&mut self, native: &ID3D12GraphicsCommandList) -> bool {
        if !self.ensure_open(native) { return false; }
        if let Err(error) = self.prefix.finish(native) {
            self.error = error.code();
            return false;
        }
        true
    }

    pub(super) fn ensure_open(&mut self, native: &ID3D12GraphicsCommandList) -> bool {
        if self.error.is_err() {
            return false;
        }
        match &self.phase {
            ListState::Fresh(allocator, initial) => {
                // The factory closed the empty native list to release its allocator.
                // Reopen with its original bindings when the first operation records.
                if let Err(error) = unsafe { native.Reset(allocator, initial.as_ref()) } {
                    self.error = error.code();
                    return false;
                }
                self.phase = ListState::Open;
            }
            ListState::Open => {}
            ListState::Closed => self.error = E_FAIL,
        }
        self.error.is_ok()
    }
}
