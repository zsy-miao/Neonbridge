//! Size/alignment query watch (TUXBOX_SIZE_WATCH=0 off): every XG layout query
//! for a 160x90 texture, a 0x4b0000-byte answer, or a small format 24/26
//! texture logs its inputs, our size and alignment, and the callers as
//! game+RVA, so the answer can be compared with what the title then allocates.
use std::sync::atomic::{AtomicU32, Ordering};

#[allow(clippy::too_many_arguments)]
pub(crate) fn note(entry: &str, width: u64, height: u32, format: u32, mips: u32, slices: u32, flags: u32, result: i32, size: u64, alignment: u64) {
    let wanted = (width == 160 && height == 90) || size == 0x4b_0000 || (matches!(format, 24 | 26) && width <= 320 && height <= 180);
    if !wanted || std::env::var_os("TUXBOX_SIZE_WATCH").is_some_and(|v| v == "0") {
        return;
    }
    static LINES: AtomicU32 = AtomicU32::new(0);
    if LINES.fetch_add(1, Ordering::Relaxed) >= 64 {
        return;
    }
    let line = format!(
        "TuxBox xg: size query {entry}: {width}x{height} format {format} mips {mips} slices {slices} flags {flags:#x} -> hr {result:#x} size {size:#x} alignment {alignment:#x}; callers {}",
        callers()
    );
    if std::env::var_os("TUXBOX_LOG").is_some_and(|v| v == "stderr") {
        eprintln!("{line}");
    }
}

/// Return addresses above this function, as game+RVA where they fall in the title image.
pub(crate) fn callers() -> String {
    use windows_sys::Win32::System::Diagnostics::Debug::RtlCaptureStackBackTrace;
    use windows_sys::Win32::System::LibraryLoader::GetModuleHandleW;
    let mut frames = [core::ptr::null_mut(); 10];
    let n = unsafe { RtlCaptureStackBackTrace(2, frames.len() as u32, frames.as_mut_ptr(), core::ptr::null_mut()) } as usize;
    let base = unsafe { GetModuleHandleW(core::ptr::null()) } as usize;
    let size = image_size(base);
    frames[..n]
        .iter()
        .map(|&f| {
            let a = f as usize;
            if base != 0 && a >= base && a < base + size { format!("game+{:#x}", a - base) } else { format!("{a:#x}") }
        })
        .collect::<Vec<_>>()
        .join(" < ")
}

fn image_size(base: usize) -> usize {
    if base == 0 {
        return 0;
    }
    unsafe {
        let nt = base + (base as *const u8).add(0x3c).cast::<u32>().read_unaligned() as usize;
        (nt as *const u8).add(24 + 56).cast::<u32>().read_unaligned() as usize // SizeOfImage
    }
}
