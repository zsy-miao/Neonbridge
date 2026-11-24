//! TUXBOX_TRANSIENT_WATCH=1: a watch over the title's transient-memory pool,
//! ported from five-star-recomp (GPL-3.0) TransientPoolWatch.cs,
//! TransientPoolHits.cs and TransientPoolLifetime.cs (4cdbfc8, b01c2c1).
//!
//! The pool cuts one region into equal blocks. A table of 0x58-byte block
//! records holds each block's address (+0x10), its retire token (+0x18) and a
//! list (+0x48) of 0x18-byte nodes {next, pooled object, token} that live in
//! the block itself. Every 5 ms the watch thread:
//! - checks each block's node list: a next pointer outside the pool or an
//!   object pointer outside user space, seen on three polls in a row, is a
//!   write the title did not make, and is reported once with the memory
//!   around it and the recent root views inside the pool;
//! - notes a block whose retire token changed (the title took it again) and
//!   reports every submission that reached that block through a root view
//!   and has not completed on its queue: GPU work that can still write memory
//!   the title reuses (the lifetime check).
//!
//! The globals are read at the title's image base plus five-star's RVAs
//! (pool base 0x75df1b8, table 0x75df1c0, count 0x75df1c8, size 0x75df1cc);
//! TUXBOX_TRANSIENT_POOL_RVA=<hex> moves all four (the other three follow at
//! +8, +0x10, +0x14). Nothing is written to title memory.
use std::collections::HashMap;
use std::sync::Mutex;
use std::sync::atomic::{AtomicBool, AtomicU64, AtomicUsize, Ordering};
use windows::Win32::Graphics::Direct3D12::*;
use windows::core::Interface;

const RECORD: usize = 0x58;
const BLOCK_AT: usize = 0x10;
const TOKEN_AT: usize = 0x18;
const LIST_AT: usize = 0x48;
const NODE: u64 = 0x18;
const MAX_NODES: usize = 1 << 16;
const CONFIRMATIONS: u32 = 3;
const PER_BLOCK: usize = 16;
const ROOTS: usize = 4096;
const REPORTED: u64 = 24;

static POOL: Mutex<Option<Pool>> = Mutex::new(None);
static START: AtomicU64 = AtomicU64::new(0);
static END: AtomicU64 = AtomicU64::new(0);
/// Recent root view addresses inside the pool: (address, list).
static ROOT_RING: Mutex<Vec<(u64, usize)>> = Mutex::new(Vec::new());
static ROOT_NEXT: AtomicUsize = AtomicUsize::new(0);
/// Blocks each open list reached through a root view.
static TOUCHED: Mutex<Option<HashMap<usize, Vec<u32>>>> = Mutex::new(None);
/// Per queue: its fence and the last value signalled.
static QUEUES: Mutex<Vec<(usize, ID3D12Fence, u64)>> = Mutex::new(Vec::new());

struct Pool {
    size: u64,
    blocks: u32,
    /// Per block: the last submissions that reached it (fence, value, queue).
    kept: Vec<Vec<(ID3D12Fence, u64, usize)>>,
    tokens: Vec<u64>,
}

// SAFETY: D3D12 fences are free-threaded.
unsafe impl Send for Pool {}

pub(crate) fn enabled() -> bool {
    static ON: std::sync::OnceLock<bool> = std::sync::OnceLock::new();
    *ON.get_or_init(|| {
        let on = crate::env_toggle!("TUXBOX_TRANSIENT_WATCH") == Some("1");
        if on {
            let _ = std::thread::Builder::new().name("TuxBox transient watch".into()).spawn(run);
        }
        on
    })
}

/// A root view a list set: kept when it lies in the pool.
pub(crate) fn touch(list: usize, address: u64) {
    if !enabled() || address < START.load(Ordering::Relaxed) || address >= END.load(Ordering::Relaxed) {
        return;
    }
    {
        let mut ring = ROOT_RING.lock().unwrap_or_else(|e| e.into_inner());
        let at = ROOT_NEXT.fetch_add(1, Ordering::Relaxed) % ROOTS;
        if ring.len() < ROOTS { ring.push((address, list)); } else { ring[at] = (address, list); }
    }
    let size = POOL.lock().unwrap_or_else(|e| e.into_inner()).as_ref().map_or(0, |p| p.size);
    if size == 0 {
        return;
    }
    let block = ((address - START.load(Ordering::Relaxed)) / size) as u32;
    let mut touched = TOUCHED.lock().unwrap_or_else(|e| e.into_inner());
    let blocks = touched.get_or_insert_with(HashMap::new).entry(list).or_default();
    if !blocks.contains(&block) {
        blocks.push(block);
    }
}

/// The lists of a batch were submitted on `queue`: signal its fence and keep
/// that value for every block they reached.
pub(crate) fn submitted(queue: &ID3D12CommandQueue, lists: &[usize]) {
    if !enabled() || START.load(Ordering::Relaxed) == 0 {
        return;
    }
    let blocks: Vec<u32> = {
        let mut touched = TOUCHED.lock().unwrap_or_else(|e| e.into_inner());
        let Some(map) = touched.as_mut() else { return };
        lists.iter().filter_map(|l| map.remove(l)).flatten().collect()
    };
    if blocks.is_empty() {
        return;
    }
    let me = queue.as_raw() as usize;
    let (fence, value) = {
        let mut queues = QUEUES.lock().unwrap_or_else(|e| e.into_inner());
        if !queues.iter().any(|q| q.0 == me) {
            let mut device: Option<ID3D12Device> = None;
            if unsafe { queue.GetDevice(&mut device) }.is_err() { return; }
            let Some(device) = device else { return };
            let Ok(fence) = (unsafe { device.CreateFence::<ID3D12Fence>(0, D3D12_FENCE_FLAG_NONE) }) else { return };
            queues.push((me, fence, 0));
        }
        let Some(entry) = queues.iter_mut().find(|q| q.0 == me) else { return };
        if unsafe { queue.Signal(&entry.1, entry.2 + 1) }.is_err() { return; }
        entry.2 += 1;
        (entry.1.clone(), entry.2)
    };
    let mut pool = POOL.lock().unwrap_or_else(|e| e.into_inner());
    let Some(pool) = pool.as_mut() else { return };
    for block in blocks {
        if let Some(kept) = pool.kept.get_mut(block as usize) {
            if kept.len() >= PER_BLOCK { kept.remove(0); }
            kept.push((fence.clone(), value, me));
        }
    }
}

fn read<T: Copy>(at: u64) -> Option<T> {
    crate::commands::accessible(at as usize, size_of::<T>(), false).then(|| unsafe { (at as *const T).read_unaligned() })
}

fn globals() -> Option<(u64, u64, u32, u32)> {
    let image = unsafe { windows::Win32::System::LibraryLoader::GetModuleHandleW(None) }.ok()?.0 as u64;
    let rva = crate::env_toggle!("TUXBOX_TRANSIENT_POOL_RVA")
        .and_then(|v| u64::from_str_radix(v.trim().trim_start_matches("0x"), 16).ok())
        .unwrap_or(0x75df1b8);
    let base = image + rva;
    Some((read::<u64>(base)?, read::<u64>(base + 8)?, read::<u32>(base + 0x10)?, read::<u32>(base + 0x14)?))
}

fn valid(node: u64, pool: u64, end: u64) -> bool {
    if node < pool || node + NODE > end || node & 7 != 0 {
        return false;
    }
    let Some(item) = read::<u64>(node + 8) else { return false };
    item == 0 || ((0x10000..=0x7fff_ffff_ffff).contains(&item) && item & 3 == 0)
}

fn run() {
    let mut announced = false;
    let mut suspect = (0u64, 0u32);
    static REPORTS: AtomicU64 = AtomicU64::new(0);
    static CORRUPT: AtomicBool = AtomicBool::new(false);
    let (mut recycles, mut violations) = (0u64, 0u64);
    let mut summary = std::time::Instant::now();
    loop {
        std::thread::sleep(std::time::Duration::from_millis(5));
        let Some((pool, table, count, size)) = globals() else { continue };
        if pool == 0 || table == 0 || count == 0 || size == 0 || count > 1 << 20 || size & 0xff != 0 {
            continue;
        }
        let end = pool + u64::from(count) * u64::from(size);
        if !announced {
            crate::diagnostics::log_line(format_args!(
                "transient watch: the pool at {pool:#x}: {count} blocks of {size:#x} bytes, records at {table:#x} (five-star-recomp TransientPoolWatch; TUXBOX_TRANSIENT_WATCH=1)"));
            *POOL.lock().unwrap_or_else(|e| e.into_inner()) = Some(Pool {
                size: u64::from(size), blocks: count,
                kept: (0..count).map(|_| Vec::new()).collect(), tokens: vec![0; count as usize],
            });
            START.store(pool, Ordering::Relaxed);
            END.store(end, Ordering::Relaxed);
            announced = true;
        }
        let mut clean = true;
        for i in 0..count {
            let record = table + u64::from(i) * RECORD as u64;
            // Lifetime: a changed retire token means the block was taken again.
            if let Some(token) = read::<u64>(record + TOKEN_AT as u64) {
                let late: Vec<(u64, u64, usize)> = {
                    let mut guard = POOL.lock().unwrap_or_else(|e| e.into_inner());
                    let Some(p) = guard.as_mut() else { continue };
                    let previous = std::mem::replace(&mut p.tokens[i as usize], token);
                    if previous == token || previous == 0 || i >= p.blocks {
                        Vec::new()
                    } else {
                        recycles += 1;
                        p.kept[i as usize].iter()
                            .filter_map(|(fence, value, queue)| {
                                let done = unsafe { fence.GetCompletedValue() };
                                (done < *value).then_some((*value, done, *queue))
                            })
                            .collect()
                    }
                };
                for (value, done, queue) in late {
                    violations += 1;
                    if REPORTS.fetch_add(1, Ordering::Relaxed) < REPORTED {
                        crate::diagnostics::log_line(format_args!(
                            "transient lifetime: block {i} taken again (token {token:#x}) while a submission that reached it is unfinished: queue {queue:#x} at {done} of {value} ({} behind)",
                            value - done));
                    }
                }
            }
            if CORRUPT.load(Ordering::Relaxed) {
                continue;
            }
            let Some(mut node) = read::<u64>(record + LIST_AT as u64) else { continue };
            let mut holder = record + LIST_AT as u64;
            for _ in 0..MAX_NODES {
                if node == 0 { break; }
                if !valid(node, pool, end) {
                    clean = false;
                    if suspect.0 != node { suspect = (node, 0); }
                    suspect.1 += 1;
                    if suspect.1 >= CONFIRMATIONS {
                        CORRUPT.store(true, Ordering::Relaxed);
                        report(i, record, holder, node, size);
                    }
                    break;
                }
                holder = node;
                match read::<u64>(node) { Some(next) => node = next, None => break }
            }
        }
        if clean { suspect = (0, 0); }
        if summary.elapsed() >= std::time::Duration::from_secs(10) && recycles != 0 {
            summary = std::time::Instant::now();
            crate::diagnostics::log_line(format_args!(
                "transient lifetime: {recycles} blocks taken again, {violations} unfinished submissions on them so far"));
        }
    }
}

fn report(index: u32, record: u64, holder: u64, node: u64, size: u32) {
    let block = read::<u64>(record + BLOCK_AT as u64).unwrap_or(0);
    let token = read::<u64>(record + TOKEN_AT as u64).unwrap_or(0);
    let mut text = format!(
        "transient watch: corrupted node list: block {index} ({block:#x}, record {record:#x}, token {token:#x}) reaches {node:#x} through {holder:#x} (a block's last node is at offset {:#x})",
        u64::from(size) - NODE);
    let around = holder.saturating_sub(0x20) & !0xf;
    for row in 0..6u64 {
        let at = around + row * 16;
        let words: Vec<String> = (0..2).map(|w| read::<u64>(at + w * 8).map_or("????????????????".into(), |v| format!("{v:016x}"))).collect();
        text.push_str(&format!("\n  {at:#x}: {}", words.join(" ")));
    }
    let roots = ROOT_RING.lock().unwrap_or_else(|e| e.into_inner()).clone();
    let near: Vec<String> = roots.iter().filter(|(a, _)| a.abs_diff(block) < u64::from(size)).take(24)
        .map(|(a, l)| format!("{a:#x} (list {l:#x})")).collect();
    text.push_str(&format!("\n  recent root views in this block: {}", if near.is_empty() { "none".into() } else { near.join(", ") }));
    crate::diagnostics::log_line(format_args!("{text}"));
}
