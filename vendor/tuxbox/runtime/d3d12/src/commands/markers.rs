//! Values a command list writes to title memory.
use super::*;

// Write32BitValueBottomOfPipeX: store a value once earlier GPU work finishes.
// The flush argument selects console caches; the desktop driver manages its own.
pub(super) unsafe extern "system" fn write_marker(list: &List, address: u64, value: u32, _flush: u32, flags: u32) {
    let mut state = recording(list);
    if !state.ensure_open(&list.native) {
        // Keep what a failed list would have written, to explain a stall.
        state.labels.push(memory_wait::Label::write(address, u64::from(value)));
        return;
    }
    let result = if flags != 0 || address == 0 || !address.is_multiple_of(4) {
        Err(E_INVALIDARG.into())
    } else {
        write_dword(list, &mut state, address, value)
            .map(|()| {
                state.labels.push(memory_wait::Label::write(address, u64::from(value)));
                memory_wait::trace_write("Write32BitValueBottomOfPipeX", address, 4, u64::from(value));
            })
    };
    if let Err(error) = result {
        crate::diagnostics::recording_error(format_args!(
            "Write32BitValueBottomOfPipeX address={address:#x} value={value:#x} flags={flags:#x}"
        ));
        state.error = error.code();
    }
}

// Write64BitValueBottomOfPipeX: a GPU timestamp, or a value such as the fence
// labels other queues wait on with Wait64BitValueX.
pub(super) unsafe extern "system" fn write_value64(list: &List, address: u64, value: u64, flush: u32, flags: u32) {
    let mut state = recording(list);
    if !state.ensure_open(&list.native) {
        // Keep what a failed list would have written, to explain a stall.
        state.labels.push(memory_wait::Label::write(address, value));
        return;
    }
    let result = if address == 0 || !address.is_multiple_of(8) {
        Err(E_INVALIDARG.into())
    } else if flags == BOP_GPU_TIMESTAMP {
        queries::timestamp(list, &mut state, address)
    } else if flags != 0 {
        Err(E_NOTIMPL.into())
    } else {
        // Desktop immediate writes are 32 bits wide. Store the high half first,
        // so a counter below 2^32 changes in one step.
        write_dword(list, &mut state, address + 4, (value >> 32) as u32)
            .and_then(|()| write_dword(list, &mut state, address, value as u32))
            .map(|()| {
                state.labels.push(memory_wait::Label::write(address, value));
                memory_wait::trace_write("Write64BitValueBottomOfPipeX", address, 8, value);
            })
    };
    if let Err(error) = result {
        crate::diagnostics::recording_error(format_args!(
            "Write64BitValueBottomOfPipeX address={address:#x} value={value:#x} flush={flush:#x} flags={flags:#x}"
        ));
        state.error = error.code();
    }
}

pub(super) fn write_dword(list: &List, state: &mut Recording, address: u64, value: u32) -> windows::core::Result<()> {
    let (resource, dest) = unsafe { device::command_buffer_address(parent(&list.parent), address, 4)? };
    if let Some(buffer) = &resource { raytracing::retain(state, &[buffer])?; }
    let write = D3D12_WRITEBUFFERIMMEDIATE_PARAMETER { Dest: dest, Value: value };
    state.prefix.push(&list.native, memory_wait::Packet::Marker(write))
}
