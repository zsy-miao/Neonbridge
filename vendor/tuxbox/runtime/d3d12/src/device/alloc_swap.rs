//! A guest allocator Reset that keeps recordings still in use
//! (TUXBOX_ALLOC_SWAP=0: reset the host allocator directly, as before).
//!
//! On Xbox a title may Reset an allocator while a closed list recorded on it
//! is still queued or will be submitted again; desktop D3D12 wipes that
//! recording (a later execute removes the device). So the guest allocator
//! stands for a host allocator that can change: at its Reset, if a list still
//! holds a recording on the current host allocator or the GPU has not passed
//! its last submission, that host allocator is set aside untouched and an idle
//! one (no list references it, fences passed) is reset and taken, or a new
//! one created. A list Reset records on the current host allocator. Each list
//! holds one recording, so the set-aside allocators are bounded by the lists.
//! (Facts from xcl 11920b5, re-derived.)
use std::collections::HashMap;
use std::sync::atomic::{AtomicU64, Ordering};
use std::sync::{Mutex, MutexGuard, OnceLock};

use windows::Win32::Graphics::Direct3D12::*;
use windows::core::Interface;

use super::Allocator;

pub(crate) fn enabled() -> bool {
    static ON: OnceLock<bool> = OnceLock::new();
    *ON.get_or_init(|| !std::env::var("TUXBOX_ALLOC_SWAP").is_ok_and(|v| v.trim() == "0"))
}

struct Host {
    allocator: ID3D12CommandAllocator,
    /// (fence, value) of every submission of a list recorded on it.
    pending: Vec<(ID3D12Fence, u64)>,
}

#[derive(Default)]
struct Guest {
    /// Replaces the proxy's native allocator once the first swap happens.
    current: Option<Host>,
    aside: Vec<Host>,
    kind: Option<D3D12_COMMAND_LIST_TYPE>,
}

#[derive(Default)]
struct Table {
    guests: HashMap<usize, Guest>,
    /// Host allocator (raw) each list's current recording uses.
    lists: HashMap<usize, usize>,
    /// Pending submissions of host allocators that are still the proxy's own.
    native_pending: HashMap<usize, Vec<(ID3D12Fence, u64)>>,
    /// One fence per queue (raw) and its last value.
    queues: HashMap<usize, (ID3D12Fence, u64)>,
    /// Host allocators of released guests kept until no list references them
    /// and their submissions are done.
    orphans: Vec<Host>,
}

// SAFETY: D3D12 objects are free-threaded; the table is behind a mutex.
unsafe impl Send for Table {}

static TABLE: OnceLock<Mutex<Table>> = OnceLock::new();

fn table() -> MutexGuard<'static, Table> {
    TABLE.get_or_init(Default::default).lock().unwrap_or_else(|e| e.into_inner())
}

fn key(guest: &Allocator) -> usize {
    guest as *const Allocator as usize
}

fn done(pending: &mut Vec<(ID3D12Fence, u64)>) -> bool {
    pending.retain(|(fence, value)| unsafe { fence.GetCompletedValue() } < *value);
    pending.is_empty()
}

fn referenced(lists: &HashMap<usize, usize>, host: usize) -> bool {
    lists.values().any(|&h| h == host)
}

/// The host allocator a list Reset on `guest` records on.
pub(crate) fn current(guest: &Allocator) -> ID3D12CommandAllocator {
    if enabled()
        && let Some(host) = table().guests.get(&key(guest)).and_then(|g| g.current.as_ref())
    {
        return host.allocator.clone();
    }
    guest.native.clone()
}

/// A list Reset succeeded: its recording now uses `host`.
pub(crate) fn recorded(list: usize, guest: &Allocator, host: &ID3D12CommandAllocator, kind: D3D12_COMMAND_LIST_TYPE) {
    if !enabled() {
        return;
    }
    let mut t = table();
    t.lists.insert(list, host.as_raw() as usize);
    t.guests.entry(key(guest)).or_default().kind.get_or_insert(kind);
}

/// After a host ExecuteCommandLists of `lists` (raw guest list keys) on `queue`.
pub(crate) fn submitted(queue: &ID3D12CommandQueue, lists: &[usize]) {
    if !enabled() || lists.is_empty() {
        return;
    }
    let mut t = table();
    let hosts: Vec<usize> = lists.iter().filter_map(|l| t.lists.get(l).copied()).collect();
    if hosts.is_empty() {
        return;
    }
    let q = queue.as_raw() as usize;
    if !t.queues.contains_key(&q) {
        let mut device: Option<ID3D12Device> = None;
        let Some(fence) = (unsafe { queue.GetDevice(&mut device) }.ok())
            .and(device)
            .and_then(|d| unsafe { d.CreateFence::<ID3D12Fence>(0, D3D12_FENCE_FLAG_NONE) }.ok())
        else {
            return;
        };
        t.queues.insert(q, (fence, 0));
    }
    let (fence, value) = {
        let entry = t.queues.get_mut(&q).expect("inserted above");
        entry.1 += 1;
        (entry.0.clone(), entry.1)
    };
    if unsafe { queue.Signal(&fence, value) }.is_err() {
        return;
    }
    for host in hosts {
        let mut placed = false;
        for h in t.orphans.iter_mut().filter(|h| h.allocator.as_raw() as usize == host) {
            h.pending.push((fence.clone(), value));
            placed = true;
        }
        for g in t.guests.values_mut() {
            for h in g.current.iter_mut().chain(g.aside.iter_mut()) {
                if h.allocator.as_raw() as usize == host {
                    h.pending.push((fence.clone(), value));
                    placed = true;
                }
            }
        }
        if !placed {
            t.native_pending.entry(host).or_default().push((fence.clone(), value));
        }
    }
}

/// TUXBOX_ALLOC_RELEASE=0: a released guest's entries stay, as before.
pub(crate) fn release_enabled() -> bool {
    static ON: OnceLock<bool> = OnceLock::new();
    *ON.get_or_init(|| !std::env::var("TUXBOX_ALLOC_RELEASE").is_ok_and(|v| v.trim() == "0"))
}

/// The guest allocator's last Release (xcl 5c3ff31, re-derived): its entry
/// goes, so a later allocator at the same address starts clean, and its host
/// allocators (swapped and `overflow`) are dropped once nothing records on
/// them and the GPU has passed their submissions.
pub(crate) fn released(guest: &Allocator, overflow: Vec<ID3D12CommandAllocator>) {
    if !release_enabled() {
        return;
    }
    let mut guard = table();
    let t = &mut *guard;
    let mut hosts: Vec<Host> = Vec::new();
    if let Some(g) = t.guests.remove(&key(guest)) {
        hosts.extend(g.current);
        hosts.extend(g.aside);
    }
    for allocator in std::iter::once(guest.native.clone()).chain(overflow) {
        let raw = allocator.as_raw() as usize;
        let pending = t.native_pending.remove(&raw).unwrap_or_default();
        if !hosts.iter().any(|h| h.allocator.as_raw() as usize == raw) {
            hosts.push(Host { allocator, pending });
        }
    }
    t.orphans.extend(hosts);
    prune(t);
}

/// Drop orphaned host allocators that are idle.
fn prune(t: &mut Table) {
    let lists = &t.lists;
    t.orphans.retain_mut(|h| referenced(lists, h.allocator.as_raw() as usize) || !done(&mut h.pending));
}

static SWAPS: AtomicU64 = AtomicU64::new(0);
static CREATED: AtomicU64 = AtomicU64::new(0);
static REUSED: AtomicU64 = AtomicU64::new(0);

/// The guest's Reset. None: reset the current host allocator directly (it is
/// free, or the swap is off). Some(status): the swap already happened.
pub(crate) fn reset(guest: &Allocator) -> Option<windows::core::HRESULT> {
    if !enabled() {
        return None;
    }
    let mut guard = table();
    let t = &mut *guard;
    prune(t);
    let k = key(guest);
    let g = t.guests.entry(k).or_default();
    let (current_raw, busy) = match &mut g.current {
        Some(h) => {
            let raw = h.allocator.as_raw() as usize;
            (raw, referenced(&t.lists, raw) || !done(&mut h.pending))
        }
        None => {
            let raw = guest.native.as_raw() as usize;
            let pending = t.native_pending.entry(raw).or_default();
            (raw, referenced(&t.lists, raw) || !done(pending))
        }
    };
    if !busy {
        return None;
    }
    let kind = g.kind?;
    // Set the busy one aside unchanged.
    let old = match g.current.take() {
        Some(h) => h,
        None => Host {
            allocator: guest.native.clone(),
            pending: t.native_pending.remove(&current_raw).unwrap_or_default(),
        },
    };
    let idle = g.aside.iter_mut().position(|h| {
        let raw = h.allocator.as_raw() as usize;
        !referenced(&t.lists, raw) && done(&mut h.pending)
    });
    let next = match idle {
        Some(i) => {
            let host = g.aside.swap_remove(i);
            if let Err(e) = unsafe { host.allocator.Reset() } {
                g.aside.push(host);
                g.current = Some(old);
                return Some(e.code());
            }
            REUSED.fetch_add(1, Ordering::Relaxed);
            host
        }
        None => {
            let mut device: Option<ID3D12Device> = None;
            let made = unsafe { guest.native.GetDevice(&mut device) }
                .and_then(|()| device.ok_or_else(|| windows::Win32::Foundation::E_FAIL.into()))
                .and_then(|d| unsafe { d.CreateCommandAllocator::<ID3D12CommandAllocator>(kind) });
            match made {
                Ok(allocator) => {
                    CREATED.fetch_add(1, Ordering::Relaxed);
                    Host { allocator, pending: Vec::new() }
                }
                Err(e) => {
                    g.current = Some(old);
                    return Some(e.code());
                }
            }
        }
    };
    g.aside.push(old);
    g.current = Some(next);
    let n = SWAPS.fetch_add(1, Ordering::Relaxed) + 1;
    if n <= 4 || n.is_power_of_two() {
        crate::diagnostics::log_line(format_args!(
            "allocator {k:#x} reset while its recording is in use: swapped host allocators ({n} swaps, {} created, {} reused, {} set aside here; TUXBOX_ALLOC_SWAP=0 resets directly)",
            CREATED.load(Ordering::Relaxed),
            REUSED.load(Ordering::Relaxed),
            g.aside.len()
        ));
    }
    Some(windows::Win32::Foundation::S_OK)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn referenced_hosts_are_found_by_raw_pointer() {
        let lists: HashMap<usize, usize> = [(1, 0x10), (2, 0x20)].into_iter().collect();
        assert!(referenced(&lists, 0x20));
        assert!(!referenced(&lists, 0x30));
    }
}
