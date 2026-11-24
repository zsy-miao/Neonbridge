//! Committed copies of dropped placed textures, kept for the next placement
//! with the same desktop description. test48 created 38k copies in two
//! minutes (16 GB) because each new placement made its own and a dropped
//! one's memory was given back only to be allocated again. The pool holds
//! at most TUXBOX_PLACED_TEXTURE_POOL_MB (default 512) and evicts the
//! oldest; TUXBOX_PLACED_TEXTURE_REUSE=0 releases copies as before.
use std::collections::VecDeque;
use std::sync::Mutex;
use std::sync::atomic::{AtomicU64, Ordering::Relaxed};
use windows::Win32::Graphics::Direct3D12::{D3D12_RESOURCE_DESC, ID3D12Device, ID3D12Fence, ID3D12Resource};
use windows::core::Interface;

/// Entries keyed by K, oldest first, with their byte sizes.
struct Pool<K, T> {
    entries: VecDeque<(K, T, u64)>,
    bytes: u64,
}

impl<K: PartialEq, T> Pool<K, T> {
    const fn new() -> Self {
        Self { entries: VecDeque::new(), bytes: 0 }
    }

    /// The newest entry with this key.
    fn take(&mut self, key: &K) -> Option<(T, u64)> {
        let at = self.entries.iter().rposition(|(k, _, _)| k == key)?;
        let (_, value, bytes) = self.entries.remove(at)?;
        self.bytes -= bytes;
        Some((value, bytes))
    }

    /// Adds an entry, then evicts the oldest until the pool fits the cap.
    /// An entry larger than the cap is not kept. Returns the evicted count.
    fn give(&mut self, key: K, value: T, bytes: u64, cap: u64) -> usize {
        if bytes > cap {
            return 0;
        }
        self.entries.push_back((key, value, bytes));
        self.bytes += bytes;
        let mut evicted = 0;
        while self.bytes > cap && let Some((_, _, b)) = self.entries.pop_front() {
            self.bytes -= b;
            evicted += 1;
        }
        evicted
    }
}

/// The device a copy belongs to and its description.
#[derive(PartialEq)]
struct Key {
    device: usize,
    desc: D3D12_RESOURCE_DESC,
}

struct Pooled {
    native: ID3D12Resource,
    readers: Vec<(ID3D12Fence, u64)>,
}

static POOL: Mutex<Pool<Key, Pooled>> = Mutex::new(Pool::new());
static LIVE: AtomicU64 = AtomicU64::new(0);
static LIVE_BYTES: AtomicU64 = AtomicU64::new(0);
static POOLED: AtomicU64 = AtomicU64::new(0);
static POOLED_BYTES: AtomicU64 = AtomicU64::new(0);
static REUSED: AtomicU64 = AtomicU64::new(0);
static SUPERSEDED: AtomicU64 = AtomicU64::new(0);
static SUPERSEDED_BYTES: AtomicU64 = AtomicU64::new(0);

/// A live texture a newer placement overlaps (true), or one of those dropped.
pub(super) fn superseded(bytes: u64, now: bool) {
    if now {
        SUPERSEDED.fetch_add(1, Relaxed);
        SUPERSEDED_BYTES.fetch_add(bytes, Relaxed);
    } else {
        SUPERSEDED.fetch_sub(1, Relaxed);
        SUPERSEDED_BYTES.fetch_sub(bytes, Relaxed);
    }
}

fn enabled() -> bool {
    crate::env_toggle!("TUXBOX_PLACED_TEXTURE_REUSE") != Some("0")
}

fn cap() -> u64 {
    crate::env_toggle!("TUXBOX_PLACED_TEXTURE_POOL_MB").and_then(|v| v.trim().parse::<u64>().ok()).unwrap_or(512) << 20
}

fn lock() -> std::sync::MutexGuard<'static, Pool<Key, Pooled>> {
    POOL.lock().unwrap_or_else(|e| e.into_inner())
}

fn publish(pool: &Pool<Key, Pooled>) {
    POOLED.store(pool.entries.len() as u64, Relaxed);
    POOLED_BYTES.store(pool.bytes, Relaxed);
}

/// A pooled copy for this description, with the fences its last readers
/// signal, so the first upload waits for them.
pub(super) fn take(device: &ID3D12Device, desc: &D3D12_RESOURCE_DESC) -> Option<(ID3D12Resource, u64, Vec<(ID3D12Fence, u64)>)> {
    if !enabled() {
        return None;
    }
    let key = Key { device: device.as_raw() as usize, desc: *desc };
    let mut pool = lock();
    let (pooled, bytes) = pool.take(&key)?;
    publish(&pool);
    REUSED.fetch_add(1, Relaxed);
    Some((pooled.native, bytes, pooled.readers))
}

/// A texture's copy now in use.
pub(super) fn created(bytes: u64) {
    LIVE.fetch_add(1, Relaxed);
    LIVE_BYTES.fetch_add(bytes, Relaxed);
}

/// A dropped texture's copy: pooled, or released with the last reference.
pub(super) fn release(native: &ID3D12Resource, desc: D3D12_RESOURCE_DESC, bytes: u64, readers: Vec<(ID3D12Fence, u64)>) {
    LIVE.fetch_sub(1, Relaxed);
    LIVE_BYTES.fetch_sub(bytes, Relaxed);
    if !enabled() || bytes == 0 {
        return;
    }
    let mut device: Option<ID3D12Device> = None;
    if unsafe { native.GetDevice(&mut device) }.is_err() { return }
    let Some(device) = device else { return };
    let key = Key { device: device.as_raw() as usize, desc };
    let mut pool = lock();
    pool.give(key, Pooled { native: native.clone(), readers }, bytes, cap());
    publish(&pool);
}

pub(crate) struct Stats {
    pub(crate) live: u64,
    pub(crate) live_bytes: u64,
    pub(crate) pooled: u64,
    pub(crate) pooled_bytes: u64,
    pub(crate) reused: u64,
    pub(crate) superseded: u64,
    pub(crate) superseded_bytes: u64,
}

pub(crate) fn stats() -> Stats {
    Stats {
        live: LIVE.load(Relaxed),
        live_bytes: LIVE_BYTES.load(Relaxed),
        pooled: POOLED.load(Relaxed),
        pooled_bytes: POOLED_BYTES.load(Relaxed),
        reused: REUSED.load(Relaxed),
        superseded: SUPERSEDED.load(Relaxed),
        superseded_bytes: SUPERSEDED_BYTES.load(Relaxed),
    }
}

#[cfg(test)]
mod tests {
    use super::Pool;

    #[test]
    fn takes_by_key_and_evicts_the_oldest_past_the_cap() {
        let mut pool = Pool::new();
        assert_eq!(pool.give(1, 'a', 40, 100), 0);
        assert_eq!(pool.give(2, 'b', 40, 100), 0);
        assert_eq!(pool.give(1, 'c', 40, 100), 1);
        assert_eq!(pool.bytes, 80);
        assert_eq!(pool.take(&1), Some(('c', 40)));
        assert_eq!(pool.take(&1), None);
        assert_eq!(pool.give(3, 'd', 200, 100), 0);
        assert_eq!(pool.take(&3), None);
        assert_eq!(pool.take(&2), Some(('b', 40)));
        assert_eq!(pool.bytes, 0);
    }
}
