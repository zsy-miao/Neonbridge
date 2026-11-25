//! TUXBOX_PLACED_DEVICE (default on, =0 imports as before), d3d12 half:
//! a buffer the title places in its registered GPU page pool gets a device
//! (DEFAULT heap) buffer instead of an import of the title pages, which
//! committed and pinned every page it covered (test155: the pool view at
//! 0xdac000000 grew 1792 -> 3795 MiB of commit). Model: LMIII / VItoriaBr /
//! leonida device shadows by title VA (victory-gpu resource.rs
//! create_at_address, shadow.rs; GPL-3.0, credit), with the copy-back moved
//! to the CPU's first touch so pages only the GPU uses are never committed.
//!
//! Each 64 KiB page of a placed buffer is GPU-owned (PAGE_NOACCESS, or
//! reserved and never touched) or CPU-owned (read-write). xmem's fault on a
//! GPU-owned page calls `touched`: if the GPU may have written the buffer
//! since (a UAV buffer, or a recorded copy into it) the page is read back
//! from the device buffer after the guest queues' work so far, then it is
//! CPU-owned. Before each guest submission the CPU-owned pages are made
//! read-only, copied up on a private queue the guest queue waits for, and
//! handed back to the GPU (no-access); a CPU write while read-only faults and
//! keeps the page CPU-owned. Kernel writers (DirectStorage) go through
//! TuxBoxCommitRange, which xmem routes through the same fault path.
//! Protection changes cover every title alias of a page (xmem).
use std::collections::BTreeMap;
use std::sync::{Mutex, MutexGuard, OnceLock};
use windows::Win32::Foundation::{CloseHandle, WAIT_OBJECT_0};
use windows::Win32::Graphics::Direct3D12::*;
use windows::Win32::Graphics::Dxgi::Common::{DXGI_FORMAT_UNKNOWN, DXGI_SAMPLE_DESC};
use windows::Win32::System::Memory::{PAGE_NOACCESS, PAGE_PROTECTION_FLAGS, PAGE_READONLY, PAGE_READWRITE};
use windows::Win32::System::Threading::{CreateEventW, WaitForSingleObject};
use windows::core::{Interface, Result};

const PAGE: usize = 0x1_0000;

#[derive(Clone, Copy, PartialEq, Eq)]
enum Owner { Gpu, Cpu, Uploading }

struct Entry {
    base: usize,
    bytes: usize,
    native: ID3D12Resource,
    device: ID3D12Device,
    uav: bool,
    /// A copy into it was recorded since the last readback.
    written: bool,
    /// By page index from `base & !(PAGE - 1)`.
    pages: Vec<Owner>,
}

#[derive(Default)]
struct Table {
    /// Page-aligned start -> entry.
    entries: BTreeMap<usize, Entry>,
    /// Native resource pointer -> page-aligned start.
    natives: BTreeMap<usize, usize>,
    /// A fence per guest queue and its last value.
    queues: BTreeMap<usize, (ID3D12Fence, u64)>,
    /// The private copy queue, its fence and last value.
    copier: Option<(ID3D12CommandQueue, ID3D12Fence, u64)>,
}

// SAFETY: D3D12 interfaces are free-threaded; the table is behind a mutex.
unsafe impl Send for Table {}

static TABLE: Mutex<Option<Table>> = Mutex::new(None);

fn table() -> MutexGuard<'static, Option<Table>> {
    TABLE.lock().unwrap_or_else(|e| e.into_inner())
}

pub(crate) fn enabled() -> bool {
    static ON: OnceLock<bool> = OnceLock::new();
    *ON.get_or_init(|| crate::env_toggle!("TUXBOX_PLACED_DEVICE") != Some("0") && xmem().is_some())
}

struct Xmem {
    range: unsafe extern "system" fn(usize, usize, u32) -> u32,
    protect: unsafe extern "system" fn(usize, usize) -> usize,
    callback: unsafe extern "system" fn(usize),
    aliases: unsafe extern "system" fn(usize, usize, u32) -> u32,
}

fn xmem() -> Option<&'static Xmem> {
    use windows::Win32::System::LibraryLoader::{GetModuleHandleW, GetProcAddress};
    static X: OnceLock<Option<Xmem>> = OnceLock::new();
    X.get_or_init(|| unsafe {
        let module = GetModuleHandleW(windows::core::w!("xmem.dll")).ok()?;
        let get = |name: windows::core::PCSTR| GetProcAddress(module, name);
        let x = Xmem {
            range: std::mem::transmute(get(windows::core::s!("TuxBoxDeviceRange"))?),
            protect: std::mem::transmute(get(windows::core::s!("TuxBoxProtectDevicePages"))?),
            callback: std::mem::transmute(get(windows::core::s!("TuxBoxDeviceFaultCallback"))?),
            aliases: std::mem::transmute(get(windows::core::s!("TuxBoxProtectDeviceAliases"))?),
        };
        (x.callback)(touched as unsafe extern "system" fn(usize, u32) -> u32 as usize);
        Some(x)
    }).as_ref()
}

/// Whether the page at `at` is committed (once per placement).
fn committed(at: usize) -> bool {
    use windows::Win32::System::Memory::{MEMORY_BASIC_INFORMATION, MEM_COMMIT, VirtualQuery};
    let mut m = MEMORY_BASIC_INFORMATION::default();
    (unsafe { VirtualQuery(Some(at as _), &mut m, size_of_val(&m)) }) != 0 && m.State == MEM_COMMIT
}

/// Sets `to` on the pages and every title alias of them (xmem).
fn protect(at: usize, bytes: usize, to: PAGE_PROTECTION_FLAGS) -> bool {
    xmem().is_some_and(|x| unsafe { (x.aliases)(at, bytes, to.0) } != 0)
}

/// Why a placed buffer was not given a device buffer (test162/164 made
/// none): the first 8 and every power of two.
fn declined(address: u64, desc: &D3D12_RESOURCE_DESC, why: &str) {
    static N: std::sync::atomic::AtomicU64 = std::sync::atomic::AtomicU64::new(0);
    let n = N.fetch_add(1, std::sync::atomic::Ordering::Relaxed) + 1;
    if n <= 8 || n.is_power_of_two() {
        crate::diagnostics::log_line(format_args!(
            "placed device buffer declined: {address:#x} ({} KiB): {why} ({n} declined so far)", desc.Width >> 10));
    }
}

/// TUXBOX_CARD_LIMITS (default on, =0 off): copied from VIctory-fxceless
/// victory-gpu shadow.rs card_limits / card_for (GPL-3.0, credit to its
/// authors). A placed buffer gets a device buffer only from
/// TUXBOX_CARD_BUFFERS_MIN_MB (0 here, fxceless 1: a buffer turned down
/// imports and pins title pages, the commit this is to cut) up, while the device buffers made hold at
/// most TUXBOX_CARD_BUFFERS_MB (1024) and, with this one, the card stays
/// under eight tenths of its budget (fxceless without its texture-queue
/// room switch, which we have no queue for); otherwise it imports as before.
/// Differences: fxceless subtracts released copies; ours are never released
/// here, so the count is of those made. test187 ran 582 MiB over the local
/// budget, which the driver moves to system memory (commit).
fn card_limits() -> (u64, u64) {
    static LIMITS: OnceLock<(u64, u64)> = OnceLock::new();
    *LIMITS.get_or_init(|| {
        let mb = |name: &str, default: u64| {
            std::env::var(name).ok().and_then(|v| v.trim().parse::<u64>().ok()).unwrap_or(default) << 20
        };
        (mb("TUXBOX_CARD_BUFFERS_MIN_MB", 0), mb("TUXBOX_CARD_BUFFERS_MB", 1024))
    })
}

/// Bytes of the device buffers made for placed buffers.
static CARD_BYTES: std::sync::atomic::AtomicU64 = std::sync::atomic::AtomicU64::new(0);

/// Whether a device buffer of `size` bytes may be made (card_limits).
fn card_room(device: &super::Device, address: u64, desc: &D3D12_RESOURCE_DESC, size: u64) -> bool {
    if crate::env_toggle!("TUXBOX_CARD_LIMITS") == Some("0") {
        return true;
    }
    let (least, most) = card_limits();
    if size < least {
        declined(address, desc, "under TUXBOX_CARD_BUFFERS_MIN_MB");
        return false;
    }
    let made = CARD_BYTES.load(std::sync::atomic::Ordering::Relaxed);
    if made + size > most {
        declined(address, desc, "device buffers at TUXBOX_CARD_BUFFERS_MB");
        return false;
    }
    let Ok(host) = device.native.cast::<ID3D12Device>() else { return true };
    if let Some((used, budget)) = super::placement::alias::segment(&host, true)
        && used + size > budget / 100 * 80
    {
        declined(address, desc, "the card would pass 80% of its budget");
        return false;
    }
    true
}

/// A device buffer for a buffer placed at `address` in a GPU page pool, or
/// None to import as before.
pub(super) fn place(device: &super::Device, address: u64, desc: &D3D12_RESOURCE_DESC) -> Option<Result<ID3D12Resource>> {
    if crate::env_toggle!("TUXBOX_PLACED_DEVICE") == Some("0") {
        return None;
    }
    if xmem().is_none() {
        declined(address, desc, "xmem exports missing");
        return None;
    }
    if !device.state.page_pools.contains(address) {
        declined(address, desc, "outside the GPU page pools");
        return None;
    }
    if desc.Width == 0 || desc.Width > 1 << 30 {
        declined(address, desc, "size 0 or over 1 GiB");
        return None;
    }
    let x = xmem()?;
    let base = address as usize & !(PAGE - 1);
    let end = (address as usize + desc.Width as usize).next_multiple_of(PAGE);
    {
        let t = table();
        if let Some(t) = t.as_ref()
            && t.entries.range(..end).next_back().is_some_and(|(_, e)| e.base + e.bytes > base)
        {
            // Overlaps a device-backed buffer: two device copies of one range
            // would disagree, so this one imports.
            declined(address, desc, "overlaps a device buffer");
            return None;
        }
    }
    // A sub-page start would need a view at an offset into the device
    // buffer: those import as before.
    if address as usize != base {
        declined(address, desc, "not page-aligned");
        return None;
    }
    if !card_room(device, address, desc, (end - base) as u64) {
        return None;
    }
    let uav = desc.Flags.0 & D3D12_RESOURCE_FLAG_ALLOW_UNORDERED_ACCESS.0 != 0;
    let heap = D3D12_HEAP_PROPERTIES { Type: D3D12_HEAP_TYPE_DEFAULT, CreationNodeMask: 1, VisibleNodeMask: 1, ..Default::default() };
    let host = D3D12_RESOURCE_DESC {
        Dimension: D3D12_RESOURCE_DIMENSION_BUFFER, Alignment: 0, Width: (end - base) as u64, Height: 1,
        DepthOrArraySize: 1, MipLevels: 1, Format: DXGI_FORMAT_UNKNOWN,
        SampleDesc: DXGI_SAMPLE_DESC { Count: 1, Quality: 0 }, Layout: D3D12_TEXTURE_LAYOUT_ROW_MAJOR,
        Flags: if uav { D3D12_RESOURCE_FLAG_ALLOW_UNORDERED_ACCESS } else { D3D12_RESOURCE_FLAG_NONE },
    };
    let mut whole: Option<ID3D12Resource> = None;
    if let Err(e) = unsafe { crate::diagnostics::wc_attrib::around(crate::diagnostics::wc_attrib::Kind::Committed, || device.native.CreateCommittedResource(&heap, D3D12_HEAP_FLAG_NONE, &host, D3D12_RESOURCE_STATE_COMMON, None, &mut whole)) } {
        return Some(Err(e));
    }
    let whole = whole?;
    CARD_BYTES.fetch_add(host.Width, std::sync::atomic::Ordering::Relaxed);
    crate::diagnostics::memory::live_on(&whole, crate::diagnostics::memory::Category::DeviceBuffer, host.Width);
    let _ = super::views::TextureAddress::at(device, &whole, address as usize, false, None);
    // Pages the CPU already wrote are CPU-owned and go up at the next submit.
    let pages: Vec<Owner> = (base..end).step_by(PAGE)
        .map(|p| if committed(p) { Owner::Cpu } else { Owner::Gpu })
        .collect();
    unsafe { (x.range)(base, end - base, 1) };
    for (i, owner) in pages.iter().enumerate() {
        if *owner == Owner::Gpu {
            unsafe { (x.protect)(base + i * PAGE, PAGE) };
        }
    }
    let mut t = table();
    let t = t.get_or_insert_with(Table::default);
    t.natives.insert(whole.as_raw() as usize, base);
    t.entries.insert(base, Entry {
        base, bytes: end - base, native: whole.clone(), device: device.native.clone(), uav, written: false, pages,
    });
    static N: std::sync::atomic::AtomicU64 = std::sync::atomic::AtomicU64::new(0);
    let n = N.fetch_add(1, std::sync::atomic::Ordering::Relaxed) + 1;
    if n <= 4 || n.is_power_of_two() {
        crate::diagnostics::log_line(format_args!(
            "placed buffer at {address:#x} ({} KiB{}) in the GPU page pool: a device buffer, title pages not imported ({n} so far; TUXBOX_PLACED_DEVICE=0 imports)",
            desc.Width >> 10, if uav { ", UAV" } else { "" }));
    }
    Some(Ok(whole))
}

/// A recorded copy writes into `native` (views note_shadow_write).
pub(crate) fn note_gpu_write(native: usize) {
    if !enabled() {
        return;
    }
    let mut t = table();
    let Some(t) = t.as_mut() else { return };
    if let Some(base) = t.natives.get(&native).copied()
        && let Some(entry) = t.entries.get_mut(&base)
    {
        entry.written = true;
    }
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

fn copier(t: &mut Table, device: &ID3D12Device) -> Result<(ID3D12CommandQueue, ID3D12Fence, u64)> {
    if t.copier.is_none() {
        let queue: ID3D12CommandQueue = unsafe { device.CreateCommandQueue(&D3D12_COMMAND_QUEUE_DESC {
            Type: D3D12_COMMAND_LIST_TYPE_DIRECT, ..Default::default()
        }) }?;
        let fence: ID3D12Fence = unsafe { device.CreateFence(0, D3D12_FENCE_FLAG_NONE) }?;
        t.copier = Some((queue, fence, 0));
    }
    let (queue, fence, value) = t.copier.as_mut().expect("set above");
    *value += 1;
    Ok((queue.clone(), fence.clone(), *value))
}

fn buffer(device: &ID3D12Device, kind: D3D12_HEAP_TYPE, bytes: u64) -> Result<ID3D12Resource> {
    let heap = D3D12_HEAP_PROPERTIES { Type: kind, ..Default::default() };
    let desc = D3D12_RESOURCE_DESC {
        Dimension: D3D12_RESOURCE_DIMENSION_BUFFER, Width: bytes, Height: 1, DepthOrArraySize: 1, MipLevels: 1,
        Layout: D3D12_TEXTURE_LAYOUT_ROW_MAJOR, SampleDesc: DXGI_SAMPLE_DESC { Count: 1, Quality: 0 }, ..Default::default()
    };
    let state = if kind == D3D12_HEAP_TYPE_READBACK { D3D12_RESOURCE_STATE_COPY_DEST } else { D3D12_RESOURCE_STATE_GENERIC_READ };
    let mut out = None;
    unsafe { crate::diagnostics::wc_attrib::around(crate::diagnostics::wc_attrib::Kind::Committed, || device.CreateCommittedResource(&heap, D3D12_HEAP_FLAG_NONE, &desc, state, None, &mut out)) }?;
    out.ok_or(windows::Win32::Foundation::E_POINTER.into())
}

/// Runs `record` on the private queue and waits for it (bounded).
fn run_private(t: &mut Table, device: &ID3D12Device, record: impl FnOnce(&ID3D12GraphicsCommandList)) -> Result<()> {
    let (queue, fence, value) = copier(t, device)?;
    let allocator: ID3D12CommandAllocator = unsafe { device.CreateCommandAllocator(D3D12_COMMAND_LIST_TYPE_DIRECT) }?;
    let list: ID3D12GraphicsCommandList = unsafe { device.CreateCommandList(0, D3D12_COMMAND_LIST_TYPE_DIRECT, &allocator, None) }?;
    record(&list);
    unsafe { list.Close() }?;
    unsafe { queue.ExecuteCommandLists(&[Some(list.cast()?)]) };
    unsafe { queue.Signal(&fence, value) }?;
    if !wait_on(&fence, value, 5000) {
        // Leaked rather than freed under a running copy (xcl 52272a0).
        std::mem::forget((allocator, list));
        return Err(windows::Win32::Foundation::E_FAIL.into());
    }
    Ok(())
}

/// xmem's fault on a device-backed page (TuxBoxDeviceFaultCallback): the
/// page at `page` is committed; returns 1 when the access may go on.
unsafe extern "system" fn touched(page: usize, _write: u32) -> u32 {
    let mut guard = table();
    let Some(t) = guard.as_mut() else { return 1 };
    let Some((&base, entry)) = t.entries.range(..=page).next_back() else { return 1 };
    if page >= base + entry.bytes {
        return 1;
    }
    let index = (page - base) / PAGE;
    let owner = entry.pages[index];
    if owner == Owner::Gpu && (entry.uav || entry.written) {
        // The guest queues' work so far first (bounded: a queue blocked on
        // this thread must not hang the title; the read is older then).
        for (fence, value) in t.queues.values() {
            wait_on(fence, *value, 500);
        }
        let entry = t.entries.get(&base).expect("found above");
        let (device, native) = (entry.device.clone(), entry.native.clone());
        let Ok(readback) = buffer(&device, D3D12_HEAP_TYPE_READBACK, PAGE as u64) else { return 0 };
        let at = (page - base) as u64;
        let copied = run_private(t, &device, |list| unsafe {
            list.CopyBufferRegion(&readback, 0, &native, at, PAGE as u64);
        });
        if copied.is_ok() {
            let mut mapped = std::ptr::null_mut();
            if unsafe { readback.Map(0, Some(&D3D12_RANGE { Begin: 0, End: PAGE }), Some(&mut mapped)) }.is_ok()
                && protect(page, PAGE, PAGE_READWRITE)
            {
                unsafe { std::ptr::copy_nonoverlapping(mapped.cast::<u8>(), page as *mut u8, PAGE) };
                unsafe { readback.Unmap(0, None) };
            }
        }
        static N: std::sync::atomic::AtomicU64 = std::sync::atomic::AtomicU64::new(0);
        let n = N.fetch_add(1, std::sync::atomic::Ordering::Relaxed) + 1;
        if n <= 4 || n.is_power_of_two() {
            crate::diagnostics::log_line(format_args!(
                "placed device buffer: page {page:#x} read back at its first CPU touch since the GPU wrote it ({n} so far; TUXBOX_PLACED_DEVICE=0 imports)"));
        }
    }
    if let Some(entry) = t.entries.get_mut(&base) {
        entry.pages[index] = Owner::Cpu;
    }
    1
}

/// Before guest work goes to `queue`: the CPU-owned pages go up and back to
/// the GPU; `queue` waits for the copy.
pub(crate) fn before_submit(queue: &ID3D12CommandQueue) {
    if !enabled() {
        return;
    }
    let mut guard = table();
    let Some(t) = guard.as_mut() else { return };
    let mut work: Vec<(usize, usize, ID3D12Resource, ID3D12Device)> = Vec::new();
    for entry in t.entries.values_mut() {
        for (i, owner) in entry.pages.iter_mut().enumerate() {
            if *owner == Owner::Cpu {
                let page = entry.base + i * PAGE;
                // Read-only first: a write from here on faults and keeps it CPU-owned.
                if protect(page, PAGE, PAGE_READONLY) {
                    *owner = Owner::Uploading;
                    work.push((entry.base, page, entry.native.clone(), entry.device.clone()));
                }
            }
        }
    }
    if work.is_empty() {
        return;
    }
    let device = work[0].3.clone();
    let Ok(upload) = buffer(&device, D3D12_HEAP_TYPE_UPLOAD, (work.len() * PAGE) as u64) else { return };
    let mut mapped = std::ptr::null_mut();
    if unsafe { upload.Map(0, Some(&D3D12_RANGE::default()), Some(&mut mapped)) }.is_err() {
        return;
    }
    for (k, &(_, page, ..)) in work.iter().enumerate() {
        unsafe { std::ptr::copy_nonoverlapping(page as *const u8, mapped.cast::<u8>().add(k * PAGE), PAGE) };
    }
    let Ok((copy_queue, fence, value)) = copier(t, &device) else { return };
    let Ok(allocator) = (unsafe { device.CreateCommandAllocator::<ID3D12CommandAllocator>(D3D12_COMMAND_LIST_TYPE_DIRECT) }) else { return };
    let Ok(list) = (unsafe { device.CreateCommandList::<_, _, ID3D12GraphicsCommandList>(0, D3D12_COMMAND_LIST_TYPE_DIRECT, &allocator, None) }) else { return };
    for (k, (base, page, native, _)) in work.iter().enumerate() {
        unsafe { list.CopyBufferRegion(native, (page - base) as u64, &upload, (k * PAGE) as u64, PAGE as u64) };
    }
    if unsafe { list.Close() }.is_err() {
        return;
    }
    let Ok(executable) = list.cast::<ID3D12CommandList>() else { return };
    unsafe { copy_queue.ExecuteCommandLists(&[Some(executable)]) };
    let _ = unsafe { copy_queue.Signal(&fence, value) };
    let _ = unsafe { queue.Wait(&fence, value) };
    // Kept until the copy is done: released at a later submit.
    PENDING.lock().unwrap_or_else(|e| e.into_inner()).push((fence.clone(), value, (allocator, list, upload)));
    PENDING.lock().unwrap_or_else(|e| e.into_inner()).retain(|(f, v, _)| unsafe { f.GetCompletedValue() } < *v);
    for (base, page, ..) in work {
        if let Some(entry) = t.entries.get_mut(&base) {
            let index = (page - base) / PAGE;
            // A CPU write since the snapshot made it CPU-owned again (fault).
            if entry.pages[index] == Owner::Uploading && protect(page, PAGE, PAGE_NOACCESS) {
                entry.pages[index] = Owner::Gpu;
            }
        }
    }
}

type Held = (ID3D12Fence, u64, (ID3D12CommandAllocator, ID3D12GraphicsCommandList, ID3D12Resource));
static PENDING: Mutex<PendingVec> = Mutex::new(PendingVec(Vec::new()));
struct PendingVec(Vec<Held>);
// SAFETY: D3D12 objects are free-threaded; these are only kept and released.
unsafe impl Send for PendingVec {}
impl std::ops::Deref for PendingVec { type Target = Vec<Held>; fn deref(&self) -> &Vec<Held> { &self.0 } }
impl std::ops::DerefMut for PendingVec { fn deref_mut(&mut self) -> &mut Vec<Held> { &mut self.0 } }

/// After guest work went to `queue`: how far it got, for readback waits.
pub(crate) fn after_submit(queue: &ID3D12CommandQueue) {
    if !enabled() {
        return;
    }
    let mut guard = table();
    let Some(t) = guard.as_mut() else { return };
    if t.entries.is_empty() {
        return;
    }
    let key = queue.as_raw() as usize;
    if !t.queues.contains_key(&key) {
        let mut device: Option<ID3D12Device> = None;
        if unsafe { queue.GetDevice(&mut device) }.is_err() { return }
        let Some(Ok(fence)) = device.map(|d| unsafe { d.CreateFence::<ID3D12Fence>(0, D3D12_FENCE_FLAG_NONE) }) else { return };
        t.queues.insert(key, (fence, 0));
    }
    let (fence, value) = t.queues.get_mut(&key).expect("inserted above");
    *value += 1;
    let _ = unsafe { queue.Signal(&*fence, *value) };
    // A UAV buffer or copy target may be written by this work: its next CPU
    // touch reads back. `written` stays set until the readback (touched).
}
