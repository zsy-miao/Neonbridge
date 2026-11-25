//! TB_ZTRX_SHADER ABI part (c), TUXBOX_HEAP_IDENTITY (default on, =0 no
//! identity heap is made): ztrx-translated shaders read
//! `ResourceDescriptorHeap[i]` at the title's own descriptor index, with no
//! index map, so the host heap they see must keep the title's descriptor
//! order ("the host heaps keep the title's descriptor order", ztrx
//! victory-shader emit/mod.rs, GPL-3.0, credit to the ztrx and VIctory
//! authors). An unpaged heap already does (its native heap is the title's).
//! A paged arena packs tables into its shader-visible heap, so it keeps a
//! second shader-visible heap here, made on first use, whose slot i holds
//! the descriptor of title index i: written through the API (publish), or,
//! for inline records a title byte-copies, re-materialized at each submit
//! from a per-slot fingerprint (as rescan_submit does for tables, after
//! VIctory-fxceless descriptor/host.rs mirror_copied). Unwritten slots hold
//! the default null view.
// Bound by Coder5's ztrx wiring (compute_bindings / ztrx_fill).
#![allow(dead_code)]
use super::{Arena, INLINE_BYTES, InlineDescriptor, fingerprint};
use windows::Win32::Foundation::E_OUTOFMEMORY;
use windows::Win32::Graphics::Direct3D12::*;
use windows::core::Result;

pub(super) struct Identity {
    heap: ID3D12DescriptorHeap,
    /// Per title index: the fingerprint of the record it was made from (0:
    /// not made from a record).
    prints: Vec<u64>,
}

pub(crate) fn enabled() -> bool {
    static ON: std::sync::LazyLock<bool> =
        std::sync::LazyLock::new(|| crate::env_toggle!("TUXBOX_HEAP_IDENTITY") != Some("0"));
    *ON
}

impl Arena {
    /// The shader-visible heap whose slot i is title index i, made and filled
    /// on the first call. Err when off, for a sampler arena (samplers keep
    /// their own heap), or when the title's heap is larger than a
    /// shader-visible heap may be.
    pub(crate) fn identity_heap(&self) -> Result<ID3D12DescriptorHeap> {
        // For a CBV/SRV/UAV arena: the caller passes the one the bound
        // ResourceDescriptorHeap is.
        if !enabled() {
            return Err(E_OUTOFMEMORY.into());
        }
        let mut identity = self.identity.lock().unwrap_or_else(|e| e.into_inner());
        if let Some(made) = identity.as_ref() {
            return Ok(made.heap.clone());
        }
        let slots = self.identity_slots();
        // TUXBOX_ZTRX_ROOT_SRV_DIRECT unset: the root view tables' private
        // slots past the title's (root_tables), the title's cut to fit the
        // host limit.
        let tail = if crate::device::ztrx_root::root_srv_tables() {
            crate::device::root_tables::tail_for(&D3D12_DESCRIPTOR_HEAP_DESC {
                Type: D3D12_DESCRIPTOR_HEAP_TYPE_CBV_SRV_UAV,
                NumDescriptors: 0,
                Flags: D3D12_DESCRIPTOR_HEAP_FLAG_SHADER_VISIBLE,
                NodeMask: 0,
            })
        } else { 0 };
        let slots = slots.min(self.limit.saturating_sub(tail));
        let heap: ID3D12DescriptorHeap = match unsafe {
            self.device.CreateDescriptorHeap(&D3D12_DESCRIPTOR_HEAP_DESC {
                Type: D3D12_DESCRIPTOR_HEAP_TYPE_CBV_SRV_UAV,
                NumDescriptors: slots + tail,
                Flags: D3D12_DESCRIPTOR_HEAP_FLAG_SHADER_VISIBLE,
                NodeMask: 0,
            })
        } {
            Ok(heap) => heap,
            Err(e) => {
                static FAILED: std::sync::Once = std::sync::Once::new();
                FAILED.call_once(|| crate::diagnostics::log_line(format_args!(
                    "descriptor arena {:#x}: identity heap of {slots} slots (title {}, host limit {}) not made: {:?}",
                    self.base(), self.count, self.limit, e.code())));
                return Err(e);
            }
        };
        crate::device::root_tables::heap_created(&heap, slots, tail);
        let mut made = Identity { heap: heap.clone(), prints: vec![0; slots as usize] };
        self.identity_fill(&mut made, 0..slots, true);
        *identity = Some(made);
        let n = MADE.fetch_add(1, std::sync::atomic::Ordering::Relaxed) + 1;
        if n <= 4 || n.is_power_of_two() {
            crate::diagnostics::log_line(format_args!(
                "descriptor arena {:#x}: title-order identity heap of {} slots made for ztrx shaders ({n} so far; TUXBOX_HEAP_IDENTITY=0 off)",
                self.base(), slots));
        }
        Ok(heap)
    }

    /// TUXBOX_IDENTITY_CAP (default on, =0 asks for the title's whole count):
    /// the identity heap holds at most the host's shader-visible limit, the
    /// most a shader-visible heap may be; a paged arena is past it, so a
    /// heap of the title's count was refused (test186: no identity heap, the
    /// ztrx draw dropped). Title indices at or past the limit have no slot.
    fn identity_slots(&self) -> u32 {
        static ON: std::sync::LazyLock<bool> =
            std::sync::LazyLock::new(|| crate::env_toggle!("TUXBOX_IDENTITY_CAP") != Some("0"));
        if *ON { self.count.min(self.limit) } else { self.count }
    }

    /// A title descriptor write through the API reached `range`.
    pub(super) fn identity_publish(&self, range: std::ops::Range<u32>) {
        let mut identity = self.identity.lock().unwrap_or_else(|e| e.into_inner());
        if let Some(made) = identity.as_mut() {
            self.identity_fill(made, range, true);
        }
    }

    /// Before a submit: inline records the title byte-copied since.
    pub(super) fn identity_rescan(&self) {
        if !self.inline {
            return;
        }
        let mut identity = self.identity.lock().unwrap_or_else(|e| e.into_inner());
        if let Some(made) = identity.as_mut() {
            self.identity_fill(made, 0..self.count, false);
        }
    }

    /// Makes slots `range` of the identity heap from the title's slots.
    /// `force`: remake even when the record's fingerprint is unchanged.
    fn identity_fill(&self, made: &mut Identity, range: std::ops::Range<u32>, force: bool) {
        let start = unsafe { made.heap.GetCPUDescriptorHandleForHeapStart() };
        let slots = made.prints.len() as u32;
        for i in range.start.min(slots)..range.end.min(slots) {
            let to = D3D12_CPU_DESCRIPTOR_HANDLE { ptr: start.ptr + i as usize * self.stride };
            if !self.inline {
                let cell = self.base() + i as usize * self.stride;
                if force {
                    self.copy_native_view(to, self.cpu(cell));
                    self.queue_texture(to.ptr);
                }
                continue;
            }
            // Inline records sit a console descriptor (32 bytes) apart by
            // title index, as gpu_table_inner reads them.
            let cell = self.base() + i as usize * INLINE_BYTES;
            if cell + INLINE_BYTES > self.base() + self.count as usize * self.stride {
                break;
            }
            let record = self.inline_record(cell);
            let print = if record == InlineDescriptor::default() { 0 } else { fingerprint(&record) };
            if !force && made.prints[i as usize] == print {
                continue;
            }
            made.prints[i as usize] = print;
            match self.resolve_inline(record) {
                Some(source) => {
                    self.copy_native_view(to, source);
                    self.queue_texture(to.ptr);
                }
                None => {
                    if super::texture_queue::enabled() {
                        super::texture_queue::removed(to.ptr);
                    }
                    self.imported.copy_to(self.stride, to.ptr, 0, 1);
                    self.placed.copy_to(self.stride, to.ptr, 0, 1);
                    self.scenes.copy_to(self.stride, to.ptr, 0, 1);
                    crate::diagnostics::display::texture_view(to.ptr, None, None);
                    crate::device::view_formats::write(to.ptr, 0);
                    crate::device::null_kinds::NullKind::DEFAULT.write(&self.device, to);
                }
            }
        }
    }
}

impl Arena {
    /// TUXBOX_HEAP_TEXTURE_QUEUE's remap hook (fx texture_slots_remapped):
    /// the identity slots whose placed texture lies in memory mapped or
    /// unmapped since the last call are queued again, so a texture streamed
    /// into place with no descriptor write is uploaded.
    pub(crate) fn identity_remapped(&self) {
        if !super::texture_queue::enabled() {
            return;
        }
        let ranges = super::texture_queue::take_remapped();
        if ranges.is_empty() {
            return;
        }
        let identity = self.identity.lock().unwrap_or_else(|e| e.into_inner());
        let Some(made) = identity.as_ref() else { return };
        let start = unsafe { made.heap.GetCPUDescriptorHandleForHeapStart() }.ptr;
        let handles = start..start + made.prints.len() * self.stride;
        let mut slots: Vec<usize> = ranges.iter()
            .flat_map(|&(lo, hi)| self.placed.handles_over(handles.clone(), lo, hi)).collect();
        drop(identity);
        slots.sort_unstable();
        slots.dedup();
        for handle in slots {
            super::texture_queue::written(handle);
        }
    }

    /// TUXBOX_HEAP_TEXTURE_QUEUE: an identity slot now showing a placed
    /// texture is due for upload; one showing anything else leaves the queue.
    fn queue_texture(&self, handle: usize) {
        if !super::texture_queue::enabled() {
            return;
        }
        if self.placed.has(handle) {
            super::texture_queue::written(handle);
        } else {
            super::texture_queue::removed(handle);
        }
    }
}

static MADE: std::sync::atomic::AtomicU64 = std::sync::atomic::AtomicU64::new(0);

impl super::Registry {
    /// For the title CBV/SRV/UAV heap whose descriptors start at
    /// `title_base` (its CPU start): the heap a ztrx shader's
    /// ResourceDescriptorHeap must be. None: not a paged arena, so its native
    /// heap is already in title order (bind that one).
    pub(crate) fn identity_heap_for(&self, title_base: usize) -> Option<ID3D12DescriptorHeap> {
        self.find(title_base)?.identity_heap().ok()
    }
}
