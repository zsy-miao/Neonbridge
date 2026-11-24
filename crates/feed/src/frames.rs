//! The reader side of the game's frame double buffer (layout in the d3d12
//! provider's present/feed.rs). Either side may create the mapping first.
use std::sync::atomic::{AtomicU32, AtomicU64, Ordering};
use std::sync::{Arc, OnceLock};

const MAX_W: usize = 1920;
const MAX_H: usize = 1080;
const HEADER: usize = 64;
const SLOT_HEADER: usize = 64;
const SLOT: usize = SLOT_HEADER + MAX_W * MAX_H * 4;
const SIZE: usize = HEADER + 2 * SLOT;

struct View(*mut u8);
// SAFETY: mapped for the process's lifetime; fields are atomics or seqlocked.
unsafe impl Send for View {}
unsafe impl Sync for View {}

impl View {
    fn u32(&self, at: usize) -> &AtomicU32 {
        unsafe { &*(self.0.add(at) as *const AtomicU32) }
    }
    fn u64(&self, at: usize) -> &AtomicU64 {
        unsafe { &*(self.0.add(at) as *const AtomicU64) }
    }
}

fn view() -> Option<&'static View> {
    static V: OnceLock<Option<View>> = OnceLock::new();
    V.get_or_init(|| unsafe {
        use windows_sys::Win32::System::Memory::*;
        let wide: Vec<u16> = crate::FRAMES_NAME.encode_utf16().chain(Some(0)).collect();
        let mapping = CreateFileMappingW(
            windows_sys::Win32::Foundation::INVALID_HANDLE_VALUE, std::ptr::null(), PAGE_READWRITE,
            (SIZE as u64 >> 32) as u32, SIZE as u32, wide.as_ptr());
        if mapping.is_null() {
            crate::say("shared frame memory unavailable");
            return None;
        }
        let at = MapViewOfFile(mapping, FILE_MAP_ALL_ACCESS, 0, 0, SIZE);
        (!at.Value.is_null()).then(|| View(at.Value.cast()))
    }).as_ref()
}

/// The feed's rate and width, read by the game at each capture.
pub(crate) fn configure(fps: u32, width: u32) {
    if let Some(v) = view() {
        v.u32(16).store(fps, Ordering::Relaxed);
        v.u32(20).store(width, Ordering::Relaxed);
    }
}

/// Someone is watching: the game captures for 5 s after the last call.
pub(crate) fn heartbeat() {
    if let Some(v) = view() {
        v.u64(8).store(unsafe { windows_sys::Win32::System::SystemInformation::GetTickCount64() }, Ordering::Release);
    }
}

pub(crate) struct Frame {
    pub width: usize,
    pub height: usize,
    /// width * height * 4 bytes, B G R 0.
    pub pixels: Arc<Vec<u8>>,
    pub number: u64,
}

/// The newest complete frame, if one was published since `after`.
pub(crate) fn latest(after: u64) -> Option<Frame> {
    let v = view()?;
    let number = v.u64(32).load(Ordering::Acquire);
    if number == 0 || number == after {
        return None;
    }
    for _ in 0..3 {
        let slot = v.u32(24).load(Ordering::Acquire) as usize & 1;
        let base = HEADER + slot * SLOT;
        let before = v.u64(base).load(Ordering::Acquire);
        if before & 1 == 1 {
            std::thread::yield_now();
            continue;
        }
        let (w, h) = (v.u32(base + 8).load(Ordering::Relaxed) as usize, v.u32(base + 12).load(Ordering::Relaxed) as usize);
        if w == 0 || h == 0 || w > MAX_W || h > MAX_H {
            return None;
        }
        // SAFETY: inside the view; a torn copy is caught by the seq check.
        let pixels = unsafe { std::slice::from_raw_parts(v.0.add(base + SLOT_HEADER), w * h * 4) }.to_vec();
        std::sync::atomic::fence(Ordering::Acquire);
        if v.u64(base).load(Ordering::Acquire) == before {
            return Some(Frame { width: w, height: h, pixels: Arc::new(pixels), number });
        }
    }
    None
}

pub(crate) fn published() -> u64 {
    view().map_or(0, |v| v.u64(32).load(Ordering::Acquire))
}
