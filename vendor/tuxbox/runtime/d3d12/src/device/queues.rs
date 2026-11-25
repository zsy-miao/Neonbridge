//! Creating command queues and command lists.
use super::*;

pub(super) unsafe fn make_queue(this: &Device, desc: &D3D12_COMMAND_QUEUE_DESC, out: Out) -> HRESULT {
    match unsafe { this.native.CreateCommandQueue(desc) } {
        Ok(native) => unsafe { wrap_command_queue(native, &identity(this), this.state.device11_queue, out) },
        Err(error) => error.code(),
    }
}

pub(super) fn supported_queue_kind(kind: D3D12_COMMAND_LIST_TYPE) -> bool {
    matches!(
        kind,
        D3D12_COMMAND_LIST_TYPE_DIRECT
            | D3D12_COMMAND_LIST_TYPE_COMPUTE
            | D3D12_COMMAND_LIST_TYPE_COPY
    )
}

pub(super) fn unsupported_queue(slot: u32, words: [u32; 4]) -> HRESULT {
    use windows::Win32::System::Diagnostics::Debug::OutputDebugStringA;
    use windows::core::PCSTR;
    let message = format!(
        "Project TuxBox: unsupported queue descriptor at slot {slot}, words {words:x?}\n\0"
    );
    unsafe { OutputDebugStringA(PCSTR(message.as_ptr())) };
    E_NOTIMPL
}

pub(super) unsafe extern "system" fn create_queue(
    this: &Device,
    desc: *const D3D12_COMMAND_QUEUE_DESC,
    iid: *const GUID,
    out: Out,
) -> HRESULT {
    let status = unsafe { check_output::<ID3D12CommandQueue>(iid, out) };
    if status.is_err() {
        return status;
    }
    let Some(desc) = (unsafe { desc.as_ref() }) else {
        return E_INVALIDARG;
    };
    if !supported_queue_kind(desc.Type)
        || !matches!(
            D3D12_COMMAND_QUEUE_PRIORITY(desc.Priority),
            D3D12_COMMAND_QUEUE_PRIORITY_NORMAL
                | D3D12_COMMAND_QUEUE_PRIORITY_HIGH
                | D3D12_COMMAND_QUEUE_PRIORITY_GLOBAL_REALTIME
        )
        || !matches!(
            desc.Flags,
            D3D12_COMMAND_QUEUE_FLAG_NONE | D3D12_COMMAND_QUEUE_FLAG_DISABLE_GPU_TIMEOUT
        )
        || desc.NodeMask > 1
    {
        return unsupported_queue(
            8,
            [
                desc.Type.0 as u32,
                desc.Priority as u32,
                desc.Flags.0 as u32,
                desc.NodeMask,
            ],
        );
    }
    unsafe { make_queue(this, desc, out) }
}

// Slot 48's field order differs from the desktop queue descriptor.
#[repr(C)]
pub(super) struct ComputeQueueDesc {
    pub(super) kind: D3D12_COMMAND_LIST_TYPE,
    pub(super) options: u32,
    pub(super) queue_index: u32,
    pub(super) engine_index: u32,
}

pub(super) unsafe extern "system" fn create_queue_x(
    this: &Device,
    desc: *const ComputeQueueDesc,
    iid: *const GUID,
    out: Out,
) -> HRESULT {
    let status = unsafe { check_output::<ID3D12CommandQueue>(iid, out) };
    if status.is_err() {
        return status;
    }
    let Some(desc) = (unsafe { desc.as_ref() }) else {
        return E_INVALIDARG;
    };
    if desc.kind != D3D12_COMMAND_LIST_TYPE_COMPUTE
        || desc.options != 0
        || desc.queue_index > 3
    {
        return unsupported_queue(
            48,
            [
                desc.kind.0 as u32,
                desc.options,
                desc.queue_index,
                desc.engine_index,
            ],
        );
    }
    let native = D3D12_COMMAND_QUEUE_DESC {
        Type: D3D12_COMMAND_LIST_TYPE_COMPUTE,
        // The guest selects a logical compute queue on this adapter. Each
        // creation already returns a separate host command queue.
        ..Default::default()
    };
    unsafe { make_queue(this, &native, out) }
}

#[repr(C)]
pub(super) struct ListDesc {
    pub(super) kind: D3D12_COMMAND_LIST_TYPE,
    pub(super) reserved1: u32,
    pub(super) node_mask: u32,
    pub(super) reserved3: u32,
}

pub(super) unsafe extern "system" fn create_list_x(
    this: &Device,
    desc: *const ListDesc,
    guest_allocator: *mut c_void,
    initial: *mut c_void,
    iid: *const GUID,
    out: Out,
) -> HRESULT {
    let compact = unsafe { iid.as_ref() } == Some(&media::DMA_IID);
    let extended = unsafe { iid.as_ref() } == Some(&crate::commands::LIST6);
    let host_iid = ID3D12GraphicsCommandList::IID;
    let status = unsafe {
        check_output::<ID3D12GraphicsCommandList>(if compact || extended { &host_iid } else { iid }, out)
    };
    if status.is_err() {
        return status;
    }
    if compact
        && unsafe { desc.as_ref() }.is_none_or(|desc| desc.kind != D3D12_COMMAND_LIST_TYPE_COPY)
    {
        return windows::Win32::Foundation::E_NOINTERFACE;
    }
    let parent = identity(this);
    let (Some(desc), Some(allocator)) =
        (unsafe { (desc.as_ref(), allocator(guest_allocator, parent.as_raw())) })
    else {
        return E_INVALIDARG;
    };
    // Electra's compact DMA list at RVA 0x19849c3 selects the default node
    // with {COPY,0,0,0}. Both default and explicit node one target node zero.
    let valid_node = desc.node_mask == 1 || (compact && desc.node_mask == 0);
    // GTA creates standard graphics lists with the console recording flags
    // 0x6. The host owns the command stream storage in either mode.
    let supported_flags = desc.reserved1 == 0 || (!compact && desc.reserved1 == 6)
        || (extended && desc.reserved1 == 2);
    if !supported_queue_kind(desc.kind) || !supported_flags || !valid_node || desc.reserved3 != 0
    {
        return E_NOTIMPL;
    }
    // SAFETY: the caller keeps its optional pipeline alive throughout creation.
    let initial = match unsafe { compute::pipeline(initial, parent.as_raw()) } {
        Ok(initial) => initial,
        Err(error) => return error,
    };
    let list: ID3D12GraphicsCommandList = match unsafe {
        this.native.CreateCommandList(
            desc.node_mask,
            desc.kind,
            &allocator.native,
            initial.map(|p| &p.native),
        )
    } {
        Ok(list) => list,
        Err(error) => return error.code(),
    };
    // Hold an empty native recording ready for a close-first guest, releasing
    // the allocator so reset-first callers can rewind it before their list.
    if let Err(error) = unsafe { list.Close() } {
        return error.code();
    }
    unsafe {
        wrap_command_list(
            list,
            allocator,
            initial,
            &parent,
            compact,
            extended,
            this.state.device11_queue,
            out,
        )
    }
}
