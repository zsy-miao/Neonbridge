//! Freeing shader-visible table slots of a paged arena (TODO: f40b107, the
//! table arena never freed a range). Ported from VIctory
//! crates/victory-gpu/src/descriptor/host.rs HostHeap::dynamic/release and
//! draw.rs State::drop (slots leased by a list, given back when it is reset
//! or released), with the c1c3281 wrap-around scan fix, GPL-3.0.
//!
//! Our ranges are cached across lists rather than leased per list, so a
//! range is only freed when the arena is full and no list may still read
//! it: every list that binds the arena holds a [`Lease`] from the arena's
//! clock; a range stamped at its last use is free once every live lease
//! started after that stamp. A lease lives until its list is reset or
//! released and every submission of it is done on the GPU (a per-queue fence
//! signalled after each ExecuteCommandLists), as VIctory gives slots back
//! only when the holding list retires. TUXBOX_TABLE_ARENA_FREE=0 never frees.
use std::collections::HashMap;
use std::sync::{Arc, Mutex, atomic::{AtomicBool, AtomicU64, Ordering}};
use windows::Win32::Graphics::Direct3D12::*;
use windows::core::Interface;

pub(super) fn enabled() -> bool {
    crate::env_toggle!("TUXBOX_TABLE_ARENA_FREE") != Some("0")
}

#[derive(Default)]
pub(crate) struct LeaseState {
    start: AtomicU64,
    ended: AtomicBool,
    /// A submission no fence could track: never retires (alloc_swap's untracked).
    untracked: AtomicBool,
    pending: Mutex<Vec<(ID3D12Fence, u64)>>,
}
// SAFETY: D3D12 fences are free-threaded; the list is behind a mutex.
unsafe impl Send for LeaseState {}
unsafe impl Sync for LeaseState {}

impl LeaseState {
    fn live(&self) -> bool {
        if !self.ended.load(Ordering::Acquire) || self.untracked.load(Ordering::Acquire) {
            return true;
        }
        let mut pending = self.pending.lock().unwrap_or_else(|e| e.into_inner());
        pending.retain(|(fence, value)| unsafe { fence.GetCompletedValue() } < *value);
        !pending.is_empty()
    }
}

/// A recording's hold on an arena's table slots; dropped at list reset or
/// release, after which it retires with its last submission.
pub(crate) struct Lease {
    pub(super) arena: Arc<super::Arena>,
    pub(super) state: Arc<LeaseState>,
}
impl Drop for Lease {
    fn drop(&mut self) {
        self.state.ended.store(true, Ordering::Release);
    }
}
impl Lease {
    pub(crate) fn arena(&self) -> &Arc<super::Arena> { &self.arena }
    pub(crate) fn state(&self) -> Arc<LeaseState> { self.state.clone() }
    /// A bundle executed in this list: hold whatever the bundle recorded.
    pub(crate) fn cover(&self, other: &Lease) {
        self.state.start.fetch_min(other.state.start.load(Ordering::Acquire), Ordering::AcqRel);
    }
}

#[derive(Default)]
pub(super) struct Clock {
    now: AtomicU64,
    leases: Mutex<Vec<Arc<LeaseState>>>,
}
impl Clock {
    pub(super) fn lease(&self) -> Arc<LeaseState> {
        let state = Arc::new(LeaseState::default());
        let mut leases = self.leases.lock().unwrap_or_else(|e| e.into_inner());
        state.start.store(self.now.fetch_add(1, Ordering::AcqRel), Ordering::Release);
        leases.push(state.clone());
        state
    }
    /// The stamp of a use now: above the start of every lease taken so far.
    pub(super) fn stamp(&self) -> u64 {
        self.now.load(Ordering::Acquire)
    }
    /// Ranges stamped at or below this are read by no live lease.
    pub(super) fn horizon(&self) -> u64 {
        let mut leases = self.leases.lock().unwrap_or_else(|e| e.into_inner());
        leases.retain(|lease| lease.live());
        leases.iter().map(|lease| lease.start.load(Ordering::Acquire)).min().unwrap_or(u64::MAX)
    }
}

/// Free slots below the bump cursor and where the search starts.
#[derive(Default)]
pub(super) struct FreeSlots {
    pub(super) free: Vec<bool>,
    pub(super) next: u32,
}
impl FreeSlots {
    pub(super) fn give_back(&mut self, first: u32, n: u32) {
        let end = (first + n) as usize;
        if self.free.len() < end {
            self.free.resize(end, false);
        }
        self.free[first as usize..end].fill(true);
    }
    /// `n` consecutive free slots (HostHeap::dynamic with the c1c3281 fix:
    /// the second pass runs n - 1 past the cursor so a run crossing it is found).
    pub(super) fn take(&mut self, n: u32) -> Option<u32> {
        let len = self.free.len();
        let n = n as usize;
        if n == 0 || n > len {
            return None;
        }
        let cursor = (self.next as usize).min(len);
        let second_end = cursor.saturating_add(n - 1).min(len);
        let starts = (cursor..len).chain(0..second_end);
        let mut run = 0;
        let mut last = usize::MAX;
        let mut found = None;
        for at in starts {
            if at != last.wrapping_add(1) {
                run = 0;
            }
            last = at;
            run = if self.free[at] { run + 1 } else { 0 };
            if run == n {
                found = Some(at + 1 - n);
                break;
            }
        }
        let first = found?;
        self.free[first..first + n].fill(false);
        self.next = ((first + n) % len) as u32;
        Some(first as u32)
    }
}

/// After a host ExecuteCommandLists whose lists hold `leases` on `queue`:
/// each lease stays live until this submission is done.
pub(crate) fn submitted(queue: &ID3D12CommandQueue, leases: &[Arc<LeaseState>]) {
    if leases.is_empty() || !enabled() {
        return;
    }
    struct Fences(HashMap<usize, (ID3D12Fence, u64)>);
    // SAFETY: D3D12 fences are free-threaded; the table is behind a mutex.
    unsafe impl Send for Fences {}
    static FENCES: Mutex<Option<Fences>> = Mutex::new(None);
    let mut fences = FENCES.lock().unwrap_or_else(|e| e.into_inner());
    let fences = &mut fences.get_or_insert_with(|| Fences(HashMap::new())).0;
    let key = queue.as_raw() as usize;
    if !fences.contains_key(&key) {
        let mut device: Option<ID3D12Device> = None;
        let fence = (unsafe { queue.GetDevice(&mut device) }.ok())
            .and(device)
            .and_then(|d| unsafe { d.CreateFence::<ID3D12Fence>(0, D3D12_FENCE_FLAG_NONE) }.ok());
        let Some(fence) = fence else {
            // Without a fence these leases never retire: their ranges stay.
            for lease in leases {
                lease.untracked.store(true, Ordering::Release);
            }
            return;
        };
        fences.insert(key, (fence, 0));
    }
    let entry = fences.get_mut(&key).expect("inserted above");
    entry.1 += 1;
    let (fence, value) = (entry.0.clone(), entry.1);
    let signalled = unsafe { queue.Signal(&fence, value) }.is_ok();
    for lease in leases {
        if !signalled {
            lease.untracked.store(true, Ordering::Release);
            continue;
        }
        lease.pending.lock().unwrap_or_else(|e| e.into_inner()).push((fence.clone(), value));
    }
}
