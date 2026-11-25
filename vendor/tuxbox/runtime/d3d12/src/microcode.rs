//! The custom-microcode export validates and submits one synchronous driver request.
use std::sync::Once;
use windows::Win32::Foundation::{
    E_FAIL, E_INVALIDARG, ERROR_INVALID_HANDLE, NTSTATUS, S_OK, STATUS_SUCCESS,
    STATUS_UNSUCCESSFUL, SetLastError,
};
use windows::core::HRESULT;

#[repr(C)]
#[derive(Clone, Copy)]
struct Image {
    address: *const u8,
    bytes: usize,
}

#[repr(C)]
struct Upload {
    selector: u32,
    reserved: u32,
    images: [Image; 3],
}

const _: () = {
    assert!(size_of::<Upload>() == 56);
    assert!(core::mem::offset_of!(Upload, reserved) == 4);
    assert!(core::mem::offset_of!(Upload, images) == 8);
    assert!(core::mem::offset_of!(Image, address) == 0);
    assert!(core::mem::offset_of!(Image, bytes) == 8);
};

fn submit(
    selector: u32,
    buffers: [(usize, *const u8); 3],
    driver: impl FnOnce(&Upload) -> NTSTATUS,
) -> HRESULT {
    let images = buffers.map(|(bytes, address)| Image { address, bytes });
    if selector == 0
        || images
            .iter()
            .any(|image| image.address.is_null() || image.bytes == 0)
    {
        return E_INVALIDARG;
    }
    let request = Upload {
        selector,
        reserved: 0,
        images,
    };
    if driver(&request) == STATUS_SUCCESS {
        S_OK
    } else {
        E_FAIL
    }
}

fn desktop_driver(request: &Upload) -> NTSTATUS {
    // Desktop D3D12 has no Xbox firmware receiver. The original DLL's missing
    // device call sets last error on every attempt, including repeated calls.
    static UNAVAILABLE: Once = Once::new();
    crate::diagnostics::log_once(&UNAVAILABLE, || {
        let lengths = request.images.map(|image| image.bytes);
        format!(
            "D3D12UploadCustomMicrocodeX selector={:#010x} image_bytes={lengths:?} returned E_FAIL (no firmware receiver)",
            request.selector
        )
    });
    // SAFETY: SetLastError updates only the calling thread's Win32 error slot.
    unsafe { SetLastError(ERROR_INVALID_HANDLE) };
    STATUS_UNSUCCESSFUL
}

#[unsafe(export_name = "D3D12UploadCustomMicrocodeX")]
pub extern "system" fn upload_custom_microcode(
    selector: u32,
    first_bytes: usize,
    first: *const u8,
    second_bytes: usize,
    second: *const u8,
    third_bytes: usize,
    third: *const u8,
) -> HRESULT {
    submit(
        selector,
        [
            (first_bytes, first),
            (second_bytes, second),
            (third_bytes, third),
        ],
        desktop_driver,
    )
}

#[cfg(test)]
#[path = "tests/microcode.rs"]
mod tests;
