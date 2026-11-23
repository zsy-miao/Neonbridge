//! Private operations implemented by the Scarlett user-mode driver.
use core::sync::atomic::{AtomicBool, Ordering};
use windows::Win32::Foundation::{E_NOINTERFACE, E_NOTIMPL, E_POINTER, S_OK};
use windows::core::HRESULT;

static ACTIVE: AtomicBool = AtomicBool::new(false);

pub(crate) fn activate() {
    ACTIVE.store(true, Ordering::Release);
}

#[repr(C)]
struct RecordUpdate {
    target: *mut u8,
    kind: u32,
    flags: u32,
}

unsafe fn replace_bits(address: *mut u32, mask: u32, value: u32) {
    // SAFETY: The selected layout contains an initialized writable DWORD here.
    unsafe {
        let previous = address.read_unaligned();
        address.write_unaligned((previous & !mask) | (value & mask));
    }
}

/// # Safety
/// Operation 1 supplies a target pointer and a layout tag. For tags 1 and 3,
/// the update contains an additional three or six DWORDs after the tag. Its
/// destination must cover the aligned payload, including byte 0xb7 for tag 3.
#[unsafe(export_name = "InternalDriverCallX")]
pub(crate) unsafe extern "system" fn dispatch(selector: u32, record: *const u8) -> HRESULT {
    if !ACTIVE.load(Ordering::Acquire) {
        return E_NOINTERFACE;
    }
    match selector {
        1 => unsafe { update(record) },
        2 => E_NOTIMPL,
        3 if record.is_null() => E_POINTER,
        3 => {
            #[repr(C)]
            struct Hook {
                device: *mut core::ffi::c_void,
                callback: Option<crate::device::DisposeCallback>,
                context: *mut core::ffi::c_void,
            }
            let hook = unsafe { record.cast::<Hook>().read_unaligned() };
            unsafe { crate::device::set_dispose_callback(hook.device, hook.callback, hook.context) }
        }
        _ => E_NOINTERFACE,
    }
}

unsafe fn update(record: *const u8) -> HRESULT {
    if record.is_null() {
        return E_POINTER;
    }
    // SAFETY: All update headers contain the tag; unknown tags do not use
    // the remaining fields or require a writable target.
    let kind = unsafe { record.add(8).cast::<u32>().read_unaligned() };
    let alignment = match kind {
        1 => 16,
        3 => 32,
        _ => return S_OK,
    };
    let header = unsafe { record.cast::<RecordUpdate>().read_unaligned() };
    if header.target.is_null() {
        return E_POINTER;
    }
    // Both driver layouts start after a 64-byte prefix followed by padding.
    let dst = unsafe { header.target.add(64) };
    let dst = unsafe { dst.add(dst.align_offset(alignment)) }.cast::<u32>();
    let src = unsafe { record.add(16) }.cast::<u32>();
    let flags = header.flags;
    unsafe {
        if kind == 1 {
            replace_bits(dst, 7, flags >> 3);
            replace_bits(
                dst.add(1),
                0x0400_6000,
                ((flags & 1) << 14) | ((flags & 2) << 12) | ((flags & 4) << 24),
            );
            dst.add(2).write_unaligned(src.read_unaligned());
            dst.add(3).write_unaligned(src.add(1).read_unaligned());
        } else {
            let extra = src.add(4).read_unaligned();
            replace_bits(dst, 0x3f, (extra & 3) | ((flags >> 2) & 0x3c));
            replace_bits(dst.add(1), 0x60, ((flags & 1) << 6) | ((flags & 2) << 4));
            replace_bits(dst.add(0xb4 / 4), 0xf, ((flags >> 2) & 3) | (extra & 0xc));
            dst.add(2).write_unaligned(src.add(1).read_unaligned());
            dst.add(3).write_unaligned(src.read_unaligned());
            dst.add(4).write_unaligned(src.add(2).read_unaligned());
            dst.add(5).write_unaligned(src.add(3).read_unaligned());
        }
    }
    S_OK
}
