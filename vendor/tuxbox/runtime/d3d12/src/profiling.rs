//! QPC samples collected as guest graphics mappings are torn down.
use std::sync::{
    Mutex,
    atomic::{AtomicBool, Ordering},
};
use windows::Win32::{
    Foundation::{E_INVALIDARG, S_OK},
    System::{
        Diagnostics::Debug::{ReadProcessMemory, WriteProcessMemory},
        Performance::QueryPerformanceCounter,
        Threading::GetCurrentProcess,
    },
};
use windows::core::HRESULT;

struct Buffer {
    cursor: usize,
    capacity: usize,
    timestamps: usize,
}

static BUFFER: Mutex<Option<Buffer>> = Mutex::new(None);
static ENABLED: AtomicBool = AtomicBool::new(false);

/// The caller keeps the aligned counter and sample storage writable until
/// unregistering. Replacing a registration preserves the caller's cursor.
#[unsafe(export_name = "D3D12XboxEnableMemoryProfiling")]
pub extern "system" fn enable(cursor: usize, capacity: usize, timestamps: usize) -> HRESULT {
    let buffer = if cursor == 0 {
        None
    } else {
        if !cursor.is_multiple_of(8)
            || (capacity != 0 && (timestamps == 0 || !timestamps.is_multiple_of(8)))
            || capacity
                .checked_mul(8)
                .and_then(|bytes| timestamps.checked_add(bytes))
                .is_none()
        {
            return E_INVALIDARG;
        }
        Some(Buffer {
            cursor,
            capacity,
            timestamps,
        })
    };
    let mut current = BUFFER.lock().unwrap_or_else(|e| e.into_inner());
    *current = buffer;
    ENABLED.store(current.is_some(), Ordering::Release);
    S_OK
}

pub(crate) fn mapping_released() {
    if !ENABLED.load(Ordering::Acquire) {
        return;
    }
    let current = BUFFER.lock().unwrap_or_else(|e| e.into_inner());
    let Some(buffer) = current.as_ref() else {
        return;
    };
    let mut timestamp = 0;
    let mut index = 0u64;
    // Copy through the process APIs so an inaccessible guest buffer cannot
    // fault on a host resource destruction thread. The lock spans publication
    // so deregistration waits for the last sample.
    unsafe {
        let process = GetCurrentProcess();
        if QueryPerformanceCounter(&mut timestamp).is_err()
            || ReadProcessMemory(
                process,
                buffer.cursor as _,
                (&raw mut index).cast(),
                8,
                None,
            )
            .is_err()
        {
            return;
        }
        let next = index.wrapping_add(1);
        if WriteProcessMemory(
            process,
            buffer.cursor as _,
            (&raw const next).cast(),
            8,
            None,
        )
        .is_ok()
            && index < buffer.capacity as u64
        {
            let address = buffer.timestamps + index as usize * 8;
            let _ = WriteProcessMemory(
                process,
                address as _,
                (&raw const timestamp).cast(),
                8,
                None,
            );
        }
    }
}
