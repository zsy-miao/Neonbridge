use super::*;
use std::collections::VecDeque;
use std::ffi::{CStr, CString};
use std::os::windows::io::{FromRawHandle, IntoRawHandle, OwnedHandle};
use std::sync::Condvar;
use std::thread::{self, JoinHandle};
use windows::Win32::Graphics::Direct3D12::ID3D12Fence;
use windows::Win32::System::Threading::{CreateEventW, ResetEvent, SetEvent};

#[implement(IQueue2)]
struct Queue {
    shared: Arc<Shared>,
    worker: Mutex<Option<JoinHandle<()>>>,
    name: Option<CString>,
    source_type: u64,
    priority: i8,
    manual: bool,
}

struct Shared {
    state: Mutex<State>,
    changed: Condvar,
    capacity: usize,
    priority: usize,
    error_event: OwnedHandle,
}

struct State {
    entries: VecDeque<Entry>,
    ready: usize,
    processing: bool,
    closed: bool,
    error_count: u32,
    first_error: Option<Box<ErrorRecord>>,
}

enum Entry {
    Read(Read),
    Status(Arc<[AtomicI32]>, usize),
    Signal(ID3D12Fence, u64),
    Event(OwnedHandle),
    Invalid(HRESULT, u32),
}

enum Source {
    File(FileRead, u64),
    Memory(usize),
    /// A physical page array (bit 23).
    Pages(Pages),
}

/// A `PULONG_PTR` array of xmem physical-page tokens, snapshotted at enqueue
/// (titles pass arrays on the stack). An entry still 0 is read again when the
/// request runs: titles fill their own page tables after EnqueueRequest.
struct Pages {
    array: usize,
    tokens: Vec<u64>,
    offset: usize,
}

impl Pages {
    unsafe fn capture(array: *const c_void, offset: u16, bytes: usize) -> Result<Self, HRESULT> {
        if array.is_null() {
            return Err(E_INVALIDARG);
        }
        let count = (offset as usize + bytes).div_ceil(PHYSICAL_PAGE);
        let mut tokens = Vec::new();
        tokens.try_reserve_exact(count).map_err(|_| E_OUTOFMEMORY)?;
        // SAFETY: the DirectStorage contract keeps the array readable at enqueue.
        tokens.extend((0..count).map(|i| unsafe { array.cast::<u64>().add(i).read_unaligned() }));
        Ok(Self { array: array as usize, tokens, offset: offset as usize })
    }

    fn resolved(&self) -> Vec<u64> {
        let mut tokens = self.tokens.clone();
        for (i, token) in tokens.iter_mut().enumerate() {
            if *token == 0 {
                // SAFETY: as at enqueue; the title owns its page table.
                *token = unsafe { (self.array as *const u64).add(i).read_unaligned() };
            }
        }
        tokens
    }
}

const PHYSICAL_PAGE: usize = 0x10000;
const ZLIB: u64 = 1;
const BCPACK_SHIFT: u64 = 1;
const SWIZZLE: u64 = 0xFFFF << 5;
const DEST_PAGES: u64 = 1 << 21;
const FROM_MEMORY: u64 = 1 << 22;
const SOURCE_PAGES: u64 = 1 << 23;
const KNOWN: u64 = (1 << 24) - 1;

struct Read {
    source: Source,
    dest: usize,
    /// Destination physical pages (bit 21); `dest` is then the array.
    dest_pages: Option<Pages>,
    capacity: usize,
    size: usize,
    options: u64,
    tag: u64,
    cancelled: bool,
}

fn log(message: std::fmt::Arguments) {
    if std::env::var_os("TUXBOX_LOG").is_some() {
        eprintln!("TuxBox xcrdapi: {message}");
    }
}

fn once(what: &'static str, message: std::fmt::Arguments) {
    static SEEN: Mutex<Vec<&'static str>> = Mutex::new(Vec::new());
    let mut seen = lock(&SEEN);
    if !seen.contains(&what) {
        seen.push(what);
        log(message);
    }
}

fn bcpack_mode(options: u64) -> u8 {
    ((options >> BCPACK_SHIFT) & 0xF) as u8
}

// Borrowed process buffer addresses are valid until queue completion by the
// DirectStorage contract. The file is retained independently of its COM object.
unsafe fn capture(queue_type: u64, req: *const Request) -> Result<Read, HRESULT> {
    let req = unsafe { req.as_ref() }.ok_or(E_POINTER)?;
    if req.options & !KNOWN != 0 {
        once("unknown", format_args!("unknown request option bits {:#x} ignored", req.options & !KNOWN));
    }
    if req.options & SWIZZLE != 0 {
        // The console's hardware swizzle; the bytes are stored as read.
        once("swizzle", format_args!("swizzle mode {:#x} not applied; data stored linear", (req.options & SWIZZLE) >> 5));
    }
    let bcpack = bcpack_mode(req.options);
    if bcpack != 0 && std::env::var("TUXBOX_BCPACK").is_ok_and(|v| v.trim() == "refuse") {
        return Err(E_NOTIMPL);
    }
    let memory = req.options & FROM_MEMORY != 0;
    if memory != (queue_type == 1)
        || (req.options & SOURCE_PAGES != 0 && !memory)
        || req.destination.is_null()
        || req.source.is_null()
        || req.size == 0
        || req.destination_size == 0
        || (req.options & ZLIB == 0 && bcpack == 0 && req.size > req.destination_size)
    {
        return Err(E_INVALIDARG);
    }
    let source = if memory && req.options & SOURCE_PAGES != 0 {
        Source::Pages(unsafe { Pages::capture(req.source, req.offset as u16, req.size as usize)? })
    } else if memory {
        Source::Memory(req.source as usize)
    } else {
        let file = unsafe { IUnknown::from_raw_borrowed(&req.source) }.ok_or(E_POINTER)?;
        let object = file
            .cast_object_ref::<StorageFile>()
            .map_err(|e| e.code())?;
        Source::File(object.backing.capture()?, req.offset)
    };
    let dest_pages = if req.options & DEST_PAGES != 0 {
        Some(unsafe { Pages::capture(req.destination.cast(), req.page_offset, req.destination_size as usize)? })
    } else {
        None
    };
    Ok(Read {
        source,
        dest: req.destination as usize,
        dest_pages,
        capacity: req.destination_size as usize,
        size: req.size as usize,
        options: req.options,
        tag: req.tag,
        cancelled: false,
    })
}

unsafe extern "system" {
    fn ReadFile(
        file: *mut c_void,
        output: *mut u8,
        size: u32,
        got: *mut u32,
        overlapped: *mut Overlapped,
    ) -> i32;
    fn GetModuleHandleW(name: *const u16) -> *mut c_void;
    fn GetProcAddress(module: *mut c_void, name: *const u8) -> *const c_void;
}

#[repr(C)]
#[derive(Default)]
struct Overlapped {
    internal: usize,
    bytes: usize,
    offset: u64,
    event: usize,
}

type PageCopy = unsafe extern "system" fn(*const u64, usize, usize, *mut u8, usize) -> i32;

/// xmem's TuxBoxReadPhysicalPages / TuxBoxWritePhysicalPages.
fn xmem_pages(write: bool) -> Option<PageCopy> {
    static FUNCTIONS: OnceLock<[Option<PageCopy>; 2]> = OnceLock::new();
    FUNCTIONS.get_or_init(|| unsafe {
        let name: Vec<u16> = "xmem.dll".encode_utf16().chain([0]).collect();
        let module = GetModuleHandleW(name.as_ptr());
        let find = |n: &[u8]| -> Option<PageCopy> {
            let f = if module.is_null() { core::ptr::null() } else { GetProcAddress(module, n.as_ptr()) };
            (!f.is_null()).then(|| core::mem::transmute::<*const c_void, PageCopy>(f))
        };
        [find(b"TuxBoxReadPhysicalPages\0"), find(b"TuxBoxWritePhysicalPages\0")]
    })[usize::from(write)]
}

fn page_copy(write: bool, pages: &Pages, buffer: *mut u8, bytes: usize) -> Result<(), HRESULT> {
    let copy = xmem_pages(write).ok_or_else(|| {
        once("xmem pages", format_args!("xmem has no physical-page copy exports; page-array requests fail"));
        E_NOTIMPL
    })?;
    let tokens = pages.resolved();
    let result = unsafe { copy(tokens.as_ptr(), tokens.len(), pages.offset, buffer, bytes) };
    if result == 1 { Ok(()) } else { Err(HRESULT::from_win32(ERROR_INVALID_ADDRESS.0)) }
}

/// One block of each BCPack mode's BCn format in a neutral grey (normal maps
/// flat): BC1, BC2, BC3, BC4, BC5, BC6H, BC7. Bit layouts from the BCn specs.
fn bcpack_block(mode: u8) -> Option<Vec<u8>> {
    // RGB565 grey 0x8410 in both endpoints, all indices 0.
    const BC1: [u8; 8] = [0x10, 0x84, 0x10, 0x84, 0, 0, 0, 0];
    const BC4_GREY: [u8; 8] = [0x80, 0x80, 0, 0, 0, 0, 0, 0];
    const BC4_OPAQUE: [u8; 8] = [0xFF, 0xFF, 0, 0, 0, 0, 0, 0];
    let join = |a: &[u8], b: &[u8]| [a, b].concat();
    Some(match mode {
        1 => BC1.to_vec(),
        2 => join(&[0xFF; 8], &BC1),
        3 => join(&BC4_OPAQUE, &BC1),
        4 => BC4_GREY.to_vec(),
        5 => join(&BC4_GREY, &BC4_GREY),
        // BC6H mode 11 (00011): one region, 10-bit endpoints; 462 is 0.5 as
        // unsigned half after unquantization. All indices 0.
        6 => {
            let v: u128 = 462;
            (3 | v << 5 | v << 15 | v << 25 | v << 35 | v << 45 | v << 55).to_le_bytes().to_vec()
        }
        // BC7 mode 6 (bit 6): 7-bit RGBA endpoints plus p-bits; rgb 64 and
        // a 127 with p = 1 give (129, 129, 129, 255). All indices 0.
        7 => {
            let (c, a): (u128, u128) = (64, 127);
            (1 << 6 | c << 7 | c << 14 | c << 21 | c << 28 | c << 35 | c << 42 | a << 49 | a << 56 | 1 << 63 | 1 << 64)
                .to_le_bytes()
                .to_vec()
        }
        _ => return None,
    })
}

impl Read {
    fn read_source(&self, target: *mut u8) -> Result<(), HRESULT> {
        match &self.source {
            Source::File(file, offset) => {
                // A synchronous Win32 handle with an explicit 64-bit OVERLAPPED
                // offset makes each read atomic w.r.t. concurrent offset reads.
                let mut ov = Overlapped {
                    offset: *offset,
                    ..Default::default()
                };
                let mut got = 0;
                {
                    let mut stats = storage_stats();
                    stats.pending_reads += 1;
                    stats.total_reads = stats.total_reads.saturating_add(1);
                }
                let success = unsafe {
                    ReadFile(
                        file.file.as_raw_handle(),
                        target,
                        self.size as u32,
                        &mut got,
                        &mut ov,
                    )
                };
                let result = if success == 0 {
                    Err(HRESULT::from_win32(unsafe { GetLastError() }.0))
                } else if got as usize != self.size {
                    Err(HRESULT::from_win32(ERROR_HANDLE_EOF.0))
                } else {
                    Ok(())
                };
                let mut stats = storage_stats();
                stats.pending_reads -= 1;
                stats.read_errors = stats.read_errors.saturating_add(u32::from(result.is_err()));
                result
            }
            Source::Memory(p) => {
                unsafe { core::ptr::copy(*p as *const u8, target, self.size) };
                Ok(())
            }
            Source::Pages(pages) => page_copy(false, pages, target, self.size),
        }
    }

    /// Write the request's output to its destination.
    fn store(&self, bytes: &[u8]) -> Result<(), HRESULT> {
        let bytes = &bytes[..bytes.len().min(self.capacity)];
        match &self.dest_pages {
            Some(pages) => page_copy(true, pages, bytes.as_ptr().cast_mut(), bytes.len()),
            None => {
                unsafe { core::ptr::copy_nonoverlapping(bytes.as_ptr(), self.dest as *mut u8, bytes.len()) };
                Ok(())
            }
        }
    }

    fn execute(&self) -> Result<(), HRESULT> {
        if self.cancelled {
            return Ok(());
        }
        let bcpack = bcpack_mode(self.options);
        if bcpack != 0 {
            // No BCPack decoder yet: the destination gets solid blocks of the
            // mode's format, so the texture shows flat grey instead of stale
            // memory and the title's request succeeds (TUXBOX_BCPACK=refuse
            // fails it instead).
            let block = bcpack_block(bcpack).ok_or(HRESULT::from_win32(ERROR_INVALID_DATA.0))?;
            let mut fill = Vec::new();
            fill.try_reserve_exact(self.capacity).map_err(|_| E_OUTOFMEMORY)?;
            fill.extend(block.iter().copied().cycle().take(self.capacity));
            static FILLED: std::sync::atomic::AtomicU64 = std::sync::atomic::AtomicU64::new(0);
            let n = FILLED.fetch_add(1, Ordering::Relaxed) + 1;
            if n.is_power_of_two() {
                log(format_args!("BCPack mode {bcpack}{} request #{n}: {} bytes filled with solid blocks (no decoder)",
                    if self.options & ZLIB != 0 { " after zlib" } else { "" }, self.capacity));
            }
            return self.store(&fill);
        }
        if self.options & ZLIB == 0 && self.dest_pages.is_none() {
            return self.read_source(self.dest as *mut u8);
        }
        let mut local = Vec::new();
        let mut staging = (self.options & ZLIB != 0 && matches!(self.source, Source::File(..)))
            .then(|| lock(&STAGING));
        let input = match staging.as_deref_mut().and_then(Option::as_mut) {
            Some((limit, _)) if self.size > *limit => return Err(E_OUTOFMEMORY),
            Some((_, bytes)) => {
                bytes.clear();
                bytes
            }
            None => &mut local,
        };
        input.try_reserve_exact(self.size).map_err(|_| E_OUTOFMEMORY)?;
        self.read_source(input.spare_capacity_mut().as_mut_ptr().cast::<u8>())?;
        // The read or copy above initialized every reported input byte.
        unsafe { input.set_len(self.size) };
        if self.options & ZLIB == 0 {
            return self.store(input);
        }
        {
            let mut stats = storage_stats();
            stats.pending_decodes += 1;
            stats.total_decodes = stats.total_decodes.saturating_add(1);
        }
        let output = miniz_oxide::inflate::decompress_to_vec_zlib_with_limit(input, self.capacity);
        {
            let mut stats = storage_stats();
            stats.pending_decodes -= 1;
            stats.decode_errors = stats
                .decode_errors
                .saturating_add(u32::from(output.is_err()));
        }
        let output = output.map_err(|_| HRESULT::from_win32(ERROR_INVALID_DATA.0))?;
        self.store(&output)
    }
}

fn run_read(shared: &Shared, entry: &Entry) -> Result<(), HRESULT> {
    let Entry::Read(req) = entry else { return Ok(()) };
    {
        let mut stats = storage_stats();
        let p = &mut stats.priorities[shared.priority];
        p.pending += 1;
        p.requests = p.requests.saturating_add(1);
        p.source_bytes = p.source_bytes.saturating_add(req.size as u64);
        p.destination_bytes = p.destination_bytes.saturating_add(req.capacity as u64);
    }
    let result = req.execute();
    storage_stats().priorities[shared.priority].pending -= 1;
    result
}

fn signal_error(shared: &Shared, entry: &Entry, hr: HRESULT) {
    let mut state = lock(&shared.state);
    state.error_count = state.error_count.saturating_add(1);
    if state.first_error.is_none() {
        // The documented record consists exclusively of pointers, integers
        // and fixed-size strings, for which all-zero is an empty record.
        let mut r: Box<ErrorRecord> = Box::new(unsafe { core::mem::zeroed() });
        r.hr = hr.0;
        match entry {
            Entry::Read(read) => {
                r.options = read.options;
                r.destination = read.dest;
                r.destination_size = read.capacity as u32;
                r.size = read.size as u32;
                r.tag = read.tag;
                match read.source {
                    Source::File(_, offset) => r.offset = offset,
                    Source::Memory(p) => r.source = p,
                    Source::Pages(ref pages) => r.source = pages.array,
                }
            }
            Entry::Invalid(_, kind) => r.kind = *kind,
            Entry::Status(..) => r.kind = 1,
            Entry::Event(_) => r.kind = 3,
            Entry::Signal(fence, value) => {
                r.kind = 2;
                r.options = fence.as_raw() as u64;
                r.destination = *value as usize;
            }
        }
        state.first_error = Some(r);
    }
    let _ = unsafe { SetEvent(HANDLE(shared.error_event.as_raw_handle())) };
}

/// Reads run this many at a time per queue (TUXBOX_STORAGE_PARALLEL, default
/// 4; 1 = one by one). Status, signal and event entries still wait for every
/// read before them, which is all DirectStorage orders.
fn parallel_reads() -> usize {
    static N: OnceLock<usize> = OnceLock::new();
    *N.get_or_init(|| std::env::var("TUXBOX_STORAGE_PARALLEL").ok().and_then(|v| v.trim().parse().ok()).unwrap_or(4).clamp(1, 16))
}

/// The queue's priority (-1 low .. 2 real-time) as its worker's thread
/// priority, so the title's urgent loads win the CPU (TUXBOX_STORAGE_PRIORITY=0
/// leaves every worker at normal).
fn set_worker_priority(priority: usize) {
    use windows::Win32::System::Threading::*;
    if std::env::var("TUXBOX_STORAGE_PRIORITY").is_ok_and(|v| v.trim() == "0") {
        return;
    }
    let level = match priority {
        0 => THREAD_PRIORITY_BELOW_NORMAL,
        1 => THREAD_PRIORITY_NORMAL,
        2 => THREAD_PRIORITY_ABOVE_NORMAL,
        _ => THREAD_PRIORITY_HIGHEST,
    };
    let _ = unsafe { SetThreadPriority(GetCurrentThread(), level) };
}

fn consume(shared: &Shared) {
    set_worker_priority(shared.priority);
    let mut last_error = S_OK;
    loop {
        let mut state = lock(&shared.state);
        while state.ready == 0 && !state.closed {
            state = shared
                .changed
                .wait(state)
                .unwrap_or_else(|e| e.into_inner());
        }
        let Some(entry) = state.entries.pop_front() else {
            return;
        };
        state.ready -= 1;
        state.processing = true;
        // Consecutive ready reads after this one join it.
        let mut batch = Vec::new();
        if matches!(entry, Entry::Read(_)) {
            while batch.len() + 1 < parallel_reads() && state.ready > 0 && matches!(state.entries.front(), Some(Entry::Read(_))) {
                batch.push(state.entries.pop_front().unwrap());
                state.ready -= 1;
            }
        }
        drop(state);
        if !batch.is_empty() {
            batch.insert(0, entry);
            let results: Vec<Result<(), HRESULT>> = std::thread::scope(|scope| {
                let handles: Vec<_> = batch[1..].iter().map(|e| scope.spawn(move || run_read(shared, e))).collect();
                let mut results = vec![run_read(shared, &batch[0])];
                results.extend(handles.into_iter().map(|h| h.join().unwrap_or(Err(E_FAIL))));
                results
            });
            for (entry, result) in batch.iter().zip(results) {
                if let Err(hr) = result {
                    if last_error.is_ok() {
                        last_error = hr;
                    }
                    {
                        let mut stats = storage_stats();
                        stats.errors = stats.errors.saturating_add(1);
                    }
                    signal_error(shared, entry, hr);
                }
            }
            lock(&shared.state).processing = false;
            shared.changed.notify_all();
            continue;
        }
        let result = match &entry {
            Entry::Read(req) => {
                {
                    let mut stats = storage_stats();
                    let p = &mut stats.priorities[shared.priority];
                    p.pending += 1;
                    p.requests = p.requests.saturating_add(1);
                    p.source_bytes = p.source_bytes.saturating_add(req.size as u64);
                    p.destination_bytes = p.destination_bytes.saturating_add(req.capacity as u64);
                }
                let result = req.execute();
                storage_stats().priorities[shared.priority].pending -= 1;
                result
            }
            Entry::Status(values, i) => {
                let mut stats = storage_stats();
                let p = &mut stats.priorities[shared.priority];
                p.statuses = p.statuses.saturating_add(1);
                values[*i].store(last_error.0, Ordering::Release);
                last_error = S_OK;
                Ok(())
            }
            Entry::Signal(fence, value) => {
                {
                    let mut stats = storage_stats();
                    let p = &mut stats.priorities[shared.priority];
                    p.fences = p.fences.saturating_add(1);
                }
                unsafe { fence.Signal(*value) }.map_err(|e| e.code())
            }
            Entry::Invalid(hr, _) => Err(*hr),
            Entry::Event(event) => {
                unsafe { SetEvent(HANDLE(event.as_raw_handle())) }.map_err(|e| e.code())
            }
        };
        if let Err(hr) = result {
            if last_error.is_ok() {
                last_error = hr;
            }
            {
                let mut stats = storage_stats();
                stats.errors = stats.errors.saturating_add(1);
            }
            signal_error(shared, &entry, hr);
        }
        lock(&shared.state).processing = false;
        shared.changed.notify_all();
    }
}

pub(super) unsafe fn create(desc: &QueueDesc, manual: bool) -> Result<IQueue2, HRESULT> {
    if desc.source_type > 1
        || (desc.source_type == 1 && desc.priority != 2)
        || !(-1..=2).contains(&desc.priority)
        || !(0x80..=0x2000).contains(&desc.capacity)
    {
        return Err(E_INVALIDARG);
    }
    let handle = unsafe { CreateEventW(None, true, false, None) }.map_err(|e| e.code())?;
    let shared = Arc::new(Shared {
        state: Mutex::new(State {
            entries: VecDeque::new(),
            ready: 0,
            processing: false,
            closed: false,
            error_count: 0,
            first_error: None,
        }),
        changed: Condvar::new(),
        capacity: desc.capacity as usize,
        priority: (desc.priority + 1) as usize,
        error_event: unsafe { OwnedHandle::from_raw_handle(handle.0) },
    });
    let run = Arc::clone(&shared);
    let worker = thread::Builder::new()
        .name("TuxBox storage".into())
        .spawn(move || consume(&run))
        .map_err(|_| E_OUTOFMEMORY)?;
    let name = if desc.name.is_null() {
        None
    } else {
        Some(unsafe { CStr::from_ptr(desc.name) }.to_owned())
    };
    {
        let mut stats = storage_stats();
        let p = &mut stats.priorities[(desc.priority + 1) as usize];
        p.queues += 1;
        p.peak_queues = p.peak_queues.max(p.queues);
        p.total_queues = p.total_queues.saturating_add(1);
    }
    Ok(Queue {
        manual,
        shared,
        worker: Mutex::new(Some(worker)),
        name,
        source_type: desc.source_type,
        priority: desc.priority,
    }
    .into())
}

impl Queue {
    fn enqueue(&self, entry: Entry) {
        let mut state = lock(&self.shared.state);
        while !state.closed
            && state.entries.len() + usize::from(state.processing) >= self.shared.capacity - 1
        {
            if !self.manual {
                state.ready = state.entries.len();
                self.shared.changed.notify_all();
            }
            state = self
                .shared
                .changed
                .wait(state)
                .unwrap_or_else(|e| e.into_inner());
        }
        if state.closed {
            return;
        }
        if let Entry::Status(v, i) = &entry {
            v[*i].store(E_PENDING.0, Ordering::Release);
        }
        state.entries.push_back(entry);
        if !self.manual && state.entries.len() - state.ready >= self.shared.capacity / 2 {
            state.ready = state.entries.len();
            self.shared.changed.notify_all();
        }
    }
    fn close(&self) {
        {
            let mut state = lock(&self.shared.state);
            state.closed = true;
            state.ready = state.entries.len();
            self.shared.changed.notify_all();
        }
        if let Some(worker) = lock(&self.worker).take() {
            let _ = worker.join();
        }
    }
}

impl Drop for Queue {
    fn drop(&mut self) {
        self.close();
        storage_stats().priorities[(self.priority + 1) as usize].queues -= 1;
    }
}

impl IQueue_Impl for Queue_Impl {
    unsafe fn EnqueueRequest(&self, req: *const Request) {
        self.enqueue(
            unsafe { capture(self.source_type, req) }
                .map_or_else(|e| Entry::Invalid(e, 0), Entry::Read),
        );
    }
    unsafe fn EnqueueStatus(&self, array: *mut c_void, index: u32) {
        let entry = unsafe { IUnknown::from_raw_borrowed(&array) }
            .and_then(|u| u.cast_object_ref::<Status>().ok())
            .filter(|obj| (index as usize) < obj.values.len())
            .map(|obj| Entry::Status(Arc::clone(&obj.values), index as usize))
            .unwrap_or(Entry::Invalid(E_INVALIDARG, 1));
        self.enqueue(entry);
    }
    unsafe fn EnqueueSignal(&self, fence: *mut c_void, value: u64) {
        let entry = unsafe { IUnknown::from_raw_borrowed(&fence) }
            .and_then(|u| u.cast::<ID3D12Fence>().ok())
            .map(|f| Entry::Signal(f, value))
            .unwrap_or(Entry::Invalid(E_INVALIDARG, 2));
        self.enqueue(entry);
    }
    unsafe fn Submit(&self) {
        let mut s = lock(&self.shared.state);
        s.ready = s.entries.len();
        self.shared.changed.notify_all();
    }
    unsafe fn CancelRequestsWithTag(&self, mask: u64, value: u64) {
        for entry in &mut lock(&self.shared.state).entries {
            if let Entry::Read(r) = entry
                && (r.tag & mask) == value
            {
                r.cancelled = true;
            }
        }
    }
    unsafe fn Close(&self) {
        self.close()
    }
    unsafe fn GetErrorEvent(&self) -> *mut c_void {
        self.shared
            .error_event
            .try_clone()
            .map_or(core::ptr::null_mut(), IntoRawHandle::into_raw_handle)
    }
    unsafe fn RetrieveErrorRecord(&self, record: *mut ErrorRecord) {
        if record.is_null() {
            return;
        }
        let mut s = lock(&self.shared.state);
        if let Some(mut first) = s.first_error.take() {
            first.count = s.error_count;
            unsafe { record.write(*first) };
        } else {
            unsafe {
                record.write_bytes(0, 1);
                (*record).kind = u32::MAX;
            }
        }
        s.error_count = 0;
        let _ = unsafe { ResetEvent(HANDLE(self.shared.error_event.as_raw_handle())) };
    }
    unsafe fn Query(&self, info: *mut QueueInfo) {
        if info.is_null() {
            return;
        }
        let s = lock(&self.shared.state);
        let desc = QueueDesc {
            source_type: self.source_type,
            priority: self.priority,
            capacity: self.shared.capacity as u16,
            name: self.name.as_ref().map_or(core::ptr::null(), |n| n.as_ptr()),
        };
        unsafe {
            info.write(QueueInfo {
                desc,
                empty: (self.shared.capacity - 1 - s.entries.len() - usize::from(s.processing))
                    as u16,
                until_submit: if self.manual {
                    u16::MAX
                } else {
                    (self.shared.capacity / 2 - (s.entries.len() - s.ready)) as u16
                },
            })
        };
    }
}

impl IQueue2_Impl for Queue_Impl {
    unsafe fn EnqueueSetEvent(&self, event: *mut c_void) {
        use std::os::windows::io::BorrowedHandle;
        let entry = if event.is_null() || event as isize == -1 {
            Entry::Invalid(E_INVALIDARG, 3)
        } else {
            unsafe { BorrowedHandle::borrow_raw(event) }
                .try_clone_to_owned()
                .map_or_else(
                    |e| {
                        Entry::Invalid(HRESULT::from_win32(e.raw_os_error().unwrap_or(1) as u32), 3)
                    },
                    Entry::Event,
                )
        };
        self.enqueue(entry);
    }
    unsafe fn Query1(&self, info: *mut QueueInfo1) {
        if info.is_null() {
            return;
        }
        let mut base = std::mem::MaybeUninit::uninit();
        unsafe {
            self.Query(base.as_mut_ptr());
            let base: QueueInfo = base.assume_init();
            info.write(QueueInfo1 {
                desc: QueueDesc1 {
                    desc: base.desc,
                    options: self.manual as u64,
                },
                empty: base.empty,
                until_submit: base.until_submit,
            });
        }
    }
}
