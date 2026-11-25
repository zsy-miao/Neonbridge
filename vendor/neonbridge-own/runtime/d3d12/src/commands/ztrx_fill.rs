//! TB_ZTRX_SHADER d3d12 side: the user-data root constants of a ztrx-form
//! program, filled at each draw and dispatch from the root arguments the
//! title bound, and the describe line's `u<i>=` words.
//!
//! Copied from ztrx victory-gpu draw.rs:922-946 (the `stage.user_data` loop:
//! values, a fresh table's offset over the title's, SetGraphics/Compute
//! Root32BitConstants on the stage's parameter) and draw.rs:1762-1773 (the
//! describe words; GPL-3.0, credit to its authors). The loop itself is Coder5's
//! `ztrx_root::user_values`; this is the seam to our recording:
//! - A title table's value is its descriptor's byte offset in the title heap
//!   at 32 bytes a descriptor (index × 32), the index taken from the logical
//!   handle the title bound (paged arena base or the heap's GPU start, over
//!   the heap's host stride).
//! - `made`: tables the draw built itself (ztrx `tables()`), whose offsets
//!   override the title's, as draw.rs:932-936 does.
//! - A sampler table's value is its index × 16 in the title's sampler heap
//!   (Coder4's sidecar note); the heap is indexed directly in title order.
//! - The root CBV/SRV/UAVs follow the user-data parameters, by (slot, kind)
//!   as Coder5's root_signature_for lays them out; each is the view the
//!   title bound on that slot (draw.rs:947-990).
//! - `ztrx_bind` is called at each draw and dispatch of a pipeline whose
//!   stages carry a sidecar (Coder5's is_ztrx flag).
use super::{List, Recording};
use std::sync::Arc;
use windows::Win32::Graphics::Direct3D12::ID3D12RootSignature;
use crate::device::ztrx_root::{RootType, TitleRoot, UserData, user_values};
use windows::Win32::Graphics::Direct3D12::ID3D12GraphicsCommandList;

/// victory-shader emit/shader.rs USER_VIEW_BIAS: the bytes a raw view begins
/// before the title's address.
const USER_VIEW_BIAS: u32 = 0x103;

fn view_bias() -> bool {
    static ON: std::sync::OnceLock<bool> = std::sync::OnceLock::new();
    *ON.get_or_init(|| crate::env_toggle!("TUXBOX_ZTRX_VIEW_BIAS") != Some("0"))
}

fn identity_sampled() -> bool {
    static ON: std::sync::OnceLock<bool> = std::sync::OnceLock::new();
    *ON.get_or_init(|| crate::env_toggle!("TUXBOX_IDENTITY_SAMPLED") != Some("0"))
}

impl Recording {
    /// The title heap byte offset of a logical table handle (index × 32, or
    /// × 16 in the sampler heap), if it is in a bound heap.
    fn ztrx_table_index(&self, logical: u64) -> Option<u32> {
        if let Some((start, stride)) = self.sampler_heap
            && let Some(at) = logical.checked_sub(start)
            && self.resource_heap.as_ref().is_none_or(|h| logical < h.gpu_start() || start > h.gpu_start())
            && self.descriptor_arena.as_ref().is_none_or(|a| !a.contains(logical))
            && at / stride.max(1) < 2048
        {
            // A sampler table: 16 bytes a descriptor.
            return u32::try_from(at / stride.max(1)).ok().and_then(|i| i.checked_mul(16));
        }
        let heap = self.resource_heap.as_ref()?;
        let stride = heap.table_stride().max(1) as u64;
        let base = match &self.descriptor_arena {
            Some(arena) if arena.contains(logical) => arena.base() as u64,
            _ => heap.gpu_start(),
        };
        u32::try_from(logical.checked_sub(base)? / stride).ok()?.checked_mul(32)
    }

    /// The user-data values of one stage (ztrx draw.rs:922-937).
    pub(super) fn ztrx_user_values(
        &self,
        compute: bool,
        dwords: u32,
        user_data: &[UserData],
        made: &[(u8, u32)],
        host_word: impl Fn(u32, u8, u32) -> u32,
    ) -> Vec<u32> {
        let constants = |slot: u8| self.bindings.root_constants(compute, u32::from(slot));
        let owned: Vec<(u8, Vec<u32>)> = user_data
            .iter()
            .filter(|u| u.root == RootType::Constants)
            .map(|u| u.slot)
            .collect::<std::collections::BTreeSet<_>>()
            .into_iter()
            .filter_map(|slot| Some((slot, constants(slot)?)))
            .collect();
        let mut values = user_values(
            dwords,
            user_data,
            |slot| {
                if let Some((_, words)) = owned.iter().find(|(s, _)| *s == slot) {
                    return Some(TitleRoot::Constants(words));
                }
                let logical = self.bindings.root_table_logical(compute, u32::from(slot))?;
                Some(TitleRoot::Table(self.ztrx_table_index(logical)?))
            },
            host_word,
        );
        // A table the draw made itself: its offset, not the title's.
        for u in user_data.iter().filter(|u| u.root == RootType::Table) {
            if let (Some(&(_, offset)), Some(v)) = (made.iter().find(|m| m.0 == u.slot), values.get_mut(u.index as usize)) {
                *v = offset;
            }
        }
        values
    }

    /// Sets a stage's user data on its root parameter (ztrx draw.rs:938-945).
    pub(super) fn ztrx_set_user_data(
        &self,
        native: &ID3D12GraphicsCommandList,
        compute: bool,
        param: u32,
        values: &[u32],
    ) {
        if values.is_empty() {
            return;
        }
        // SAFETY: an open list; `values` covers the parameter.
        unsafe {
            if compute {
                native.SetComputeRoot32BitConstants(param, values.len() as u32, values.as_ptr().cast(), 0);
            } else {
                native.SetGraphicsRoot32BitConstants(param, values.len() as u32, values.as_ptr().cast(), 0);
            }
        }
    }

    /// A ztrx pipeline's root arguments at a draw or dispatch: stage k's user
    /// data on parameter k, then the root views by (slot, kind).
    pub(super) fn ztrx_bind(
        &mut self,
        native: &ID3D12GraphicsCommandList,
        compute: bool,
        stages: &[&crate::device::ztrx_root::ZtrxStage],
        made: &[(u8, u32)],
        host_word: impl Fn(&Self, u32, u8, u32) -> u32,
        view_heap: Option<&windows::Win32::Graphics::Direct3D12::ID3D12DescriptorHeap>,
    ) {
        // The parameters as the root was laid out (ztrx_root::layout,
        // TUXBOX_ZTRX_ROOT_FX).
        let layout = crate::device::ztrx_root::layout(stages);
        for (k, stage) in stages.iter().enumerate() {
            let Some(param) = layout.user[k] else { continue };
            let values = self.ztrx_user_values(compute, stage.user_data_dwords().max(1), &stage.user_data, made, |k, s, d| host_word(self, k, s, d));
            self.ztrx_set_user_data(native, compute, param, &values);
        }
        // Gap 15 (TUXBOX_ZTRX_VIEW_BIAS, default on, =0 binds the title's
        // address): a raw root SRV/UAV whose shader asks USER_VIEW_BIAS is
        // bound from the 16 bytes below and the shader adds the rest, as
        // VIctory-fxceless victory-gpu draw.rs:1090-1110,1240-1250,1316-1322
        // (GPL-3.0, credit to its authors). An unaligned root UAV read
        // nothing (root_tables.rs) before.
        let bias_on = view_bias();
        for (_, slot, kind, param) in layout.views {
            let biased = bias_on && kind != 0 && stages.iter().any(|s| {
                s.user_data.iter().any(|u| u.slot == slot && u.root == RootType::Other(USER_VIEW_BIAS))
            });
            let table = (crate::device::ztrx_root::root_srv_tables() && kind != 0)
                .then(|| view_heap.map(|h| (h, kind == 2))).flatten();
            if !unsafe { self.bindings.ztrx_view(native, compute, u32::from(slot), param, biased, table) } {
                static ONCE: std::sync::Once = std::sync::Once::new();
                crate::diagnostics::log_once(&ONCE, || format!(
                    "TB_ZTRX_SHADER: a shader reads root view {slot} the title did not set"));
            }
        }
    }

    /// The describe words of one stage (ztrx draw.rs:1762-1773):
    /// ` u<i>=<value>`, and the heap slot a table value points at.
    pub(super) fn ztrx_describe(&self, compute: bool, dwords: u32, user_data: &[UserData]) -> String {
        use std::fmt::Write;
        let values = self.ztrx_user_values(compute, dwords, user_data, &[], |_, _, _| 0);
        let mut text = String::new();
        for u in user_data {
            let v = values.get(u.index as usize).copied().unwrap_or(0);
            let _ = write!(text, " u{}={v:#x}", u.index);
            if u.root == RootType::Table {
                let _ = write!(text, " [heap byte {v:#x}]");
            }
        }
        text
    }

    /// A draw or dispatch of a TB_ZTRX_SHADER pipeline: its root signature,
    /// its root arguments from the title's (ztrx_bind), the work, then the
    /// title's root signature and arguments again for the draws after it.
    /// Whether the work was recorded.
    /// - On a paged descriptor arena the draw binds the arena's identity heap
    ///   (Coder5's TUXBOX_HEAP_IDENTITY, dd52389: slot i is title index i) as
    ///   its resource heap, and the title's heaps again after; an unpaged heap
    ///   is already in title order. No identity heap: the draw is dropped.
    /// - No fresh CBV-table copy yet (draw.rs:1856-2090 `tables()`): `made`
    ///   is empty, so tables read the title's.
    pub(super) fn ztrx_record(
        &mut self,
        list: &List,
        compute: bool,
        (stages, root): (Arc<[Arc<crate::device::ztrx_root::ZtrxStage>]>, ID3D12RootSignature),
        work: impl FnOnce(),
    ) -> bool {
        let native = &list.native;
        let mut view_heap = None;
        let swapped = match &self.descriptor_arena {
            None => false,
            Some(arena) => match arena.identity_heap() {
                Ok(identity) => {
                    view_heap = Some(identity.clone());
                    // TUXBOX_IDENTITY_SAMPLED (default on, =0 off): the
                    // identity heap is what a ztrx shader samples through, so
                    // its views count as sampled for the submit's placed
                    // texture uploads, as the title heap's do for other
                    // draws (sample_heap). Only the title heap's range was
                    // sampled, so a placed texture a ztrx draw read there was
                    // never uploaded and sampled black (test189: grey loading
                    // screen with black tiles; "levels not uploaded").
                    if identity_sampled()
                        && let Some(heap) = &self.resource_heap
                    {
                        let start = unsafe { identity.GetCPUDescriptorHandleForHeapStart() }.ptr;
                        let slots = unsafe { identity.GetDesc() }.NumDescriptors as usize;
                        let stride = heap.table_stride();
                        if crate::device::descriptor_paging::texture_queue::enabled() {
                            // Only the slots written since, or due a look
                            // (fxceless heap_textures).
                            let frame = crate::device::placement::UPLOAD_FRAME.load(std::sync::atomic::Ordering::Relaxed);
                            arena.identity_remapped();
                            let due = crate::device::descriptor_paging::texture_queue::due(frame);
                            // Once a list a frame: a later draw finds them in.
                            if !due.first().is_some_and(|h| self.sampled_heaps.iter().any(|r| r.contains(h))) {
                                for handle in due {
                                    super::descriptors::include_range(&mut self.sampled_heaps, handle..handle + stride);
                                }
                            }
                        } else {
                            super::descriptors::include_range(&mut self.sampled_heaps, start..start + slots * stride);
                        }
                        static ONCE: std::sync::Once = std::sync::Once::new();
                        crate::diagnostics::log_once(&ONCE, || format!(
                            "TB_ZTRX_SHADER: the identity heap's {slots} views count as sampled for placed texture uploads (TUXBOX_IDENTITY_SAMPLED=0 off)"));
                    }
                    let mut heaps: Vec<Option<windows::Win32::Graphics::Direct3D12::ID3D12DescriptorHeap>> = vec![Some(identity)];
                    heaps.extend(self.bindings.bound_heaps().iter().filter(|h| {
                        h.as_ref().is_some_and(|h| unsafe { h.GetDesc() }.Type == windows::Win32::Graphics::Direct3D12::D3D12_DESCRIPTOR_HEAP_TYPE_SAMPLER)
                    }).cloned());
                    unsafe { native.SetDescriptorHeaps(&heaps) };
                    true
                }
                Err(e) => {
                    crate::diagnostics::frame_work::add(crate::diagnostics::frame_work::Work::DroppedZtrxHeap);
                    static PAGED: std::sync::atomic::AtomicU64 = std::sync::atomic::AtomicU64::new(0);
                    let n = PAGED.fetch_add(1, std::sync::atomic::Ordering::Relaxed) + 1;
                    if n <= 4 || n.is_power_of_two() {
                        crate::diagnostics::log_line(format_args!(
                            "TB_ZTRX_SHADER: a ztrx {} on a paged descriptor heap with no identity heap ({:?}) is dropped ({n} so far)",
                            if compute { "dispatch" } else { "draw" }, e.code()));
                    }
                    return false;
                }
            },
        };
        unsafe {
            if compute {
                native.SetComputeRootSignature(&root);
            } else {
                native.SetGraphicsRootSignature(&root);
            }
        }
        let stages: Vec<&crate::device::ztrx_root::ZtrxStage> = stages.iter().map(|s| &**s).collect();
        let bias_on = view_bias();
        // An unpaged heap is the view heap the draw keeps (its root tail).
        let view_heap = view_heap.or_else(|| self.bindings.bound_heaps().iter().flatten()
            .find(|h| unsafe { h.GetDesc() }.Type == windows::Win32::Graphics::Direct3D12::D3D12_DESCRIPTOR_HEAP_TYPE_CBV_SRV_UAV).cloned());
        self.ztrx_bind(native, compute, &stages, &[], |this: &Self, kind, slot, dword| {
            if kind == USER_VIEW_BIAS {
                return if bias_on { this.bindings.ztrx_bias(compute, u32::from(slot)) } else { 0 };
            }
            static OTHER: std::sync::Once = std::sync::Once::new();
            crate::diagnostics::log_once(&OTHER, || format!(
                "TB_ZTRX_SHADER: host word kind {kind} (root {slot} dword {dword}) is not filled yet: 0"));
            0
        }, view_heap.as_ref());
        if compute {
            self.sample_tables::<true>();
        } else {
            self.sample_tables::<false>();
        }
        work();
        if swapped {
            unsafe { native.SetDescriptorHeaps(self.bindings.bound_heaps()) };
        }
        unsafe { self.bindings.ztrx_restore(native, compute) };
        true
    }
}
