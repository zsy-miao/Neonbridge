//! DXR builds whose destination or scratch lies in a buffer the title placed
//! in its own memory through device slot 133 (Xbox2Windows 641be458, MIT,
//! re-derived). The title hands the build its own addresses, which are not
//! host GPU addresses.
//! - Scratch resolves to the placed buffer's host address plus the offset,
//!   after checking the prebuild scratch size fits inside that one buffer.
//! - A destination cannot alias title memory: a host acceleration structure
//!   must live in a buffer created in that state. Each title destination gets
//!   a private committed buffer, reused for later builds, updates and copies at
//!   the same address. The guest-to-native publication every build already
//!   records lets instances and views find it.
//!
//! - Copies (clone, compact, deserialize) to or from a title address use the
//!   same private buffer; a copy that cannot be resolved is logged and
//!   skipped instead of failing the list (TUXBOX_AS_COPY_SKIP=0 fails it).
//!
//! TUXBOX_PLACED_DXR=0 passes title addresses through as before.
use super::{guest, lock, rebuild};
use std::collections::BTreeMap;
use std::sync::Mutex;
use windows::Win32::Foundation::E_INVALIDARG;
use windows::Win32::Graphics::Direct3D12::*;
use windows::core::{IUnknown, Interface, Result};

pub(super) type Targets = Mutex<BTreeMap<u64, ID3D12Resource>>;

pub(crate) fn enabled() -> bool {
    crate::env_toggle!("TUXBOX_PLACED_DXR") != Some("0")
}

const ALIGNMENT: u64 = D3D12_RAYTRACING_ACCELERATION_STRUCTURE_BYTE_ALIGNMENT as u64;
const PLACEMENT: u64 = D3D12_DEFAULT_RESOURCE_PLACEMENT_ALIGNMENT as u64;

/// None reuses a target of `existing` bytes; otherwise the capacity to create.
fn capacity(existing: Option<u64>, needed: u64) -> Option<Option<u64>> {
    match existing {
        Some(held) if held >= needed => Some(None),
        _ => needed.max(1).checked_next_multiple_of(PLACEMENT).map(Some),
    }
}

/// A title placed buffer covering `[address, address + bytes)`: its host
/// resource and the host address of `address`. None: no placed buffer holds
/// the address (a native address, passed through).
unsafe fn placed(owner: &IUnknown, address: u64, bytes: u64) -> Result<Option<(ID3D12Resource, u64)>> {
    match unsafe { crate::device::command_buffer_address(owner.as_raw(), address, bytes.max(1)) }? {
        (Some(buffer), host) => Ok(Some((buffer, host))),
        (None, _) => Ok(None),
    }
}

pub(crate) fn copy_skip() -> bool {
    crate::env_toggle!("TUXBOX_AS_COPY_SKIP") != Some("0")
}

/// Whether a title placed buffer holds `address`.
pub(crate) unsafe fn is_title(owner: &IUnknown, address: u64) -> bool {
    enabled() && matches!(unsafe { placed(owner, address, 1) }, Ok(Some(_)))
}

/// The private destination for a title address inside a slot-133 buffer:
/// `[address, address + guest_bytes)` must lie in one placed buffer and the
/// private buffer holds at least `host_bytes`. None when the address is not
/// title placed memory.
pub(crate) unsafe fn destination(owner: &IUnknown, address: u64, guest_bytes: u64, host_bytes: u64) -> Result<Option<ID3D12Resource>> {
    if !enabled() || !address.is_multiple_of(ALIGNMENT) {
        return Ok(None);
    }
    let bytes = host_bytes.max(guest_bytes);
    if unsafe { placed(owner, address, guest_bytes) }?.is_none() {
        return Ok(None);
    }
    let device = unsafe { guest(owner) };
    let mut targets = lock(&device.state.blas.placed);
    let existing = targets.get(&address);
    let Some(create) = capacity(existing.map(|t| unsafe { t.GetDesc() }.Width), bytes) else {
        return Err(E_INVALIDARG.into());
    };
    let Some(capacity) = create else { return Ok(existing.cloned()) };
    if let Some(old) = existing {
        // Recorded lists keep the old buffer alive; later ones see the new one.
        crate::diagnostics::log_line(format_args!(
            "placed DXR destination {address:#x} grows from {} to {capacity} bytes", unsafe { old.GetDesc() }.Width));
    }
    let target = rebuild::as_buffer(&device.native, capacity)?;
    targets.insert(address, target.clone());
    static FIRST: std::sync::Once = std::sync::Once::new();
    crate::diagnostics::log_once(&FIRST, || format!(
        "placed DXR destination {address:#x} ({bytes} bytes) built in a private {capacity}-byte buffer (TUXBOX_PLACED_DXR=0 passes it through)"));
    Ok(Some(target))
}

/// The private buffer already serving a title destination (an update source).
pub(crate) unsafe fn existing(owner: &IUnknown, address: u64) -> Option<ID3D12Resource> {
    if !enabled() {
        return None;
    }
    lock(&unsafe { guest(owner) }.state.blas.placed).get(&address).cloned()
}

/// Scratch at a title address: the placed buffer and its host address. The
/// whole prebuild size must fit in one buffer created for unordered access.
pub(crate) unsafe fn scratch(owner: &IUnknown, address: u64, bytes: u64) -> Result<Option<(ID3D12Resource, u64)>> {
    if !enabled() || address == 0 || bytes == 0 {
        return Ok(None);
    }
    let Some((buffer, host)) = unsafe { placed(owner, address, bytes) }? else { return Ok(None) };
    let flags = unsafe { buffer.GetDesc() }.Flags;
    if !scratch_usable(host, flags) {
        crate::diagnostics::log_line(format_args!(
            "placed DXR scratch {address:#x} ({bytes} bytes) refused: host address {host:#x}, flags {:#x}", flags.0));
        return Err(E_INVALIDARG.into());
    }
    Ok(Some((buffer, host)))
}

fn scratch_usable(host: u64, flags: D3D12_RESOURCE_FLAGS) -> bool {
    host.is_multiple_of(ALIGNMENT) && flags.contains(D3D12_RESOURCE_FLAG_ALLOW_UNORDERED_ACCESS)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn a_destination_reuses_a_large_enough_target_and_grows_otherwise() {
        assert_eq!(capacity(None, 2304), Some(Some(PLACEMENT)));
        assert_eq!(capacity(Some(PLACEMENT), 2048), Some(None));
        assert_eq!(capacity(Some(PLACEMENT), PLACEMENT + 1), Some(Some(2 * PLACEMENT)));
        assert_eq!(capacity(None, u64::MAX), None);
    }

    #[test]
    fn scratch_needs_alignment_and_unordered_access() {
        assert!(scratch_usable(0x1_0000_0300, D3D12_RESOURCE_FLAG_ALLOW_UNORDERED_ACCESS));
        assert!(!scratch_usable(0x1_0000_0301, D3D12_RESOURCE_FLAG_ALLOW_UNORDERED_ACCESS));
        assert!(!scratch_usable(0x1_0000_0300, D3D12_RESOURCE_FLAG_NONE));
    }
}
