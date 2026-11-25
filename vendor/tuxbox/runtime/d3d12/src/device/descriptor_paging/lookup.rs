//! Finding or materialising the shader-visible copy of a guest table.
use super::*;

impl Arena {
    pub(crate) fn gpu(
        self: &Arc<Self>,
        address: u64,
        length: u32,
        leases: &mut DescriptorLeases,
    ) -> Result<D3D12_GPU_DESCRIPTOR_HANDLE> {
        self.gpu_table(address, length, &[0..length], leases)
    }

    pub(crate) fn resident_gpu(&self, address: u64, length: u32) -> Option<D3D12_GPU_DESCRIPTOR_HANDLE> {
        let offset = (address as usize).checked_sub(self.base())?;
        let first = u32::try_from(offset / self.stride).ok()?;
        let tables = self.tables.lock().unwrap_or_else(|e| e.into_inner());
        let slot = tables
            .ranges
            .get(&(first, length))
            .map(|entry| entry.slot)
            .or_else(|| {
                (!self.inline)
                    .then(|| tables.direct.contiguous(first, length))
                    .flatten()
            })?;
        Some(self.gpu_handle(slot))
    }

    pub(super) fn gpu_handle(&self, slot: u32) -> D3D12_GPU_DESCRIPTOR_HANDLE {
        let base = self
            .native
            .as_ref()
            .map_or(0x2000_0000, |n| n.visible_gpu_start);
        D3D12_GPU_DESCRIPTOR_HANDLE {
            ptr: base + u64::from(slot) * self.stride as u64,
        }
    }

    pub(crate) fn gpu_table(
        self: &Arc<Self>,
        address: u64,
        length: u32,
        active: &[std::ops::Range<u32>],
        leases: &mut DescriptorLeases,
    ) -> Result<D3D12_GPU_DESCRIPTOR_HANDLE> {
        let offset = (address as usize)
            .checked_sub(self.base())
            .ok_or(E_INVALIDARG)?;
        if length == 0 || !offset.is_multiple_of(self.stride) {
            return Err(E_INVALIDARG.into());
        }
        let first = u32::try_from(offset / self.stride).map_err(|_| E_INVALIDARG)?;
        if first.checked_add(length).is_none_or(|end| end > self.count) {
            return Err(E_INVALIDARG.into());
        }
        leases.reserve_slot(self)?;
        let mut retired = RetiredViews::default();
        let result = (|| -> Result<D3D12_GPU_DESCRIPTOR_HANDLE> {
            let mut tables_guard = self.tables.lock().unwrap_or_else(|e| e.into_inner());
            let tables = &mut *tables_guard;
            let key = (first, length);
            let tick = tables.next_tick();
            let (entry_id, resident, fresh) = if let Some(entry) = tables.ranges.get_mut(&key) {
                entry.last_used = tick;
                (Some(entry.id), entry.slot, false)
            } else if !self.inline && let Some(index) = tables.direct.contiguous(first, length) {
                (None, index, false)
            } else {
                let slot = tables.allocate_contiguous(self, length, &mut retired)?;
                if !self.inline {
                    self.copy(first, slot, length, &mut retired);
                }
                let id = tables.alloc_id();
                tables.ranges.insert(
                    key,
                    TableEntry {
                        id,
                        slot,
                        pins: 0,
                        direct_pinned: false,
                        last_used: tick,
                    },
                );
                tables.by_id.insert(id, key);
                (Some(id), slot, true)
            };
            if self.inline {
                let inline_result = (|| -> Result<()> {
                    let Some(native) = &self.native else {
                        return Ok(());
                    };
                    let start = native.visible_cpu_start;
                    let mut materialized = 0;
                    for index in active
                        .iter()
                        .flat_map(|range| range.clone().take_while(|&i| i < length))
                    {
                        let index = index as usize;
                        let cell = address as usize + index * INLINE_BYTES;
                        let record = self.inline_record(cell);
                        let slot = resident as usize + index;
                        if tables.resident_records.get(&slot) == Some(&record) {
                            continue;
                        }
                        let destination = D3D12_CPU_DESCRIPTOR_HANDLE {
                            ptr: start + slot * self.stride,
                        };
                        if let Some(source) = self.resolve_inline(record) {
                            self.copy_native_view(destination, source, &mut retired);
                            materialized +=
                                usize::from(record.source != cell || INLINE_BYTES != self.stride);
                        } else if record == InlineDescriptor::default() {
                            // Inactive slots have no resource. Use a valid null SRV
                            // rather than copying an uninitialized host descriptor.
                            use windows::Win32::Graphics::Dxgi::Common::DXGI_FORMAT_R8G8B8A8_UNORM;
                            let null = D3D12_SHADER_RESOURCE_VIEW_DESC {
                                Format: DXGI_FORMAT_R8G8B8A8_UNORM,
                                ViewDimension: D3D12_SRV_DIMENSION_TEXTURE2D,
                                Shader4ComponentMapping: D3D12_DEFAULT_SHADER_4_COMPONENT_MAPPING,
                                ..Default::default()
                            };
                            retired.imported.extend(native.imported.clear_deferred(
                                self.stride,
                                destination.ptr,
                                1,
                            ));
                            retired.placed.extend(native.placed.clear_deferred(
                                self.stride,
                                destination.ptr,
                                1,
                            ));
                            retired.scenes.extend(native.scenes.clear_deferred(
                                self.stride,
                                destination.ptr,
                                1,
                            ));
                            crate::diagnostics::display::texture_view(destination.ptr, None, None);
                            unsafe {
                                native.device.CreateShaderResourceView(
                                    None,
                                    Some(&null),
                                    destination,
                                )
                            };
                        } else {
                            if crate::diagnostics::display::trace_views() {
                                crate::diagnostics::log_once(&std::sync::Once::new(), || format!(
                                    "unresolved inline descriptor at {cell:#x}, source {:#x}, version {} (table entry {index}/{length})",
                                    record.source, record.version,
                                ));
                            }
                            return Err(E_INVALIDARG.into());
                        }
                        tables.resident_records.insert(slot, record);
                    }
                    if materialized != 0 {
                        static FIRST: std::sync::Once = std::sync::Once::new();
                        crate::diagnostics::log_once(&FIRST, || format!(
                            "materialized {materialized} CPU-authored descriptor records (table length {length}, native stride {})",
                            self.stride
                        ));
                    }
                    Ok(())
                })();
                if let Err(error) = inline_result {
                    if fresh {
                        if let Some(entry) = tables.ranges.remove(&key) {
                            tables.by_id.remove(&entry.id);
                            let slot_start = entry.slot as usize;
                            let slot_end = slot_start + length as usize;
                            tables
                                .resident_records
                                .extract_if(slot_start..slot_end, |_, _| true)
                                .for_each(drop);
                            self.clear_host_tracking(entry.slot, length, &mut retired);
                            tables.free.free(entry.slot, length, &mut tables.used);
                        }
                    }
                    return Err(error);
                }
            }
            if let Some(id) = entry_id
                && !leases.contains(self, id)
            {
                let entry = tables
                    .ranges
                    .get_mut(&key)
                    .expect("resident table entry present for pinning");
                entry.pins = entry.pins.saturating_add(1);
                leases.insert_reserved(self, id);
            }
            Ok(self.gpu_handle(resident))
        })();
        drop(retired);
        result
    }
}
