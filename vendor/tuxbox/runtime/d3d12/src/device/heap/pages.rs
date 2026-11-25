//! Pages registered by the title and the pools that back them.
use super::*;

pub(in super::super) unsafe extern "system" fn register_pages(
    device: &Device, address: u64, count: u32, out: *mut usize,
) -> HRESULT {
    if out.is_null() { return E_POINTER; }
    unsafe { out.write(0) };
    if address == 0 || !address.is_multiple_of(65536) || count == 0 { return E_INVALIDARG; }
    let bytes = u64::from(count) * 65536;
    let Some(end) = address.checked_add(bytes) else { return E_INVALIDARG; };
    if let Err(e) = validate_shared_range(address, end) { return e.code(); }
    static FIRST: std::sync::Once = std::sync::Once::new();
    crate::diagnostics::log_once(&FIRST, || format!("registered Xbox GPU page pool: {count} pages at {address:#x}"));
    let token = device.state.page_pools.register(address, end);
    unsafe { out.write(token) };
    windows::Win32::Foundation::S_OK
}

// Registration returns an opaque cookie. Import the actual slices when a
// resource uses them, so a large title arena doesn't pin untouched pages.
#[derive(Default)]
pub(in super::super) struct PagePools {
    pub(super) pools: std::sync::Mutex<std::collections::HashMap<usize, Box<PagePool>>>,
    // Single-page imports outside every registered pool. Titles place many
    // resources on the same page: ProjectScarlett 1.0.2.1 imported one page
    // 9,352 times in its first minute. Each fresh host heap is another driver
    // buffer, and RADV passes every live buffer with each submission, so the
    // kernel's per-submit validation grows with the import count.
    //
    // Imports of several pages share this cache under a key that also holds
    // their page count. The same title views five fixed ranges of 448 KiB to
    // 1.2 MiB about five times a frame each, and its recording allocators
    // keep every import for 216 frames: about 4,500 live buffers.
    pub(super) loose: std::sync::Mutex<PageCache<ImportedHeap>>,
}

impl PagePools {
    pub(super) fn register(&self, start: u64, end: u64) -> usize {
        let pool = Box::new(PagePool { start, end, pages: PageCache::default() });
        let token = core::ptr::from_ref(pool.as_ref()).addr();
        self.pools.lock().unwrap_or_else(|e| e.into_inner()).insert(token, pool);
        token
    }

    pub(super) fn import(&self, device: &ID3D12Device13, base: u64, size: u64, pin: ImportPin, source: &'static str) -> windows::core::Result<Arc<ImportedHeap>> {
        let open = || -> windows::core::Result<Arc<ImportedHeap>> {
            let native = unsafe { device.OpenExistingHeapFromAddress1(base as _, size as usize) }.map_err(|error| {
                trace_import_failure(source, "sized", size, error.code());
                error
            })?;
            Ok(Arc::new(ImportedHeap { native, _pin: pin }))
        };
        if size == u64::from(D3D12_DEFAULT_RESOURCE_PLACEMENT_ALIGNMENT) {
            use std::sync::atomic::AtomicU64;
            let mut pools = self.pools.lock().unwrap_or_else(|e| e.into_inner());
            if let Some(pool) = pools.values_mut().find(|pool| pool.start <= base && base + size <= pool.end) {
                static OPEN: AtomicU64 = AtomicU64::new(0);
                static HIT: AtomicU64 = AtomicU64::new(0);
                return reuse_page(&mut pool.pages, base, open, [("pool_open", &OPEN), ("pool_hit", &HIT)], source, size);
            }
            drop(pools);
            static OPEN: AtomicU64 = AtomicU64::new(0);
            static HIT: AtomicU64 = AtomicU64::new(0);
            let mut loose = self.loose.lock().unwrap_or_else(|e| e.into_inner());
            return reuse_page(&mut loose, base, open, [("page_open", &OPEN), ("page_hit", &HIT)], source, size);
        }
        if let Some(key) = span_key(base, size) {
            use std::sync::atomic::AtomicU64;
            static OPEN: AtomicU64 = AtomicU64::new(0);
            static HIT: AtomicU64 = AtomicU64::new(0);
            let mut loose = self.loose.lock().unwrap_or_else(|e| e.into_inner());
            return reuse_page(&mut loose, key, open, [("span_open", &OPEN), ("span_hit", &HIT)], source, size);
        }
        open()
    }
}

// Interleaved allocations can revisit a page whose heap is still owned by a
// resource. Reuse only that exact live import, while its pages are unchanged.
pub(super) fn reuse_page(
    pages: &mut PageCache<ImportedHeap>, base: u64,
    open: impl FnOnce() -> windows::core::Result<Arc<ImportedHeap>>,
    [(open_path, opens), (hit_path, hits)]: [(&'static str, &std::sync::atomic::AtomicU64); 2],
    source: &'static str, size: u64,
) -> windows::core::Result<Arc<ImportedHeap>> {
    let mut opened = false;
    let result = pages.get_or_open(base, ImportedHeap::is_live, || {
        opened = true;
        open()
    });
    // The native resource owns the Arc; the cache is weak.
    // Count only successful imports and hits, not failed opens.
    if result.is_ok() {
        if opened {
            trace_buffer_path(open_path, opens, source, size);
        } else {
            trace_buffer_path(hit_path, hits, source, size);
        }
    }
    result
}
