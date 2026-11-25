//! Slot 148 hands out memory that lives as long as the command allocator.
//!
//! The console's XboxBvhBuilder calls slot 148 with (byte count, alignment)
//! and writes build descriptors and post-build records into the returned
//! pointer before recording commands that read it by GPU virtual address. On
//! the console that memory comes from the allocator's command buffer and stays
//! valid until the allocator is reset. A null result is checked by only one of
//! the four call sites, so it is not an acceptable fallback.
//!
//! Here each allocator owns a bump arena of committed host pages. Guest GPU
//! addresses equal CPU addresses, so commands that read the memory resolve it
//! through the usual guest-memory import. Pages are never released: a reset
//! returns them to a shared pool, so the same addresses come back and any GPU
//! import of them stays valid.

use std::collections::HashMap;
use std::sync::{LazyLock, Mutex};

use windows::Win32::System::Memory::{VirtualAlloc, MEM_COMMIT, MEM_RESERVE, PAGE_READWRITE};

use super::{recording, List};

const CHUNK: usize = 1 << 20;

struct Chunk {
    base: usize,
    size: usize,
}

#[derive(Default)]
struct Arena {
    chunks: Vec<Chunk>,
    used: usize,
}

#[derive(Default)]
struct Pool {
    arenas: HashMap<usize, Arena>,
    free: Vec<Chunk>,
}

static POOL: LazyLock<Mutex<Pool>> = LazyLock::new(Default::default);

fn take_chunk(free: &mut Vec<Chunk>, bytes: usize) -> Option<Chunk> {
    if let Some(index) = free.iter().position(|chunk| chunk.size >= bytes) {
        return Some(free.swap_remove(index));
    }
    let size = bytes.max(CHUNK).checked_next_multiple_of(0x10000)?;
    let base = unsafe { VirtualAlloc(None, size, MEM_COMMIT | MEM_RESERVE, PAGE_READWRITE) };
    (!base.is_null()).then(|| Chunk { base: base as usize, size })
}

/// Bump-allocate `bytes` aligned to `alignment` from the allocator's arena.
fn allocate(allocator: usize, bytes: usize, alignment: usize) -> Option<usize> {
    if bytes == 0 || !alignment.is_power_of_two() || alignment > 0x10000 {
        return None;
    }
    let mut pool = POOL.lock().unwrap_or_else(|e| e.into_inner());
    let Pool { arenas, free } = &mut *pool;
    let arena = arenas.entry(allocator).or_default();
    if let Some(chunk) = arena.chunks.last() {
        let start = (chunk.base + arena.used).checked_next_multiple_of(alignment)?;
        if start.checked_add(bytes)? <= chunk.base + chunk.size {
            arena.used = start + bytes - chunk.base;
            return Some(start);
        }
    }
    let chunk = take_chunk(free, bytes)?;
    let start = chunk.base;
    arena.used = bytes;
    arena.chunks.push(chunk);
    Some(start)
}

/// Return an allocator's pages to the pool once its native reset succeeded.
pub(crate) fn release(allocator: usize) {
    let mut pool = POOL.lock().unwrap_or_else(|e| e.into_inner());
    if let Some(arena) = pool.arenas.remove(&allocator) {
        pool.free.extend(arena.chunks);
    }
}

pub(super) unsafe extern "system" fn allocate_slot(list: &List, bytes: u32, alignment: u32) -> *mut core::ffi::c_void {
    let state = recording(list);
    let allocator = std::sync::Arc::as_ptr(&state.resources) as usize;
    drop(state);
    let result = allocate(allocator, bytes as usize, (alignment as usize).max(1));
    static FIRST: std::sync::Once = std::sync::Once::new();
    crate::diagnostics::log_once(&FIRST, || format!(
        "slot 148 list memory bytes={bytes} alignment={alignment} -> {:#x}", result.unwrap_or(0)));
    result.map_or(core::ptr::null_mut(), |address| address as *mut _)
}

#[cfg(test)]
#[path = "../tests/commands/list_memory.rs"]
mod tests;
