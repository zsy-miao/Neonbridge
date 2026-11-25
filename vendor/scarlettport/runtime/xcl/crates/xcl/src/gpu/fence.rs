//! Completion checks for XCL-owned D3D12 fences.

use std::fmt;
use std::time::Instant;

use windows::core::Error;
use windows::Win32::Foundation::{CloseHandle, HANDLE, WAIT_FAILED, WAIT_OBJECT_0, WAIT_TIMEOUT};
use windows::Win32::Graphics::Direct3D12::{ID3D12Device, ID3D12Fence};
use windows::Win32::Graphics::Dxgi::DXGI_ERROR_DEVICE_REMOVED;
use windows::Win32::System::Threading::{CreateEventW, WaitForSingleObject};

#[derive(Debug)]
pub(crate) enum FenceError {
    Removed(Error),
    Timeout(u32),
    Api(&'static str, Error),
    UnexpectedWait(u32),
}

impl fmt::Display for FenceError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::Removed(e) => write!(f, "D3D12 device removed ({:#010x}: {})", e.code().0 as u32, e.message()),
            Self::Timeout(ms) => write!(f, "GPU fence still pending after {ms} ms"),
            Self::Api(op, e) => write!(f, "{op} failed: {e}"),
            Self::UnexpectedWait(status) => write!(f, "WaitForSingleObject returned {status:#x}"),
        }
    }
}

pub(crate) unsafe fn api_error(dev: &ID3D12Device, op: &'static str, error: Error) -> FenceError {
    dev.GetDeviceRemovedReason().err().map_or(FenceError::Api(op, error), FenceError::Removed)
}

pub(crate) unsafe fn completed_value(fence: &ID3D12Fence, dev: &ID3D12Device) -> Result<u64, FenceError> {
    let value = fence.GetCompletedValue();
    // Removal signals monitored fences to UINT64_MAX and can wake their waiters.
    if value == u64::MAX {
        let reason = dev.GetDeviceRemovedReason().err().unwrap_or_else(|| Error::from_hresult(DXGI_ERROR_DEVICE_REMOVED));
        return Err(FenceError::Removed(reason));
    }
    Ok(value)
}

struct Event(HANDLE);

impl Drop for Event {
    fn drop(&mut self) {
        unsafe {
            let _ = CloseHandle(self.0);
        }
    }
}

/// Wait for a valid completion sample. A reused event can also wake for an earlier registration.
pub(crate) unsafe fn wait(
    fence: &ID3D12Fence, target: u64, dev: &ID3D12Device, event: Option<HANDLE>, timeout_ms: u32,
) -> Result<(), FenceError> {
    if completed_value(fence, dev)? >= target {
        return Ok(());
    }
    let owned;
    let event = match event {
        Some(event) => event,
        None => {
            owned = Event(CreateEventW(None, false, false, None).map_err(|e| api_error(dev, "CreateEventW", e))?);
            owned.0
        }
    };
    if let Err(e) = fence.SetEventOnCompletion(target, event) {
        completed_value(fence, dev)?;
        return Err(api_error(dev, "SetEventOnCompletion", e));
    }
    let start = Instant::now();
    let mut remaining = timeout_ms;
    loop {
        let status = WaitForSingleObject(event, remaining);
        let error = (status == WAIT_FAILED).then(Error::from_thread);
        let completed = completed_value(fence, dev)?;
        if let Some(e) = error {
            return Err(api_error(dev, "WaitForSingleObject", e));
        }
        if status != WAIT_OBJECT_0 && status != WAIT_TIMEOUT {
            return Err(FenceError::UnexpectedWait(status.0));
        }
        if completed >= target {
            return Ok(());
        }
        let elapsed = start.elapsed().as_millis();
        if status == WAIT_TIMEOUT || elapsed >= u128::from(timeout_ms) {
            return Err(dev.GetDeviceRemovedReason().err().map_or(FenceError::Timeout(timeout_ms), FenceError::Removed));
        }
        remaining = timeout_ms - elapsed as u32;
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::cell::Cell;
    use std::ffi::c_void;
    use std::mem::{offset_of, size_of};
    use windows::core::{Interface, HRESULT};
    use windows::Win32::Foundation::{E_OUTOFMEMORY, S_OK};
    use windows::Win32::Graphics::Direct3D12::{ID3D12Device_Vtbl, ID3D12Fence_Vtbl};
    use windows::Win32::Graphics::Dxgi::DXGI_ERROR_DEVICE_HUNG;
    use windows::Win32::System::Threading::SetEvent;

    #[repr(C)]
    struct MockFence {
        vtable: *const usize,
        value: Cell<u64>,
        on_arm: Option<(u64, bool)>,
        arm_result: HRESULT,
        arms: Cell<u32>,
    }

    #[repr(C)]
    struct MockDevice {
        vtable: *const usize,
        reason: HRESULT,
    }

    unsafe extern "system" fn value(this: *mut c_void) -> u64 {
        (*this.cast::<MockFence>()).value.get()
    }

    unsafe extern "system" fn arm(this: *mut c_void, _target: u64, event: HANDLE) -> HRESULT {
        let fence = &*this.cast::<MockFence>();
        fence.arms.set(fence.arms.get() + 1);
        if let Some((value, wake)) = fence.on_arm {
            fence.value.set(value);
            if wake {
                let _ = SetEvent(event);
            }
        }
        fence.arm_result
    }

    unsafe extern "system" fn reason(this: *mut c_void) -> HRESULT {
        (*this.cast::<MockDevice>()).reason
    }

    extern "system" fn unexpected_call() {
        std::process::abort();
    }

    fn mock_wait(
        initial: u64, target: u64, on_arm: Option<(u64, bool)>, arm_result: HRESULT, removal_reason: HRESULT,
    ) -> (Result<(), FenceError>, u32) {
        // Back the full vtables and implement the three methods used by wait.
        const SLOT: usize = size_of::<usize>();
        let mut fence_vtable = [unexpected_call as usize; size_of::<ID3D12Fence_Vtbl>() / SLOT];
        fence_vtable[offset_of!(ID3D12Fence_Vtbl, GetCompletedValue) / SLOT] = value as usize;
        fence_vtable[offset_of!(ID3D12Fence_Vtbl, SetEventOnCompletion) / SLOT] = arm as usize;
        let mut device_vtable = [unexpected_call as usize; size_of::<ID3D12Device_Vtbl>() / SLOT];
        device_vtable[offset_of!(ID3D12Device_Vtbl, GetDeviceRemovedReason) / SLOT] = reason as usize;
        let mut mock_fence = MockFence {
            vtable: fence_vtable.as_ptr(),
            value: Cell::new(initial),
            on_arm,
            arm_result,
            arms: Cell::new(0),
        };
        let mut mock_device = MockDevice { vtable: device_vtable.as_ptr(), reason: removal_reason };
        let fence_raw = std::ptr::from_mut(&mut mock_fence).cast();
        let device_raw = std::ptr::from_mut(&mut mock_device).cast();
        let result = unsafe {
            let fence = ID3D12Fence::from_raw_borrowed(&fence_raw).unwrap();
            let dev = ID3D12Device::from_raw_borrowed(&device_raw).unwrap();
            wait(fence, target, dev, None, 0)
        };
        (result, mock_fence.arms.get())
    }

    #[test]
    fn completed_fence_skips_event_registration() {
        for value in [7, 8, u64::MAX - 1] {
            let (result, arms) = mock_wait(value, 7, None, E_OUTOFMEMORY, S_OK);
            assert!(result.is_ok());
            assert_eq!(arms, 0);
        }
    }

    #[test]
    fn removal_precedes_completion_even_for_zero_target() {
        let (result, arms) = mock_wait(u64::MAX, 0, None, S_OK, DXGI_ERROR_DEVICE_HUNG);
        assert!(matches!(result, Err(FenceError::Removed(e)) if e.code() == DXGI_ERROR_DEVICE_HUNG));
        assert_eq!(arms, 0);

        let (result, _) = mock_wait(u64::MAX, 7, None, S_OK, S_OK);
        assert!(matches!(result, Err(FenceError::Removed(e)) if e.code() == DXGI_ERROR_DEVICE_REMOVED));
    }

    #[test]
    fn event_wake_requires_a_valid_completed_value() {
        let (result, arms) = mock_wait(6, 7, Some((7, true)), S_OK, S_OK);
        assert!(result.is_ok());
        assert_eq!(arms, 1);

        let (result, arms) = mock_wait(6, 7, Some((u64::MAX, true)), S_OK, DXGI_ERROR_DEVICE_HUNG);
        assert!(matches!(result, Err(FenceError::Removed(e)) if e.code() == DXGI_ERROR_DEVICE_HUNG));
        assert_eq!(arms, 1);

        // An event can already be signalled for an earlier wait on the reused handle.
        let (result, _) = mock_wait(6, 7, Some((6, true)), S_OK, S_OK);
        assert!(matches!(result, Err(FenceError::Timeout(0))));
    }

    #[test]
    fn timeout_resamples_the_fence() {
        let (result, _) = mock_wait(6, 7, Some((7, false)), S_OK, S_OK);
        assert!(result.is_ok());

        let (result, _) = mock_wait(6, 7, Some((u64::MAX, false)), S_OK, S_OK);
        assert!(matches!(result, Err(FenceError::Removed(e)) if e.code() == DXGI_ERROR_DEVICE_REMOVED));
    }

    #[test]
    fn pending_and_registration_failure_keep_their_own_errors() {
        let (result, arms) = mock_wait(6, 7, None, S_OK, S_OK);
        assert!(matches!(result, Err(FenceError::Timeout(0))));
        assert_eq!(arms, 1);

        let (result, _) = mock_wait(6, 7, None, E_OUTOFMEMORY, S_OK);
        assert!(matches!(result, Err(FenceError::Api("SetEventOnCompletion", e)) if e.code() == E_OUTOFMEMORY));

        let (result, _) = mock_wait(6, 7, Some((u64::MAX, true)), E_OUTOFMEMORY, DXGI_ERROR_DEVICE_HUNG);
        assert!(matches!(result, Err(FenceError::Removed(e)) if e.code() == DXGI_ERROR_DEVICE_HUNG));
    }
}
