//! On-demand write-back of committed textures into the memory behind their
//! GPU address (palmetto e604c28a: the title reads console image addresses
//! on the CPU and expects what the GPU wrote there).
//!
//! Nothing is copied per submission. The backing pages stay no-access; the
//! first CPU touch faults, and this module then:
//! - waits (at most TUXBOX_WRITEBACK_WAIT_MS, default 500) for the work each
//!   guest queue has submitted so far,
//! - copies that one texture to a readback buffer on a private direct queue,
//!   moving it from the layout its last recorded barrier left it in,
//! - lays the texels out in the console layout the size answer assumed (XG
//!   plane; linear rows when no plane fits) and opens the pages read/write.
//!
//! After the next guest submission the opened pages are closed again, so the
//! next CPU touch reads what that submission wrote. Cost therefore follows
//! only the textures the CPU actually reads, once per submission at most.
//! CPU writes to these pages are not uploaded to the GPU and are lost at the
//! next re-read. The recorded layout is the last barrier recorded, which can
//! differ from execution order.
//!
//! Default off: TUXBOX_COMMITTED_WRITEBACK=1 turns it on.
use std::collections::{BTreeMap, HashMap};
use std::mem::ManuallyDrop;
use std::sync::{Mutex, MutexGuard, Once};
use windows::Win32::Foundation::{CloseHandle, E_FAIL, E_NOTIMPL, WAIT_OBJECT_0};
use windows::Win32::Graphics::Direct3D12::*;
use windows::Win32::Graphics::Dxgi::Common::DXGI_SAMPLE_DESC;
use windows::Win32::System::Diagnostics::Debug::{AddVectoredExceptionHandler, EXCEPTION_POINTERS};
use windows::Win32::System::Memory::{PAGE_NOACCESS, PAGE_PROTECTION_FLAGS, PAGE_READWRITE, VirtualProtect};
use windows::Win32::System::Threading::{CreateEventW, WaitForSingleObject};
use windows::core::{Interface, Result};

pub(crate) fn enabled() -> bool {
    static ON: std::sync::LazyLock<bool> =
        std::sync::LazyLock::new(|| crate::env_toggle!("TUXBOX_COMMITTED_WRITEBACK") == Some("1"));
    *ON
}

fn wait_ms() -> u32 {
    crate::env_toggle!("TUXBOX_WRITEBACK_WAIT_MS").and_then(|v| v.parse().ok()).unwrap_or(500)
}

struct Entry {
    base: usize,
    size: usize,
    native: ID3D12Resource,
    device: ID3D12Device,
    desc: D3D12_RESOURCE_DESC,
    layout: D3D12_BARRIER_LAYOUT,
    open: bool,
}

#[derive(Default)]
struct Table {
    /// Keyed by the native resource pointer.
    entries: HashMap<usize, Entry>,
    /// Backing base -> native resource pointer.
    bases: BTreeMap<usize, usize>,
    /// A private fence per guest queue and its last value.
    queues: HashMap<usize, (ID3D12Fence, u64)>,
    /// A private direct queue per device.
    readers: HashMap<usize, (ID3D12CommandQueue, ID3D12Fence, u64)>,
}

// SAFETY: D3D12 interfaces are free-threaded; the table is behind a mutex.
unsafe impl Send for Table {}

static TABLE: std::sync::LazyLock<Mutex<Table>> = std::sync::LazyLock::new(Default::default);

fn table() -> MutexGuard<'static, Table> {
    TABLE.lock().unwrap_or_else(|e| e.into_inner())
}

/// Watch the committed backing `[base, base + size)` of `native`. The pages
/// must be committed no-access.
pub(crate) fn register(device: &ID3D12Device, native: &ID3D12Resource, desc: &D3D12_RESOURCE_DESC, base: usize, size: usize) {
    static HANDLER: Once = Once::new();
    HANDLER.call_once(|| {
        if unsafe { AddVectoredExceptionHandler(1, Some(handler)) }.is_null() {
            crate::diagnostics::log_line(format_args!("committed write-back: no exception handler; pages stay no-access"));
        }
    });
    let key = native.as_raw() as usize;
    let mut t = table();
    t.bases.insert(base, key);
    t.entries.insert(key, Entry {
        base, size, native: native.clone(), device: device.clone(), desc: *desc,
        layout: D3D12_BARRIER_LAYOUT_COMMON, open: false,
    });
}

/// The backing at `base` is being released.
pub(crate) fn unregister(base: usize) {
    if !enabled() {
        return;
    }
    let mut t = table();
    if let Some(key) = t.bases.remove(&base) {
        t.entries.remove(&key);
    }
}

/// A recorded texture barrier left `native` in `layout`.
pub(crate) fn note_layout(native: &ID3D12Resource, layout: D3D12_BARRIER_LAYOUT) {
    if !enabled() || layout == D3D12_BARRIER_LAYOUT_UNDEFINED {
        return;
    }
    if let Some(entry) = table().entries.get_mut(&(native.as_raw() as usize)) {
        entry.layout = layout;
    }
}

/// After guest work went to `queue`: mark how far it got and close the pages
/// the CPU opened, so the next touch reads the new contents.
pub(crate) fn submitted(queue: &ID3D12CommandQueue) {
    if !enabled() {
        return;
    }
    let mut t = table();
    if t.entries.is_empty() {
        return;
    }
    let key = queue.as_raw() as usize;
    if !t.queues.contains_key(&key) {
        let mut device: Option<ID3D12Device> = None;
        let Ok(()) = (unsafe { queue.GetDevice(&mut device) }) else { return };
        let Some(Ok(fence)) = device.map(|d| unsafe { d.CreateFence::<ID3D12Fence>(0, D3D12_FENCE_FLAG_NONE) }) else { return };
        t.queues.insert(key, (fence, 0));
    }
    let (fence, value) = t.queues.get_mut(&key).expect("inserted above");
    *value += 1;
    let _ = unsafe { queue.Signal(&*fence, *value) };
    for entry in t.entries.values_mut().filter(|e| e.open) {
        let mut old = PAGE_PROTECTION_FLAGS::default();
        if unsafe { VirtualProtect(entry.base as _, entry.size, PAGE_NOACCESS, &mut old) }.is_ok() {
            entry.open = false;
        }
    }
}

unsafe extern "system" fn handler(info: *mut EXCEPTION_POINTERS) -> i32 {
    const CONTINUE_EXECUTION: i32 = -1;
    const CONTINUE_SEARCH: i32 = 0;
    let Some(record) = (unsafe { info.as_ref() }).and_then(|i| unsafe { i.ExceptionRecord.as_ref() }) else {
        return CONTINUE_SEARCH;
    };
    if record.ExceptionCode.0 as u32 != 0xC000_0005 || record.NumberParameters < 2 {
        return CONTINUE_SEARCH;
    }
    let address = record.ExceptionInformation[1];
    // Never block in a fault: a thread holding the table lock is ours.
    let Ok(mut t) = TABLE.try_lock() else { return CONTINUE_SEARCH };
    let Some((&base, &key)) = t.bases.range(..=address).next_back() else { return CONTINUE_SEARCH };
    let Some(entry) = t.entries.get(&key) else { return CONTINUE_SEARCH };
    if address - base >= entry.size || entry.open {
        return CONTINUE_SEARCH;
    }
    let (native, device, desc, layout, size) = (entry.native.clone(), entry.device.clone(), entry.desc, entry.layout, entry.size);
    let queues: Vec<(ID3D12Fence, u64)> = t.queues.values().cloned().collect();
    let reader = reader(&mut t, &device);
    drop(t);
    let mut old = PAGE_PROTECTION_FLAGS::default();
    if unsafe { VirtualProtect(base as _, size, PAGE_READWRITE, &mut old) }.is_err() {
        return CONTINUE_SEARCH;
    }
    let result = reader.and_then(|reader| {
        wait_for(&queues);
        let (queue, fence, value) = reader;
        unsafe { fill(&device, &queue, &fence, value, &native, &desc, layout, base, size) }
    });
    static COUNT: std::sync::atomic::AtomicU64 = std::sync::atomic::AtomicU64::new(0);
    let n = COUNT.fetch_add(1, std::sync::atomic::Ordering::Relaxed) + 1;
    if n <= 8 || n.is_power_of_two() || result.is_err() {
        crate::diagnostics::log_line(format_args!(
            "committed write-back: CPU touched {address:#x}; {}x{} format {} at {base:#x} read back: {:?} ({n} so far)",
            desc.Width, desc.Height, desc.Format.0, result.as_ref().map(|_| ()).map_err(|e| e.code())));
    }
    if let Some(entry) = table().entries.get_mut(&key) {
        entry.open = true;
    }
    CONTINUE_EXECUTION
}

/// The device's private direct queue, its fence and the next fence value.
fn reader(t: &mut Table, device: &ID3D12Device) -> Result<(ID3D12CommandQueue, ID3D12Fence, u64)> {
    let key = device.as_raw() as usize;
    if !t.readers.contains_key(&key) {
        let queue: ID3D12CommandQueue = unsafe { device.CreateCommandQueue(&D3D12_COMMAND_QUEUE_DESC {
            Type: D3D12_COMMAND_LIST_TYPE_DIRECT, ..Default::default()
        }) }?;
        let fence: ID3D12Fence = unsafe { device.CreateFence(0, D3D12_FENCE_FLAG_NONE) }?;
        t.readers.insert(key, (queue, fence, 0));
    }
    let (queue, fence, value) = t.readers.get_mut(&key).expect("inserted above");
    *value += 1;
    Ok((queue.clone(), fence.clone(), *value))
}

fn wait_on(fence: &ID3D12Fence, value: u64, ms: u32) -> bool {
    if unsafe { fence.GetCompletedValue() } >= value {
        return true;
    }
    let Ok(event) = (unsafe { CreateEventW(None, false, false, None) }) else { return false };
    let reached = unsafe { fence.SetEventOnCompletion(value, event) }.is_ok()
        && unsafe { WaitForSingleObject(event, ms) } == WAIT_OBJECT_0;
    let _ = unsafe { CloseHandle(event) };
    reached
}

/// Bounded: a queue blocked on work this faulting thread has yet to do must
/// not hang the title; the read then shows older contents.
fn wait_for(queues: &[(ID3D12Fence, u64)]) {
    for (fence, value) in queues {
        wait_on(fence, *value, wait_ms());
    }
}

#[allow(clippy::too_many_arguments)]
unsafe fn fill(
    device: &ID3D12Device, queue: &ID3D12CommandQueue, fence: &ID3D12Fence, value: u64,
    native: &ID3D12Resource, desc: &D3D12_RESOURCE_DESC, layout: D3D12_BARRIER_LAYOUT, base: usize, size: usize,
) -> Result<()> {
    if desc.SampleDesc.Count > 1 || super::format::is_depth(desc) {
        return Err(E_NOTIMPL.into());
    }
    let volume = desc.Dimension == D3D12_RESOURCE_DIMENSION_TEXTURE3D;
    let mips = u32::from(desc.MipLevels.max(1));
    let count = if volume { mips } else { mips * u32::from(desc.DepthOrArraySize.max(1)) };
    let n = count as usize;
    let mut footprints = vec![D3D12_PLACED_SUBRESOURCE_FOOTPRINT::default(); n];
    let mut rows = vec![0u32; n];
    let mut row_bytes = vec![0u64; n];
    let mut total = 0u64;
    unsafe { device.GetCopyableFootprints(desc, 0, count, 0, Some(footprints.as_mut_ptr()), Some(rows.as_mut_ptr()),
        Some(row_bytes.as_mut_ptr()), Some(&mut total)) };
    if total == u64::MAX || total == 0 {
        return Err(E_NOTIMPL.into());
    }
    let buffer_desc = D3D12_RESOURCE_DESC {
        Dimension: D3D12_RESOURCE_DIMENSION_BUFFER, Width: total, Height: 1, DepthOrArraySize: 1, MipLevels: 1,
        SampleDesc: DXGI_SAMPLE_DESC { Count: 1, Quality: 0 }, Layout: D3D12_TEXTURE_LAYOUT_ROW_MAJOR, ..Default::default()
    };
    let mut readback: Option<ID3D12Resource> = None;
    unsafe { device.CreateCommittedResource(&D3D12_HEAP_PROPERTIES { Type: D3D12_HEAP_TYPE_READBACK, ..Default::default() },
        D3D12_HEAP_FLAG_NONE, &buffer_desc, D3D12_RESOURCE_STATE_COPY_DEST, None, &mut readback) }?;
    let readback = readback.ok_or(E_FAIL)?;
    let allocator: ID3D12CommandAllocator = unsafe { device.CreateCommandAllocator(D3D12_COMMAND_LIST_TYPE_DIRECT) }?;
    let list: ID3D12GraphicsCommandList = unsafe { device.CreateCommandList(0, D3D12_COMMAND_LIST_TYPE_DIRECT, &allocator, None) }?;
    crate::dred::name(&list, "tuxbox committed writeback");
    let barriers: ID3D12GraphicsCommandList7 = list.cast()?;
    let from = if layout == D3D12_BARRIER_LAYOUT_UNDEFINED { D3D12_BARRIER_LAYOUT_COMMON } else { layout };
    let transition = |before: D3D12_BARRIER_LAYOUT, after: D3D12_BARRIER_LAYOUT, to_copy: bool| {
        let barrier = D3D12_TEXTURE_BARRIER {
            SyncBefore: if to_copy { D3D12_BARRIER_SYNC_NONE } else { D3D12_BARRIER_SYNC_COPY },
            SyncAfter: if to_copy { D3D12_BARRIER_SYNC_COPY } else { D3D12_BARRIER_SYNC_NONE },
            AccessBefore: if to_copy { D3D12_BARRIER_ACCESS_NO_ACCESS } else { D3D12_BARRIER_ACCESS_COPY_SOURCE },
            AccessAfter: if to_copy { D3D12_BARRIER_ACCESS_COPY_SOURCE } else { D3D12_BARRIER_ACCESS_NO_ACCESS },
            LayoutBefore: before,
            LayoutAfter: after,
            pResource: ManuallyDrop::new(Some(native.clone())),
            Subresources: D3D12_BARRIER_SUBRESOURCE_RANGE { IndexOrFirstMipLevel: 0xffff_ffff, ..Default::default() },
            Flags: D3D12_TEXTURE_BARRIER_FLAG_NONE,
        };
        let group = D3D12_BARRIER_GROUP {
            Type: D3D12_BARRIER_TYPE_TEXTURE, NumBarriers: 1,
            Anonymous: D3D12_BARRIER_GROUP_0 { pTextureBarriers: &barrier },
        };
        unsafe { barriers.Barrier(&[group]) };
        drop(ManuallyDrop::into_inner(barrier.pResource));
    };
    let moves = from != D3D12_BARRIER_LAYOUT_COPY_SOURCE;
    if moves {
        transition(from, D3D12_BARRIER_LAYOUT_COPY_SOURCE, true);
    }
    for (index, footprint) in footprints.iter().enumerate() {
        let src = D3D12_TEXTURE_COPY_LOCATION {
            pResource: ManuallyDrop::new(Some(native.clone())),
            Type: D3D12_TEXTURE_COPY_TYPE_SUBRESOURCE_INDEX,
            Anonymous: D3D12_TEXTURE_COPY_LOCATION_0 { SubresourceIndex: index as u32 },
        };
        let dst = D3D12_TEXTURE_COPY_LOCATION {
            pResource: ManuallyDrop::new(Some(readback.clone())),
            Type: D3D12_TEXTURE_COPY_TYPE_PLACED_FOOTPRINT,
            Anonymous: D3D12_TEXTURE_COPY_LOCATION_0 { PlacedFootprint: *footprint },
        };
        unsafe { list.CopyTextureRegion(&dst, 0, 0, 0, &src, None) };
        drop(ManuallyDrop::into_inner(src.pResource));
        drop(ManuallyDrop::into_inner(dst.pResource));
    }
    if moves {
        transition(D3D12_BARRIER_LAYOUT_COPY_SOURCE, from, false);
    }
    unsafe { list.Close() }?;
    unsafe { queue.ExecuteCommandLists(&[Some(list.cast()?)]) };
    unsafe { queue.Signal(fence, value) }?;
    if !wait_on(fence, value, 5000) {
        return Err(E_FAIL.into());
    }
    let mut mapped = core::ptr::null_mut();
    unsafe { readback.Map(0, Some(&D3D12_RANGE { Begin: 0, End: total as usize }), Some(&mut mapped)) }?;
    // SAFETY: a completed readback mapped for its whole width; the backing
    // pages are read/write and `size` bytes long.
    let data = unsafe { core::slice::from_raw_parts(mapped.cast::<u8>(), total as usize) };
    let pages = unsafe { core::slice::from_raw_parts_mut(base as *mut u8, size) };
    let plane = super::placement::color_plane(&super::placement::console_layout(desc)).ok();
    for (index, footprint) in footprints.iter().enumerate() {
        let (slice, level) = if volume { (0, index as u32) } else { (index as u32 / mips, index as u32 % mips) };
        let place = Subresource {
            offset: footprint.Offset as usize, pitch: footprint.Footprint.RowPitch as usize,
            rows: rows[index], row_bytes: row_bytes[index] as usize, depth: footprint.Footprint.Depth,
        };
        match &plane {
            Some(plane) => place.tile(data, pages, plane.element_bytes() as usize, |z, x, y| if volume {
                plane.element_offset_in_volume(z, level, x, y)
            } else {
                plane.element_offset_in_slice(slice, level, x, y)
            }),
            None => place.linear(data, pages, linear_start(&rows, &row_bytes, &footprints, index)),
        }
    }
    unsafe { readback.Unmap(0, Some(&D3D12_RANGE::default())) };
    Ok(())
}

/// Where subresource `index` starts when every earlier one is packed as tight rows.
fn linear_start(rows: &[u32], row_bytes: &[u64], footprints: &[D3D12_PLACED_SUBRESOURCE_FOOTPRINT], index: usize) -> usize {
    (0..index).map(|i| rows[i] as usize * row_bytes[i] as usize * footprints[i].Footprint.Depth as usize).sum()
}

struct Subresource {
    offset: usize,
    pitch: usize,
    rows: u32,
    row_bytes: usize,
    depth: u32,
}

impl Subresource {
    /// Each element to the console address `at(z, x, y)` gives; elements it
    /// cannot place, or that fall past the backing, are left as they are.
    fn tile(&self, data: &[u8], pages: &mut [u8], element: usize, at: impl Fn(u32, u64, u32) -> Option<u64>) {
        if element == 0 {
            return;
        }
        let elements = self.row_bytes / element;
        for z in 0..self.depth {
            for y in 0..self.rows {
                let row = self.offset + (z as usize * self.rows as usize + y as usize) * self.pitch;
                for x in 0..elements {
                    let Some(target) = at(z, x as u64, y).map(|t| t as usize) else { continue };
                    let source = row + x * element;
                    if let (Some(out), Some(src)) = (pages.get_mut(target..target + element), data.get(source..source + element)) {
                        out.copy_from_slice(src);
                    }
                }
            }
        }
    }

    /// Tight rows from `start` when no console plane fits the texture.
    fn linear(&self, data: &[u8], pages: &mut [u8], start: usize) {
        let mut at = start;
        for z in 0..self.depth {
            for y in 0..self.rows {
                let source = self.offset + (z as usize * self.rows as usize + y as usize) * self.pitch;
                if let (Some(out), Some(src)) = (pages.get_mut(at..at + self.row_bytes), data.get(source..source + self.row_bytes)) {
                    out.copy_from_slice(src);
                }
                at += self.row_bytes;
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn rows_land_at_the_console_address_and_out_of_range_elements_are_skipped() {
        // Two rows of two 4-byte elements, row pitch 16 in the readback.
        let data: Vec<u8> = (0..32).collect();
        let mut pages = vec![0xAA; 16];
        let sub = Subresource { offset: 0, pitch: 16, rows: 2, row_bytes: 8, depth: 1 };
        // Swap the two elements of each row; row 1 element 1 maps past the end.
        sub.tile(&data, &mut pages, 4, |_, x, y| Some(u64::from(y) * 8 + if x == 0 { 4 } else if y == 1 { 64 } else { 0 }));
        assert_eq!(&pages[0..4], &[4, 5, 6, 7]);
        assert_eq!(&pages[4..8], &[0, 1, 2, 3]);
        assert_eq!(&pages[12..16], &[16, 17, 18, 19]);
        assert_eq!(&pages[8..12], &[0xAA; 4]);
    }

    #[test]
    fn linear_fallback_packs_rows_tightly() {
        let data: Vec<u8> = (0..32).collect();
        let mut pages = vec![0; 8];
        Subresource { offset: 0, pitch: 16, rows: 2, row_bytes: 4, depth: 1 }.linear(&data, &mut pages, 0);
        assert_eq!(pages, [0, 1, 2, 3, 16, 17, 18, 19]);
    }
}
