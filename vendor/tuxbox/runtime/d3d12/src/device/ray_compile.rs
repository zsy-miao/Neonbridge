//! Host DXR compilation can exhaust the 1 MiB stack of a title's render worker.
//! Keep the D3D12 call synchronous while providing the host compiler its own stack.
use windows::Win32::Foundation::{E_FAIL, E_OUTOFMEMORY};
use windows::Win32::Graphics::Direct3D12::*;
use windows::core::{HRESULT, Interface, Result};

struct Description<'a>(&'a D3D12_STATE_OBJECT_DESC);

// SAFETY: compile() requires all nested descriptions, strings and bytecode to
// remain readable until return. The scoped worker only reads them. D3D12 device,
// root signature and state-object interfaces support calls from multiple threads.
unsafe impl Send for Description<'_> {}

impl Description<'_> {
    unsafe fn compile(
        self,
        device: &ID3D12Device5,
        previous: Option<&ID3D12StateObject>,
    ) -> Result<ID3D12StateObject> {
        if let Some(previous) = previous {
            unsafe {
                device
                    .cast::<ID3D12Device7>()?
                    .AddToStateObject(self.0, previous)
            }
        } else {
            unsafe { device.CreateStateObject(self.0) }
        }
    }
}

/// All pointers reachable through desc must remain valid for synchronous native
/// state-object creation, and all referenced interfaces must belong to device.
pub(super) unsafe fn compile(
    device: &ID3D12Device5,
    desc: &D3D12_STATE_OBJECT_DESC,
    previous: Option<&ID3D12StateObject>,
) -> Result<ID3D12StateObject> {
    let borrowed = Description(desc);
    std::thread::scope(|scope| {
        let worker = std::thread::Builder::new()
            .name("TuxBox DXR compiler".into())
            .stack_size(16 * 1024 * 1024)
            .spawn_scoped(scope, move || unsafe { borrowed.compile(device, previous) })
            .map_err(|error| {
                error
                    .raw_os_error()
                    .map_or(E_OUTOFMEMORY, |code| HRESULT::from_win32(code as u32))
            })?;
        worker.join().map_err(|_| E_FAIL)?
    })
}
