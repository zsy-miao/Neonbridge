//! Buffer views whose records cannot start on a host placement boundary.
//!
//! Resources in a heap opened from title memory must allow cross-adapter
//! sharing, which pins their placement to 64 KiB on desktop drivers, and a
//! structured view can only advance by whole records. A record that starts
//! between those boundaries is read from a device-local copy instead. Each
//! queue submission first copies every live shadow from the title pages, so
//! the view observes CPU writes and earlier GPU work. GPU writes recorded
//! earlier in the same submission are not observed.
//!
//! An unordered access view's copy is written back to the title pages after
//! each submission, which replaces any other write to those bytes in it.
//!
//! Constant buffers use the same copies: Xbox places them on 16-byte
//! boundaries, desktop D3D12 needs 256 and removes the device otherwise.
use super::TextureAddress;
use crate::device::Device;
use std::collections::HashMap;
use std::sync::{Arc, Mutex, MutexGuard, Weak};
use windows::Win32::Foundation::E_POINTER;
use windows::Win32::Graphics::Direct3D12::*;
use windows::Win32::Graphics::Dxgi::Common::DXGI_SAMPLE_DESC;
use windows::core::{Interface, Result};

struct Entry {
    /// (title pages, offset in them, offset in the copy, bytes).
    pieces: Vec<(ID3D12Resource, u64, u64, u64)>,
    writable: bool,
    view: Weak<TextureAddress>,
}

struct Recorder {
    kind: D3D12_COMMAND_LIST_TYPE,
    allocator: ID3D12CommandAllocator,
    list: ID3D12GraphicsCommandList,
    fence: ID3D12Fence,
    value: u64,
    // ExecuteCommandLists does not retain resources until completion.
    held: Vec<(ID3D12Resource, ID3D12Resource)>,
}

#[derive(Default)]
pub(crate) struct Shadows {
    entries: Mutex<HashMap<(u64, u64, bool), Entry>>,
    recorders: Mutex<Vec<Recorder>>,
}

fn lock<T>(value: &Mutex<T>) -> MutexGuard<'_, T> {
    value.lock().unwrap_or_else(|e| e.into_inner())
}

fn buffer(width: u64) -> D3D12_RESOURCE_DESC {
    D3D12_RESOURCE_DESC {
        Dimension: D3D12_RESOURCE_DIMENSION_BUFFER, Width: width, Height: 1, DepthOrArraySize: 1,
        MipLevels: 1, Layout: D3D12_TEXTURE_LAYOUT_ROW_MAJOR,
        SampleDesc: DXGI_SAMPLE_DESC { Count: 1, Quality: 0 }, ..Default::default()
    }
}

/// A view of `bytes` at `address` whose resource begins exactly at `address`.
pub(super) fn view(device: &Device, address: u64, bytes: u64) -> Result<Arc<TextureAddress>> {
    copy_of(device, address, bytes, false)
}

/// Like [`view`], for unordered access: the copy is written back.
pub(super) fn writable_view(device: &Device, address: u64, bytes: u64) -> Result<Arc<TextureAddress>> {
    copy_of(device, address, bytes, true)
}

fn copy_of(device: &Device, address: u64, bytes: u64, writable: bool) -> Result<Arc<TextureAddress>> {
    let shadows = &device.state.shadows;
    let mut entries = lock(&shadows.entries);
    if let Some(view) = entries.get(&(address, bytes, writable)).and_then(|e| e.view.upgrade()) {
        return Ok(view);
    }
    let page = address & !0xffff;
    let width = (address - page + bytes).next_multiple_of(65536);
    let pieces = match crate::device::heap::import_buffer(device, page, &buffer(width), D3D12_RESOURCE_STATE_COMMON) {
        Ok(source) => vec![(source, address - page, 0, bytes)],
        // The range spans separately mapped title pages that no single heap can
        // open; copy each 64 KiB page through its own import.
        Err(e) if e.code() == windows::Win32::Foundation::E_INVALIDARG => {
            let mut pieces = Vec::new();
            for at in (page..address + bytes).step_by(65536) {
                let source = crate::device::heap::import_buffer(device, at, &buffer(65536), D3D12_RESOURCE_STATE_COMMON)?;
                let (start, end) = (address.max(at), (address + bytes).min(at + 65536));
                pieces.push((source, start - at, start - address, end - start));
            }
            pieces
        }
        Err(e) => return Err(e),
    };
    let heap = D3D12_HEAP_PROPERTIES { Type: D3D12_HEAP_TYPE_DEFAULT, CreationNodeMask: 1, VisibleNodeMask: 1, ..Default::default() };
    let mut copy: Option<ID3D12Resource> = None;
    unsafe {
        // Whole 256-byte units, so a constant buffer view of it is valid.
        device.native.CreateCommittedResource(&heap, D3D12_HEAP_FLAG_NONE, &D3D12_RESOURCE_DESC {
                Flags: if writable { D3D12_RESOURCE_FLAG_ALLOW_UNORDERED_ACCESS } else { D3D12_RESOURCE_FLAG_NONE },
                ..buffer(bytes.next_multiple_of(256))
            },
            D3D12_RESOURCE_STATE_COMMON, None, &mut copy)?;
    }
    let copy = copy.ok_or(E_POINTER)?;
    // Not registered by address: CBVs, UAVs and command arguments at this VA
    // must keep reaching the title pages, never the copy.
    let view = TextureAddress::unlisted(device, &copy, address as usize);
    entries.retain(|_, e| e.view.strong_count() > 0);
    entries.insert((address, bytes, writable), Entry { pieces, writable, view: Arc::downgrade(&view) });
    static FIRST: std::sync::Once = std::sync::Once::new();
    crate::diagnostics::log_once(&FIRST, || format!("shadowed buffer view at {address:#x}, {bytes} bytes"));
    Ok(view)
}

/// Copy every live shadow from title memory ahead of this queue's work.
pub(crate) unsafe fn refresh(device: &Device, queue: &ID3D12CommandQueue) -> Result<()> {
    unsafe { transfer(device, queue, false) }
}

/// Copy every writable shadow back to title memory after this queue's work.
pub(crate) unsafe fn write_back(device: &Device, queue: &ID3D12CommandQueue) -> Result<()> {
    unsafe { transfer(device, queue, true) }
}

unsafe fn transfer(device: &Device, queue: &ID3D12CommandQueue, back: bool) -> Result<()> {
    let shadows = &device.state.shadows;
    let kind = unsafe { queue.GetDesc() }.Type;
    if kind != D3D12_COMMAND_LIST_TYPE_DIRECT && kind != D3D12_COMMAND_LIST_TYPE_COMPUTE {
        return Ok(());
    }
    let mut copies = Vec::new();
    {
        let mut entries = lock(&shadows.entries);
        entries.retain(|_, e| e.view.strong_count() > 0);
        for entry in entries.values().filter(|e| !back || e.writable) {
            if let Some(view) = entry.view.upgrade() {
                for (source, offset, at, bytes) in &entry.pieces {
                    copies.push((source.clone(), *offset, *at, *bytes, view.clone()));
                }
            }
        }
    }
    if copies.is_empty() {
        return Ok(());
    }
    let mut recorders = lock(&shadows.recorders);
    let ready = recorders.iter().position(|r| r.kind == kind && unsafe { r.fence.GetCompletedValue() } >= r.value);
    let mut recorder = match ready {
        Some(index) => {
            let r = recorders.swap_remove(index);
            let mut r = r;
            r.held.clear();
            unsafe { r.allocator.Reset()?; r.list.Reset(&r.allocator, None)?; }
            r
        }
        None => unsafe {
            let allocator: ID3D12CommandAllocator = device.native.CreateCommandAllocator(kind)?;
            let list: ID3D12GraphicsCommandList = device.native.CreateCommandList(0, kind, &allocator, None)?;
            let fence: ID3D12Fence = device.native.CreateFence(0, D3D12_FENCE_FLAG_NONE)?;
            Recorder { kind, allocator, list, fence, value: 0, held: Vec::new() }
        },
    };
    for (source, offset, at, bytes, view) in &copies {
        if back {
            unsafe { recorder.list.CopyBufferRegion(source, *offset, &view.native, *at, *bytes) };
        } else {
            unsafe { recorder.list.CopyBufferRegion(&view.native, *at, source, *offset, *bytes) };
        }
        recorder.held.push((source.clone(), view.native.clone()));
    }
    unsafe {
        recorder.list.Close()?;
        queue.ExecuteCommandLists(&[Some(recorder.list.cast()?)]);
        crate::dred::check(queue, if back { "shadow write-back" } else { "shadow refresh" });
        recorder.value += 1;
        queue.Signal(&recorder.fence, recorder.value)?;
    }
    recorders.push(recorder);
    Ok(())
}
