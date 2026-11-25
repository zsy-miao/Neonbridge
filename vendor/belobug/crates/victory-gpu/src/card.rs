//! Large title buffers resident in device-local VRAM (`D3D12_HEAP_TYPE_DEFAULT`)
//! with GPU-side Wave64 64 KiB page hashing (`CARD_COMPARE`) for copy-back.
//!
//! Instead of keeping large buffers (4–75+ MiB) in system RAM (`L0`) over PCIe
//! and taking multi-megabyte CPU snapshots + CPU diffs every submission:
//! - Shaders read and write `Shadow::buffer` directly in GDDR6 VRAM.
//! - `CardState::staging` (`L0` WRITE_BACK, mapped as `Shadow::data`) receives
//!   CPU title-memory updates and simultaneously serves as the exact
//!   pre-submission snapshot (zero snapshot `memcpy` in `Shadow::writing`!).
//! - Before a submission (`card_before`), only CPU-modified ranges are uploaded
//!   via `CopyBufferRegion` and re-hashed on the GPU (if the buffer has hashes).
//! - At the end of a submission (`card_after`), `CARD_COMPARE` dispatches one
//!   64-thread workgroup per declared 64 KiB page in VRAM (64 coalesced
//!   1024-byte bursts per page, 64x less sequential latency than 1 thread/page),
//!   writing a 4-byte dirty flag per page to a small readback buffer.
//! - After the fence (`card_back`), ONLY pages flagged dirty are copied from
//!   VRAM to a readback buffer and merged into title memory + `staging`.

use std::sync::atomic::{AtomicBool, AtomicU64, Ordering};
use std::sync::{Arc, Mutex, Weak};

use windows::Win32::Graphics::Direct3D::Fxc::D3DCompile;
use windows::Win32::Graphics::Direct3D::ID3DBlob;
use windows::Win32::Graphics::Direct3D12::*;
use windows::core::Interface;

use crate::host::Host;
use crate::watch::PAGE;

/// Wave64 / 64-thread-per-page 64 KiB hashing compute shader (`cs_5_1`).
/// One workgroup (`SV_GroupID.x`) hashes one 64 KiB buffer-relative piece using
/// 64 threads reading coalesced 1024-byte strides (64 iterations per page).
const CARD_COMPARE_HLSL: &str = r#"
#define RS "RootConstants(num32BitConstants=6, b0), SRV(t0), UAV(u0), UAV(u1)"
cbuffer Span : register(b0) {
    uint first;    // first 64 KiB piece index
    uint pad0;     // reserved (0)
    uint size;     // buffer size in bytes (multiple of 65536)
    uint pieces;   // number of pieces in this dispatch
    uint out_at;   // byte offset in `differs` for gid.x == 0
    uint compare;  // 0: store hashes; 1: compare only; 2: compare and store new hash
};
ByteAddressBuffer now : register(t0);
RWByteAddressBuffer hashes : register(u0);
RWByteAddressBuffer differs : register(u1);

groupshared uint4 s_acc[64];

[RootSignature(RS)]
[numthreads(64, 1, 1)]
void main(uint3 gid : SV_GroupID, uint tid : SV_GroupIndex) {
    if (gid.x >= pieces) return;
    uint piece = first + gid.x;
    uint lo = piece * 65536u;
    uint hi = min(lo + 65536u, size);

    uint4 h = uint4(
        0x9E3779B9u ^ (tid * 0x85EBCA6Bu),
        0x85EBCA6Bu ^ (tid * 0xC2B2AE35u),
        0xC2B2AE35u ^ (tid * 0x27D4EB2Fu),
        0x27D4EB2Fu ^ (tid * 0x165667B1u)
    );
    [loop]
    for (uint at = lo + tid * 16u; at < hi; at += 1024u) {
        h = (h ^ now.Load4(at)) * uint4(0x01000193u, 0x9E3779B1u, 0x85EBCA77u, 0xC2B2AE3Du);
        h = h.yzwx ^ (h >> 15);
    }
    s_acc[tid] = h;
    GroupMemoryBarrierWithGroupSync();

    [unroll]
    for (uint stride = 32u; stride > 0u; stride >>= 1u) {
        if (tid < stride) {
            uint4 a = s_acc[tid];
            uint4 b = s_acc[tid + stride];
            uint4 m = (a ^ (b * uint4(0x27D4EB2Fu, 0x165667B1u, 0x9E3779B9u, 0x85EBCA6Bu)))
                    * uint4(0x01000193u, 0x9E3779B1u, 0x85EBCA77u, 0xC2B2AE3Du);
            s_acc[tid] = m.yzwx ^ (m >> 15);
        }
        GroupMemoryBarrierWithGroupSync();
    }

    if (tid == 0u) {
        uint4 final_h = s_acc[0];
        if (compare != 0u) {
            uint4 prev = hashes.Load4(piece * 16u);
            differs.Store(out_at + gid.x * 4u, any(final_h != prev) ? 1u : 0u);
            if (compare == 2u) {
                hashes.Store4(piece * 16u, final_h);
            }
        } else {
            hashes.Store4(piece * 16u, final_h);
        }
    }
}
"#;

/// Bytes currently allocated in VRAM for card buffers (`[0]`) and their hash
/// tables (`[1]`).
static CARD_BYTES: [AtomicU64; 2] = [const { AtomicU64::new(0) }; 2];

/// Card buffers with pending CPU→VRAM uploads (`dirty_up`).
static DIRTY_CARDS: Mutex<Vec<Weak<CardState>>> = Mutex::new(Vec::new());

/// Whether VRAM card buffers are enabled (`VICTORY_CARD_BUFFERS=0` disables).
pub(crate) fn enabled() -> bool {
    if cfg!(test) {
        return false;
    }
    static ON: std::sync::OnceLock<bool> = std::sync::OnceLock::new();
    *ON.get_or_init(|| std::env::var_os("VICTORY_CARD_BUFFERS").is_none_or(|v| v != "0"))
}

/// `(min_bytes, max_total_bytes)` for card buffers:
/// `VICTORY_CARD_BUFFERS_MIN_MB` (default 4 MiB) and
/// `VICTORY_CARD_BUFFERS_MB` (default 2048 MiB).
fn limits() -> (u64, u64) {
    static LIMITS: std::sync::OnceLock<(u64, u64)> = std::sync::OnceLock::new();
    *LIMITS.get_or_init(|| {
        let mb = |name: &str, default: u64| {
            std::env::var(name)
                .ok()
                .and_then(|v| v.trim().parse::<u64>().ok())
                .unwrap_or(default)
                << 20
        };
        (
            mb("VICTORY_CARD_BUFFERS_MIN_MB", 4).max(1 << 20),
            mb("VICTORY_CARD_BUFFERS_MB", 2048),
        )
    })
}

pub(crate) fn hash_table_bytes(size: u64) -> u64 {
    ((size / PAGE as u64 + 1) * 16).next_multiple_of(256)
}

/// Per-`Shadow` state when a buffer lives in device-local VRAM (`DEFAULT`).
pub(crate) struct CardState {
    /// CPU-mapped staging mirror (`L0` WRITE_BACK; `Shadow::data` points here).
    pub(crate) staging: ID3D12Resource,
    /// Device-local VRAM buffer (`DEFAULT`, `ALLOW_UNORDERED_ACCESS`; same COM
    /// resource as `Shadow::buffer`).
    pub(crate) vram: ID3D12Resource,
    /// Buffer width in bytes (multiple of `PAGE`).
    pub(crate) size: u64,
    /// Lazily created `DEFAULT` UAV buffer holding 16-byte hashes per 64 KiB piece.
    hashes: Mutex<Option<ID3D12Resource>>,
    /// True once `hashes` exists and has been populated for the whole buffer.
    hashes_ready: AtomicBool,
    /// Buffer-relative byte ranges `[from, to)` written to `staging` by the CPU
    /// that must be copied to `vram` (and re-hashed if `hashes_ready`) before
    /// the next GPU submission.
    dirty_up: Mutex<Vec<(usize, usize)>>,
    /// Deduplication flag for [`DIRTY_CARDS`].
    queued: AtomicBool,
}

// SAFETY: D3D12 resources are free-threaded; mutable fields are synchronized.
unsafe impl Send for CardState {}
// SAFETY: as above.
unsafe impl Sync for CardState {}

impl Drop for CardState {
    fn drop(&mut self) {
        CARD_BYTES[0].fetch_sub(self.size, Ordering::Relaxed);
        let hashes = self.hashes.get_mut().ok().and_then(Option::take);
        if hashes.is_some() {
            CARD_BYTES[1].fetch_sub(hash_table_bytes(self.size), Ordering::Relaxed);
        }
        crate::host::retire((self.staging.clone(), self.vram.clone(), hashes));
    }
}

impl CardState {
    /// Allocates a VRAM card buffer for `size` bytes (must be a multiple of
    /// `PAGE`) if enabled and within VRAM budget.
    pub(crate) fn try_new(host: &Host, staging: &ID3D12Resource, size: u64) -> Option<Arc<Self>> {
        if !enabled() {
            return None;
        }
        let (min_bytes, max_bytes) = limits();
        if size < min_bytes || size >= (1 << 30) || size % PAGE as u64 != 0 {
            return None;
        }
        if !host.card_ready() {
            return None;
        }
        if let Some([(used, budget), _]) = crate::host::video_memory()
            && budget > 0
            && used.saturating_add(size) > budget * 75 / 100
        {
            return None;
        }
        if CARD_BYTES[0].fetch_add(size, Ordering::Relaxed) + size > max_bytes {
            CARD_BYTES[0].fetch_sub(size, Ordering::Relaxed);
            return None;
        }
        Self::new_unchecked(host, staging, size, true)
    }

    /// Creates a `CardState` directly (used by `try_new` and WARP unit tests).
    pub(crate) fn new_unchecked(
        host: &Host,
        staging: &ID3D12Resource,
        size: u64,
        counted: bool,
    ) -> Option<Arc<Self>> {
        let Some(vram) = host.uav_buffer(size) else {
            if counted {
                CARD_BYTES[0].fetch_sub(size, Ordering::Relaxed);
            }
            return None;
        };
        if !counted {
            CARD_BYTES[0].fetch_add(size, Ordering::Relaxed);
        }
        Some(Arc::new(Self {
            staging: staging.clone(),
            vram,
            size,
            hashes: Mutex::new(None),
            hashes_ready: AtomicBool::new(false),
            dirty_up: Mutex::new(Vec::new()),
            queued: AtomicBool::new(false),
        }))
    }

    /// Notes that CPU copied new bytes into `staging[from..to]`.
    pub(crate) fn mark_dirty(self: &Arc<Self>, from: usize, to: usize) {
        if from >= to {
            return;
        }
        {
            let mut dirty = self.dirty_up.lock().unwrap_or_else(|e| e.into_inner());
            dirty.push((from, to));
        }
        if !self.queued.swap(true, Ordering::AcqRel) {
            DIRTY_CARDS
                .lock()
                .unwrap_or_else(|e| e.into_inner())
                .push(Arc::downgrade(self));
        }
    }

    /// Ensures the GPU hash table exists; returns `(hashes_resource, needs_full_init)`.
    fn ensure_hashes(&self, host: &Host) -> Option<(ID3D12Resource, bool)> {
        let mut h = self.hashes.lock().unwrap_or_else(|e| e.into_inner());
        if let Some(res) = h.as_ref() {
            return Some((res.clone(), !self.hashes_ready.load(Ordering::Acquire)));
        }
        let bytes = hash_table_bytes(self.size);
        let res = host.uav_buffer(bytes)?;
        crate::host::name(&res, || format!("card hashes {bytes:#x}"));
        CARD_BYTES[1].fetch_add(bytes, Ordering::Relaxed);
        *h = Some(res.clone());
        Some((res, true))
    }

    fn take_dirty_up(&self) -> Vec<(usize, usize)> {
        self.queued.store(false, Ordering::Release);
        let mut dirty = self.dirty_up.lock().unwrap_or_else(|e| e.into_inner());
        if dirty.is_empty() {
            return Vec::new();
        }
        let mut ranges = std::mem::take(&mut *dirty);
        merge_byte_ranges(&mut ranges);
        ranges
    }

    pub(crate) fn total_pieces(&self) -> u32 {
        (self.size / PAGE as u64) as u32
    }
}

/// Merges overlapping or adjacent `[from, to)` ranges in place.
fn merge_byte_ranges(ranges: &mut Vec<(usize, usize)>) {
    if ranges.len() <= 1 {
        return;
    }
    ranges.sort_unstable();
    let mut out = 0usize;
    for i in 1..ranges.len() {
        let (a, b) = ranges[i];
        if a <= ranges[out].1 {
            ranges[out].1 = ranges[out].1.max(b);
        } else {
            out += 1;
            ranges[out] = (a, b);
        }
    }
    ranges.truncate(out + 1);
}

/// Converts byte ranges `[from, to)` into sorted, merged 64 KiB piece runs `(first_piece, count)`.
fn byte_ranges_to_piece_runs(ranges: &[(usize, usize)], max_pieces: u32) -> Vec<(u32, u32)> {
    if ranges.is_empty() || max_pieces == 0 {
        return Vec::new();
    }
    let mut spans: Vec<(u32, u32)> = Vec::with_capacity(ranges.len());
    for &(from, to) in ranges {
        if from >= to {
            continue;
        }
        let first = (from / PAGE) as u32;
        let end = (to.div_ceil(PAGE) as u32).min(max_pieces);
        if first < end {
            spans.push((first, end));
        }
    }
    if spans.is_empty() {
        return Vec::new();
    }
    spans.sort_unstable();
    let mut out: Vec<(u32, u32)> = Vec::with_capacity(spans.len());
    for (a, b) in spans {
        if let Some(last) = out.last_mut()
            && a <= last.0 + last.1
        {
            let end = (last.0 + last.1).max(b);
            last.1 = end - last.0;
        } else {
            out.push((a, b - a));
        }
    }
    out
}

/// Per-submission plan built in `card_before` and consumed in `card_after` + `card_back`.
pub(crate) struct CardPlan {
    entries: Vec<CardWriteEntry>,
    total_pieces: u32,
}

struct CardWriteEntry {
    shadow: Arc<crate::shadow::Shadow>,
    card: Arc<CardState>,
    hashes: ID3D12Resource,
    /// Original declared title ranges `(address, len)` for clearing `pending`.
    declared: Vec<(usize, usize)>,
    /// Merged 64 KiB buffer-relative piece runs `(first_piece, count)`.
    runs: Vec<(u32, u32)>,
    /// First dword index in `differs` for this entry.
    out_start: u32,
}

impl CardPlan {
    pub(crate) fn has_writes(&self) -> bool {
        self.total_pieces > 0
    }

    pub(crate) fn clear_pending(self) {
        for entry in &self.entries {
            entry.shadow.finish_card_pending(&entry.declared);
        }
    }
}

/// Per-`Host` reusable GPU pipelines, command lists, and readback scratch buffers
/// for VRAM card buffer uploads, Wave64 hashing, and dirty-page fetch.
pub(crate) struct CardEngine {
    root_sig: ID3D12RootSignature,
    pso: ID3D12PipelineState,
    before_alloc: ID3D12CommandAllocator,
    before_list: ID3D12GraphicsCommandList,
    after_alloc: ID3D12CommandAllocator,
    after_list: ID3D12GraphicsCommandList,
    fetch_alloc: ID3D12CommandAllocator,
    fetch_list: ID3D12GraphicsCommandList,
    differs_gpu: ID3D12Resource,
    differs_rb: (ID3D12Resource, usize, u64),
    dummy_uav: ID3D12Resource,
    fetch_rb: Option<(ID3D12Resource, usize, u64)>,
}

// SAFETY: accessed only under `Host::submitted` lock or `CardEngine` mutex;
// D3D12 interfaces are free-threaded.
unsafe impl Send for CardEngine {}

impl CardEngine {
    pub(crate) fn new(host: &Host) -> windows::core::Result<Self> {
        let mut code: Option<ID3DBlob> = None;
        // SAFETY: valid HLSL source and entry point.
        unsafe {
            D3DCompile(
                CARD_COMPARE_HLSL.as_ptr().cast(),
                CARD_COMPARE_HLSL.len(),
                windows::core::s!("card_compare_wave64"),
                None,
                None,
                windows::core::s!("main"),
                windows::core::s!("cs_5_1"),
                0,
                0,
                &mut code,
                None,
            )?;
        }
        let code = code.ok_or_else(windows::core::Error::empty)?;
        // SAFETY: valid compiled DXBC blob from D3DCompile.
        let bytecode = unsafe {
            std::slice::from_raw_parts(code.GetBufferPointer().cast::<u8>(), code.GetBufferSize())
        };
        // SAFETY: bytecode embeds `[RootSignature(RS)]`.
        let root_sig: ID3D12RootSignature =
            unsafe { host.device.CreateRootSignature(0, bytecode)? };
        let pso_desc = D3D12_COMPUTE_PIPELINE_STATE_DESC {
            pRootSignature: std::mem::ManuallyDrop::new(Some(root_sig.clone())),
            CS: D3D12_SHADER_BYTECODE {
                pShaderBytecode: bytecode.as_ptr().cast(),
                BytecodeLength: bytecode.len(),
            },
            ..Default::default()
        };
        // SAFETY: valid compute pipeline description.
        let pso_res: windows::core::Result<ID3D12PipelineState> =
            unsafe { host.device.CreateComputePipelineState(&pso_desc) };
        // Release our extra ref from `ManuallyDrop` above even on error.
        drop(std::mem::ManuallyDrop::into_inner(pso_desc.pRootSignature));
        let pso = pso_res?;

        let make_pair = || -> windows::core::Result<(ID3D12CommandAllocator, ID3D12GraphicsCommandList)> {
            // SAFETY: plain direct command allocator and list creation.
            unsafe {
                let alloc: ID3D12CommandAllocator = host
                    .device
                    .CreateCommandAllocator(D3D12_COMMAND_LIST_TYPE_DIRECT)?;
                let list: ID3D12GraphicsCommandList = host.device.CreateCommandList(
                    0,
                    D3D12_COMMAND_LIST_TYPE_DIRECT,
                    &alloc,
                    None,
                )?;
                list.Close()?;
                Ok((alloc, list))
            }
        };
        let (before_alloc, before_list) = make_pair()?;
        let (after_alloc, after_list) = make_pair()?;
        let (fetch_alloc, fetch_list) = make_pair()?;

        let init_diff_bytes = 256 << 10; // 65,536 pieces (4 GiB of ranges)
        let differs_gpu = host
            .uav_buffer(init_diff_bytes)
            .ok_or_else(windows::core::Error::empty)?;
        let (rb_res, rb_ptr) = host
            .readback_buffer(init_diff_bytes)
            .ok_or_else(windows::core::Error::empty)?;
        let dummy_uav = host
            .uav_buffer(256)
            .ok_or_else(windows::core::Error::empty)?;
        crate::host::name(&differs_gpu, || "card differs gpu".into());
        crate::host::name(&rb_res, || "card differs readback".into());
        crate::host::name(&dummy_uav, || "card dummy uav".into());

        Ok(Self {
            root_sig,
            pso,
            before_alloc,
            before_list,
            after_alloc,
            after_list,
            fetch_alloc,
            fetch_list,
            differs_gpu,
            differs_rb: (rb_res, rb_ptr as usize, init_diff_bytes),
            dummy_uav,
            fetch_rb: None,
        })
    }

    fn ensure_differs_capacity(&mut self, host: &Host, needed_bytes: u64) -> bool {
        if needed_bytes <= self.differs_rb.2 {
            return true;
        }
        let size = needed_bytes.next_multiple_of(256 << 10);
        let Some(gpu) = host.uav_buffer(size) else {
            return false;
        };
        let Some((rb, ptr)) = host.readback_buffer(size) else {
            return false;
        };
        let old_gpu = std::mem::replace(&mut self.differs_gpu, gpu);
        let old_rb = std::mem::replace(&mut self.differs_rb, (rb, ptr as usize, size));
        crate::host::retire((old_gpu, old_rb.0));
        true
    }

    fn ensure_fetch_capacity(&mut self, host: &Host, needed_bytes: u64) -> Option<(ID3D12Resource, usize)> {
        if let Some((ref res, ptr, cap)) = self.fetch_rb
            && needed_bytes <= cap
        {
            return Some((res.clone(), ptr));
        }
        let size = needed_bytes.next_multiple_of(8 << 20);
        let (rb, ptr) = host.readback_buffer(size)?;
        crate::host::name(&rb, || format!("card fetch readback {size:#x}"));
        if let Some((old, _, _)) = self.fetch_rb.replace((rb.clone(), ptr as usize, size)) {
            crate::host::retire(old);
        }
        Some((rb, ptr as usize))
    }

    /// Dispatches `CARD_COMPARE` for `count` contiguous 64 KiB pieces starting at `first`.
    unsafe fn dispatch_hash(
        &self,
        list: &ID3D12GraphicsCommandList,
        vram: &ID3D12Resource,
        hashes: &ID3D12Resource,
        differs: &ID3D12Resource,
        first: u32,
        size: u32,
        count: u32,
        out_at_bytes: u32,
        compare: u32,
    ) {
        if count == 0 {
            return;
        }
        let constants = [first, 0u32, size, count, out_at_bytes, compare];
        // SAFETY: caller ensures `list` is open with `root_sig` and `pso` bound
        // and all 3 buffers are live with valid states.
        unsafe {
            list.SetComputeRoot32BitConstants(0, 6, constants.as_ptr().cast(), 0);
            list.SetComputeRootShaderResourceView(1, vram.GetGPUVirtualAddress());
            list.SetComputeRootUnorderedAccessView(2, hashes.GetGPUVirtualAddress());
            list.SetComputeRootUnorderedAccessView(3, differs.GetGPUVirtualAddress());
            list.Dispatch(count, 1, 1);
        }
    }

    /// Uploads all CPU-modified `Card` ranges to VRAM, initializes hashes for
    /// any `Card` shadow about to be written by shaders, and re-hashes uploaded
    /// pieces of hashed shadows. Returns `(optional before command list, CardPlan)`.
    pub(crate) fn record_before(
        &mut self,
        host: &Host,
        writes: &[(Arc<crate::shadow::Shadow>, usize, usize)],
    ) -> (Option<ID3D12CommandList>, CardPlan) {
        // 1. Drain all card shadows with pending CPU -> VRAM uploads.
        let mut dirty_cards: Vec<(Arc<CardState>, Vec<(usize, usize)>)> = Vec::new();
        {
            let mut q = DIRTY_CARDS.lock().unwrap_or_else(|e| e.into_inner());
            for weak in q.drain(..) {
                if let Some(card) = weak.upgrade() {
                    let ranges = card.take_dirty_up();
                    if !ranges.is_empty() {
                        dirty_cards.push((card, ranges));
                    }
                }
            }
        }

        // 2. Group declared `Step::GpuWrite` ranges by `Card` shadow and ensure
        //    each written card shadow has its hash table allocated.
        let mut entries: Vec<CardWriteEntry> = Vec::new();
        let mut need_full_hash: Vec<(Arc<CardState>, ID3D12Resource)> = Vec::new();
        let mut total_pieces = 0u32;

        for (shadow, address, len) in writes {
            let Some(card) = shadow.card_state() else {
                continue;
            };
            if let Some(entry) = entries.iter_mut().find(|e| Arc::ptr_eq(&e.shadow, shadow)) {
                entry.declared.push((*address, *len));
            } else if let Some((hashes, full_init)) = card.ensure_hashes(host) {
                if full_init {
                    need_full_hash.push((card.clone(), hashes.clone()));
                }
                entries.push(CardWriteEntry {
                    shadow: shadow.clone(),
                    card: card.clone(),
                    hashes,
                    declared: vec![(*address, *len)],
                    runs: Vec::new(),
                    out_start: 0,
                });
            }
        }

        for entry in &mut entries {
            let mut byte_ranges = Vec::with_capacity(entry.declared.len());
            for &(addr, len) in &entry.declared {
                let (from, to) = entry.shadow.inside_offsets(addr, len);
                if from < to {
                    byte_ranges.push((from, to));
                }
            }
            entry.runs = byte_ranges_to_piece_runs(&byte_ranges, entry.card.total_pieces());
            entry.out_start = total_pieces;
            total_pieces += entry.runs.iter().map(|&(_, c)| c).sum::<u32>();
        }

        // Also check which dirty-uploaded shadows already have hashes (so we
        // re-hash their newly uploaded pieces after CopyBufferRegion).
        let mut rehash_uploads: Vec<(Arc<CardState>, ID3D12Resource, Vec<(u32, u32)>)> = Vec::new();
        for (card, ranges) in &dirty_cards {
            // Skip partial rehash if this card is already getting a full hash below.
            if need_full_hash.iter().any(|(c, _)| Arc::ptr_eq(c, card)) {
                continue;
            }
            if card.hashes_ready.load(Ordering::Acquire)
                && let Some(hashes) = card
                    .hashes
                    .lock()
                    .unwrap_or_else(|e| e.into_inner())
                    .clone()
            {
                let runs = byte_ranges_to_piece_runs(ranges, card.total_pieces());
                if !runs.is_empty() {
                    rehash_uploads.push((card.clone(), hashes, runs));
                }
            }
        }

        let plan = CardPlan {
            entries,
            total_pieces,
        };

        if dirty_cards.is_empty() && need_full_hash.is_empty() {
            return (None, plan);
        }

        // SAFETY: allocator & list are idle under `Host::submitted`.
        let list = unsafe {
            if self.before_alloc.Reset().is_err()
                || self.before_list.Reset(&self.before_alloc, None).is_err()
            {
                return (None, plan);
            }
            &self.before_list
        };

        // Step A: Copy dirty staging ranges -> VRAM.
        for (card, ranges) in &dirty_cards {
            for &(from, to) in ranges {
                // SAFETY: both buffers cover `[from, to)`; implicit COMMON promotion.
                unsafe {
                    list.CopyBufferRegion(
                        &card.vram,
                        from as u64,
                        &card.staging,
                        from as u64,
                        (to - from) as u64,
                    );
                }
            }
        }

        // Step B: Transition uploaded VRAM buffers COPY_DEST -> NON_PIXEL_SHADER_RESOURCE
        // (or COMMON if no hashing follows).
        let mut to_srv = Vec::new();
        let mut to_common = Vec::new();
        for (card, _) in &dirty_cards {
            let will_hash = need_full_hash.iter().any(|(c, _)| Arc::ptr_eq(c, card))
                || rehash_uploads.iter().any(|(c, _, _)| Arc::ptr_eq(c, card));
            let after = if will_hash {
                D3D12_RESOURCE_STATE_NON_PIXEL_SHADER_RESOURCE
            } else {
                D3D12_RESOURCE_STATE_COMMON
            };
            to_srv.push(transition_barrier(
                &card.vram,
                D3D12_RESOURCE_STATE_COPY_DEST,
                after,
            ));
            if will_hash {
                to_common.push(transition_barrier(
                    &card.vram,
                    D3D12_RESOURCE_STATE_NON_PIXEL_SHADER_RESOURCE,
                    D3D12_RESOURCE_STATE_COMMON,
                ));
            }
        }
        if !to_srv.is_empty() {
            // SAFETY: valid resource transitions.
            unsafe { list.ResourceBarrier(&to_srv) };
            drop_barriers(to_srv);
        }

        // Step C: Dispatch Wave64 CARD_COMPARE (compare = 0) to store hashes.
        if !need_full_hash.is_empty() || !rehash_uploads.is_empty() {
            // SAFETY: live root signature and PSO.
            unsafe {
                list.SetComputeRootSignature(&self.root_sig);
                list.SetPipelineState(&self.pso);
            }
            for (card, hashes) in &need_full_hash {
                // SAFETY: buffer is readable as SRV (either transitioned above or COMMON).
                unsafe {
                    self.dispatch_hash(
                        list,
                        &card.vram,
                        hashes,
                        &self.dummy_uav,
                        0,
                        card.size as u32,
                        card.total_pieces(),
                        0,
                        0,
                    );
                }
                card.hashes_ready.store(true, Ordering::Release);
            }
            for (card, hashes, runs) in &rehash_uploads {
                for &(first, count) in runs {
                    // SAFETY: buffer is in NON_PIXEL_SHADER_RESOURCE.
                    unsafe {
                        self.dispatch_hash(
                            list,
                            &card.vram,
                            hashes,
                            &self.dummy_uav,
                            first,
                            card.size as u32,
                            count,
                            0,
                            0,
                        );
                    }
                }
            }
            // Flush hash UAV writes and return transitioned VRAM buffers to COMMON.
            to_common.push(uav_barrier());
        }

        if !to_common.is_empty() {
            // SAFETY: valid barriers.
            unsafe { list.ResourceBarrier(&to_common) };
            drop_barriers(to_common);
        }

        // SAFETY: close command list.
        if unsafe { list.Close() }.is_err() {
            return (None, plan);
        }
        (list.cast().ok(), plan)
    }

    /// Records the post-submission GPU hash compare (`compare = 2`) and copies
    /// the tiny `differs` array to `differs_rb` (runs in the same GPU wait!).
    pub(crate) fn record_after(&mut self, host: &Host, plan: &CardPlan) -> Option<ID3D12CommandList> {
        if plan.total_pieces == 0 {
            return None;
        }
        let needed_bytes = u64::from(plan.total_pieces) * 4;
        if !self.ensure_differs_capacity(host, needed_bytes) {
            return None;
        }
        // SAFETY: allocator & list are idle under `Host::submitted`.
        unsafe {
            self.after_alloc.Reset().ok()?;
            self.after_list.Reset(&self.after_alloc, None).ok()?;
            // Global UAV barrier ensures all shader writes from `runs` are visible.
            self.after_list.ResourceBarrier(&[uav_barrier()]);
            self.after_list.SetComputeRootSignature(&self.root_sig);
            self.after_list.SetPipelineState(&self.pso);

            for entry in &plan.entries {
                let mut out_idx = entry.out_start;
                for &(first, count) in &entry.runs {
                    self.dispatch_hash(
                        &self.after_list,
                        &entry.card.vram,
                        &entry.hashes,
                        &self.differs_gpu,
                        first,
                        entry.card.size as u32,
                        count,
                        out_idx * 4,
                        2, // compare against previous hash AND store updated hash
                    );
                    out_idx += count;
                }
            }

            let to_copy = [transition_barrier(
                &self.differs_gpu,
                D3D12_RESOURCE_STATE_UNORDERED_ACCESS,
                D3D12_RESOURCE_STATE_COPY_SOURCE,
            )];
            self.after_list.ResourceBarrier(&to_copy);
            drop_barriers(to_copy);

            self.after_list.CopyBufferRegion(
                &self.differs_rb.0,
                0,
                &self.differs_gpu,
                0,
                needed_bytes,
            );

            let back_common = [transition_barrier(
                &self.differs_gpu,
                D3D12_RESOURCE_STATE_COPY_SOURCE,
                D3D12_RESOURCE_STATE_COMMON,
            )];
            self.after_list.ResourceBarrier(&back_common);
            drop_barriers(back_common);

            self.after_list.Close().ok()?;
            self.after_list.cast().ok()
        }
    }

    /// Reads the `differs` results after the submission fence completes, fetches
    /// ONLY the 64 KiB pages the GPU actually modified, and merges them into
    /// title memory + `staging`. Returns total bytes written to title memory.
    pub(crate) fn finish_back(
        &mut self,
        host: &Host,
        plan: CardPlan,
        submitted: &mut u64,
    ) -> usize {
        if plan.entries.is_empty() {
            return 0;
        }
        // SAFETY: GPU finished writing `differs_rb` before the fence returned.
        let differs: &[u32] = if plan.total_pieces > 0 {
            unsafe {
                std::slice::from_raw_parts(
                    self.differs_rb.1 as *const u32,
                    plan.total_pieces as usize,
                )
            }
        } else {
            &[]
        };

        // Collect coalesced runs of dirty 64 KiB pieces per entry.
        struct DirtyRun {
            entry_idx: usize,
            first_piece: u32,
            count: u32,
            rb_offset: u64,
        }
        let mut dirty_runs: Vec<DirtyRun> = Vec::new();
        let mut total_bytes = 0u64;

        for (entry_idx, entry) in plan.entries.iter().enumerate() {
            let mut out_idx = entry.out_start as usize;
            let mut cur: Option<(u32, u32)> = None;
            for &(first, count) in &entry.runs {
                for k in 0..count {
                    let piece = first + k;
                    if differs.get(out_idx).copied().unwrap_or(0) != 0 {
                        match &mut cur {
                            Some((s, c)) if *s + *c == piece => *c += 1,
                            Some((s, c)) => {
                                let bytes = u64::from(*c) * PAGE as u64;
                                dirty_runs.push(DirtyRun {
                                    entry_idx,
                                    first_piece: *s,
                                    count: *c,
                                    rb_offset: total_bytes,
                                });
                                total_bytes += bytes;
                                cur = Some((piece, 1));
                            }
                            None => cur = Some((piece, 1)),
                        }
                    }
                    out_idx += 1;
                }
            }
            if let Some((s, c)) = cur {
                let bytes = u64::from(c) * PAGE as u64;
                dirty_runs.push(DirtyRun {
                    entry_idx,
                    first_piece: s,
                    count: c,
                    rb_offset: total_bytes,
                });
                total_bytes += bytes;
            }
        }

        let mut written = 0usize;
        if total_bytes > 0
            && let Some((fetch_res, fetch_ptr)) = self.ensure_fetch_capacity(host, total_bytes)
        {
            let ok = unsafe {
                self.fetch_alloc.Reset().is_ok()
                    && self.fetch_list.Reset(&self.fetch_alloc, None).is_ok()
            };
            if ok {
                for run in &dirty_runs {
                    let entry = &plan.entries[run.entry_idx];
                    // SAFETY: `vram` is in COMMON (promotes to COPY_SOURCE, decays to COMMON).
                    unsafe {
                        self.fetch_list.CopyBufferRegion(
                            &fetch_res,
                            run.rb_offset,
                            &entry.card.vram,
                            u64::from(run.first_piece) * PAGE as u64,
                            u64::from(run.count) * PAGE as u64,
                        );
                    }
                }
                if unsafe { self.fetch_list.Close() }.is_ok()
                    && let Ok(cmd) = self.fetch_list.cast::<ID3D12CommandList>()
                {
                    host.execute_and_wait_locked(cmd, submitted);

                    // Group dirty runs by entry so distinct shadows merge in parallel.
                    let mut per_entry: Vec<(usize, Vec<(u32, u32, usize)>)> = Vec::new();
                    for run in &dirty_runs {
                        let ptr = fetch_ptr + run.rb_offset as usize;
                        if let Some(g) = per_entry.iter_mut().find(|(i, _)| *i == run.entry_idx) {
                            g.1.push((run.first_piece, run.count, ptr));
                        } else {
                            per_entry.push((run.entry_idx, vec![(run.first_piece, run.count, ptr)]));
                        }
                    }
                    let total_written = AtomicU64::new(0);
                    crate::host::in_parallel(&per_entry, 4, |chunk| {
                        for (entry_idx, runs) in chunk {
                            let entry = &plan.entries[*entry_idx];
                            let n = entry.shadow.merge_card_pieces(&entry.card, runs);
                            total_written.fetch_add(n as u64, Ordering::Relaxed);
                        }
                    });
                    written = total_written.into_inner() as usize;
                }
            }
        }

        // Clear all declared pending ranges on each card shadow.
        for entry in &plan.entries {
            entry.shadow.finish_card_pending(&entry.declared);
        }
        written
    }
}

fn transition_barrier(
    resource: &ID3D12Resource,
    before: D3D12_RESOURCE_STATES,
    after: D3D12_RESOURCE_STATES,
) -> D3D12_RESOURCE_BARRIER {
    D3D12_RESOURCE_BARRIER {
        Type: D3D12_RESOURCE_BARRIER_TYPE_TRANSITION,
        Flags: D3D12_RESOURCE_BARRIER_FLAG_NONE,
        Anonymous: D3D12_RESOURCE_BARRIER_0 {
            Transition: std::mem::ManuallyDrop::new(D3D12_RESOURCE_TRANSITION_BARRIER {
                pResource: std::mem::ManuallyDrop::new(Some(resource.clone())),
                Subresource: D3D12_RESOURCE_BARRIER_ALL_SUBRESOURCES,
                StateBefore: before,
                StateAfter: after,
            }),
        },
    }
}

fn uav_barrier() -> D3D12_RESOURCE_BARRIER {
    D3D12_RESOURCE_BARRIER {
        Type: D3D12_RESOURCE_BARRIER_TYPE_UAV,
        Flags: D3D12_RESOURCE_BARRIER_FLAG_NONE,
        Anonymous: D3D12_RESOURCE_BARRIER_0 {
            UAV: std::mem::ManuallyDrop::new(D3D12_RESOURCE_UAV_BARRIER {
                pResource: std::mem::ManuallyDrop::new(None),
            }),
        },
    }
}

fn drop_barriers(barriers: impl IntoIterator<Item = D3D12_RESOURCE_BARRIER>) {
    for b in barriers {
        if b.Type == D3D12_RESOURCE_BARRIER_TYPE_TRANSITION {
            // SAFETY: we constructed this TRANSITION barrier with `ManuallyDrop`.
            let t = unsafe { std::mem::ManuallyDrop::into_inner(b.Anonymous.Transition) };
            let _ = std::mem::ManuallyDrop::into_inner(t.pResource);
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn range_helpers_merge_and_convert_pieces() {
        let mut r = vec![(100, 200), (50, 120), (300, 400), (200, 300)];
        merge_byte_ranges(&mut r);
        assert_eq!(r, vec![(50, 400)]);

        let runs = byte_ranges_to_piece_runs(
            &[(0, 10), (PAGE - 1, PAGE + 1), (3 * PAGE, 4 * PAGE)],
            8,
        );
        assert_eq!(runs, vec![(0, 2), (3, 1)]);
    }

    #[test]
    fn warp_card_roundtrip_upload_wave64_hash_and_concurrent_merge() {
        let Some(host) = Host::new_warp() else {
            return;
        };
        let mut engine = CardEngine::new(&host).expect("CardEngine should build on WARP");
        let pages = 4usize;
        let size = (pages * PAGE) as u64;
        let mut title = vec![0u8; size as usize];
        for (i, b) in title.iter_mut().enumerate() {
            *b = ((i.wrapping_mul(31) ^ (i >> 8)) & 0xFF) as u8;
        }
        let base = title.as_ptr() as usize;

        let (staging, staging_ptr) = host.shared_buffer(size).expect("staging buffer");
        let card = CardState::new_unchecked(&host, &staging, size, false).expect("card buffer");
        let shadow = crate::shadow::Shadow::new_card_test(
            card.clone(),
            staging_ptr,
            base as u64,
            size,
        );
        let mut submitted = 0u64;

        // Phase 1: Initial CPU copy -> upload to VRAM + Wave64 hash, no GPU writes.
        shadow.copy_for_test(base, base + size as usize);
        shadow.writing(base, size as usize);
        let (before, plan) = engine.record_before(&host, &[(shadow.clone(), base, size as usize)]);
        assert!(before.is_some(), "initial upload + hash must emit before list");
        assert_eq!(plan.total_pieces, pages as u32);
        let after = engine.record_after(&host, &plan).expect("after list");
        host.execute_and_wait_locked(before.unwrap(), &mut submitted);
        host.execute_and_wait_locked(after, &mut submitted);

        let differs_1 = unsafe {
            std::slice::from_raw_parts(engine.differs_rb.1 as *const u32, pages).to_vec()
        };
        assert_eq!(differs_1, vec![0, 0, 0, 0]);
        let written_1 = engine.finish_back(&host, plan, &mut submitted);
        assert_eq!(written_1, 0);

        // Phase 2: GPU writes 4 bytes on page 2 while CPU concurrently writes
        // 4 different bytes on the same page 2.
        let (patch_buf, patch_ptr) = host.shared_buffer(256).expect("patch buffer");
        let gpu_bytes = [0xDEu8, 0xAD, 0xBE, 0xEF];
        unsafe { std::ptr::copy_nonoverlapping(gpu_bytes.as_ptr(), patch_ptr, 4) };
        unsafe {
            let alloc: ID3D12CommandAllocator = host
                .device
                .CreateCommandAllocator(D3D12_COMMAND_LIST_TYPE_DIRECT)
                .expect("alloc");
            let list: ID3D12GraphicsCommandList = host
                .device
                .CreateCommandList(0, D3D12_COMMAND_LIST_TYPE_DIRECT, &alloc, None)
                .expect("list");
            list.CopyBufferRegion(
                &card.vram,
                (2 * PAGE + 128) as u64,
                &patch_buf,
                0,
                4,
            );
            list.Close().expect("close");
            let cmd: ID3D12CommandList = list.cast().expect("cast");
            // Execute immediately after record_before while alloc stays alive.
            shadow.writing(base, size as usize);
            let (before2, plan2) =
                engine.record_before(&host, &[(shadow.clone(), base, size as usize)]);
            assert!(before2.is_none(), "no CPU dirty uploads expected in phase 2");
            let after2 = engine.record_after(&host, &plan2).expect("after2");
            // Simulate concurrent CPU modification on page 2 while GPU runs.
            title[2 * PAGE + 256..2 * PAGE + 260].copy_from_slice(&[0x11, 0x22, 0x33, 0x44]);
            host.execute_and_wait_locked(cmd, &mut submitted);
            drop((list, alloc));
            host.execute_and_wait_locked(after2, &mut submitted);

            let differs_2 =
                std::slice::from_raw_parts(engine.differs_rb.1 as *const u32, pages).to_vec();
            assert_eq!(differs_2, vec![0, 0, 1, 0], "only page 2 should differ");
            let written_2 = engine.finish_back(&host, plan2, &mut submitted);
            assert_eq!(written_2, 4, "tight dirty span should write exactly 4 GPU bytes");
            assert_eq!(&title[2 * PAGE + 128..2 * PAGE + 132], &gpu_bytes);
            assert_eq!(&title[2 * PAGE + 256..2 * PAGE + 260], &[0x11, 0x22, 0x33, 0x44]);

            // Phase 3: Next submission uploads merged concurrent CPU write on page 2
            // back to VRAM and re-hashes page 2 so subsequent compare reports 0 diffs.
            shadow.writing(base, size as usize);
            let (before3, plan3) =
                engine.record_before(&host, &[(shadow.clone(), base, size as usize)]);
            assert!(
                before3.is_some(),
                "concurrent CPU write must queue page 2 for re-upload and re-hash"
            );
            let after3 = engine.record_after(&host, &plan3).expect("after3");
            host.execute_and_wait_locked(before3.unwrap(), &mut submitted);
            host.execute_and_wait_locked(after3, &mut submitted);
            let written_3 = engine.finish_back(&host, plan3, &mut submitted);
            assert_eq!(written_3, 0, "after re-upload + re-hash, 0 bytes should differ");

            // Read back page 2 directly from VRAM and verify both GPU and CPU writes exist there.
            let (rb, rb_ptr) = host.readback_buffer(PAGE as u64).expect("verify rb");
            let v_alloc: ID3D12CommandAllocator = host
                .device
                .CreateCommandAllocator(D3D12_COMMAND_LIST_TYPE_DIRECT)
                .expect("v_alloc");
            let v_list: ID3D12GraphicsCommandList = host
                .device
                .CreateCommandList(0, D3D12_COMMAND_LIST_TYPE_DIRECT, &v_alloc, None)
                .expect("v_list");
            v_list.CopyBufferRegion(&rb, 0, &card.vram, (2 * PAGE) as u64, PAGE as u64);
            v_list.Close().expect("v_close");
            let v_cmd: ID3D12CommandList = v_list.cast().expect("v_cast");
            host.execute_and_wait_locked(v_cmd, &mut submitted);
            let vram_page2 = std::slice::from_raw_parts(rb_ptr, PAGE);
            assert_eq!(&vram_page2[128..132], &gpu_bytes);
            assert_eq!(&vram_page2[256..260], &[0x11, 0x22, 0x33, 0x44]);
        }
    }
}
