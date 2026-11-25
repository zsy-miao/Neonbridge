//! The pipelines the title created, on disk, so `neonbridge --pipelines`
//! builds them outside the title (XCL's pipeline journal and `xcl pipelines`:
//! ScarlettPort runtime/xcl/crates/xcl/src/hle/d3d12x/pso_journal.rs and
//! src/pipelines.rs, e980ecb and f14d3ff, re-derived for TuxBox guest streams).
//!
//! test140 created 80,000 pipelines (79,406 distinct) and the run's two
//! workers made a few hundred; the title dropped 4,426 dispatches in its last
//! 10 s waiting for them. Every pipeline a worker makes goes into the pipeline
//! library (`pipeline_cache`), named by its host description, so one made by
//! another process with the same translations is a library hit in the next
//! run. Here each pipeline the title creates lazily is written with its guest
//! shaders and root signature; the builder (`TuxBoxBuildPipelines`, in the
//! launcher's process) makes a guest device, recreates the roots and makes
//! each pipeline on the ordinary worker path, which stores it in the library.
//! Pipelines a run bound (`U`) are built first.
//!
//! `pipelines.journal` beside the pipeline library, append-only frames,
//! little-endian: tag u8, 0 u8 x3, payload length u32, hash u64, payload.
//! `S` a guest shader (hash = FNV-1a of its bytes), `R` a serialized root
//! signature (likewise), `G` a pipeline stream, `C` a slot 62 compute
//! pipeline, `U` (no payload) the pipeline was bound. `G`, `C` and `U` are
//! hashed by the pipeline's guest key (`pipeline_cache` keys). A frame is
//! written once across runs. TUXBOX_PIPELINE_JOURNAL=0 writes none.
use super::super::super::compute::{self, create_pipeline_x};
use super::super::super::{Device, identity};
use super::super::{AMPLIFICATION_X, MESH_X, STENCIL_X, layout};
use super::{NULLED, OwnedCompute, OwnedStream, WORKER};
use core::{ffi::c_void, mem::MaybeUninit, ptr, slice};
use std::collections::{HashMap, HashSet};
use std::ffi::{CStr, CString};
use std::io::{BufWriter, Read, Seek, SeekFrom, Write};
use std::sync::Mutex;
use std::sync::atomic::{AtomicU64, AtomicUsize, Ordering};
use std::time::{Duration, Instant};
use windows::Win32::Graphics::Direct3D12::*;
use windows::core::{IUnknown, Interface, PCSTR};

const MAGIC: &[u8; 8] = b"TBXPJ001";
const HEAD: usize = 16;
const FILE: &str = "pipelines.journal";

fn fnv64(bytes: &[u8]) -> u64 {
    bytes.iter().fold(0xcbf2_9ce4_8422_2325u64, |h, &b| (h ^ u64::from(b)).wrapping_mul(0x100_0000_01b3))
}

struct Journal {
    file: BufWriter<std::fs::File>,
    seen: HashSet<(u8, u64)>,
}

static JOURNAL: Mutex<Option<Option<Journal>>> = Mutex::new(None);

fn open() -> Option<Journal> {
    use std::os::windows::fs::OpenOptionsExt;
    if crate::env_toggle!("TUXBOX_PIPELINE_JOURNAL") == Some("0") {
        return None;
    }
    let path = crate::device::pipeline_cache::folder()?.join(FILE);
    // The only writer: a second title process gets no journal; the builder only reads.
    const FILE_SHARE_READ: u32 = 1;
    let mut file = std::fs::OpenOptions::new()
        .read(true)
        .write(true)
        .create(true)
        .truncate(false)
        .share_mode(FILE_SHARE_READ)
        .open(&path)
        .ok()?;
    let len = file.metadata().map_or(0, |m| m.len());
    let mut seen = HashSet::new();
    let mut magic = [0u8; 8];
    // Read through an 8 MiB buffer, skipping payloads inside it: a seek per
    // frame header took 5.0 s on the title's first pipeline (test168: 111827
    // frames in 335 MB). TUXBOX_JOURNAL_BUFFERED=0 seeks per frame as before.
    let buffered = crate::env_toggle!("TUXBOX_JOURNAL_BUFFERED") != Some("0");
    let mut reader = std::io::BufReader::with_capacity(if buffered { 8 << 20 } else { 0 }, &file);
    if reader.read_exact(&mut magic).is_ok() && &magic == MAGIC {
        // Headers only; a torn last frame (a run killed mid-write) is cut off.
        let mut at = MAGIC.len() as u64;
        let mut head = [0u8; HEAD];
        while reader.read_exact(&mut head).is_ok() {
            let payload = u64::from(u32::from_le_bytes(head[4..8].try_into().ok()?));
            let end = at + HEAD as u64 + payload;
            if end > len {
                break;
            }
            seen.insert((head[0], u64::from_le_bytes(head[8..16].try_into().ok()?)));
            at = end;
            if reader.seek_relative(payload as i64).is_err() {
                break;
            }
        }
        drop(reader);
        if at < len {
            let _ = file.set_len(at);
        }
    } else {
        file.set_len(0).ok()?;
        file.seek(SeekFrom::Start(0)).ok()?;
        file.write_all(MAGIC).ok()?;
    }
    file.seek(SeekFrom::End(0)).ok()?;
    let pipelines = seen.iter().filter(|(tag, _)| matches!(tag, b'G' | b'C')).count();
    crate::diagnostics::log_line(format_args!(
        "pipeline journal: {} ({pipelines} pipelines from earlier runs; `neonbridge --pipelines` builds them outside the title; TUXBOX_PIPELINE_JOURNAL=0 off)",
        path.display()));
    Some(Journal { file: BufWriter::with_capacity(1 << 20, file), seen })
}

/// Opens the journal on a thread of its own at startup, so the title's first
/// pipeline does not wait for its read (test168: 5.0 s before the first
/// pipeline). TUXBOX_JOURNAL_WARM=0 opens it at the first pipeline.
pub(in crate::device) fn warm() {
    if crate::env_toggle!("TUXBOX_JOURNAL_WARM") == Some("0") {
        return;
    }
    let _ = std::thread::Builder::new().name("tuxbox-journal-open".into()).spawn(|| with(|_| Ok(())));
}

/// Runs `f` on the journal; a write error turns it off for the run.
fn with(f: impl FnOnce(&mut Journal) -> std::io::Result<()>) {
    let mut journal = JOURNAL.lock().unwrap_or_else(|e| e.into_inner());
    let Some(j) = journal.get_or_insert_with(open).as_mut() else { return };
    if let Err(error) = f(j) {
        crate::diagnostics::log_line(format_args!("pipeline journal: {error}; off for this run"));
        *journal = Some(None);
    }
}

fn put(j: &mut Journal, tag: u8, hash: u64, payload: &[u8]) -> std::io::Result<()> {
    if !j.seen.insert((tag, hash)) {
        return Ok(());
    }
    let length = u32::try_from(payload.len()).map_err(|_| std::io::Error::other("frame over 4 GiB"))?;
    j.file.write_all(&[tag, 0, 0, 0])?;
    j.file.write_all(&length.to_le_bytes())?;
    j.file.write_all(&hash.to_le_bytes())?;
    j.file.write_all(payload)
}

/// Writes buffered frames out (the pipeline cache's saver, every 20 s).
pub(in crate::device) fn flush() {
    let mut journal = JOURNAL.lock().unwrap_or_else(|e| e.into_inner());
    if let Some(Some(j)) = journal.as_mut() {
        let _ = j.file.flush();
    }
}

fn u32s(out: &mut Vec<u8>, words: &[u32]) {
    for w in words {
        out.extend_from_slice(&w.to_le_bytes());
    }
}

fn name(out: &mut Vec<u8>, name: PCSTR) {
    if name.is_null() {
        out.extend_from_slice(&u32::MAX.to_le_bytes());
        return;
    }
    let bytes = unsafe { CStr::from_ptr(name.0.cast()) }.to_bytes();
    out.extend_from_slice(&(bytes.len() as u32).to_le_bytes());
    out.extend_from_slice(bytes);
}

/// The serialized root signature of a guest root proxy.
fn root_bytes(device: &Device, root: *mut c_void) -> Option<Option<Vec<u8>>> {
    if root.is_null() {
        return Some(None);
    }
    let proxy = unsafe { compute::root_proxy(root, identity(device).as_raw()) }.ok()??;
    Some(Some(proxy.state.serialized().to_vec()))
}

fn put_root(j: &mut Journal, root: Option<&[u8]>) -> std::io::Result<u64> {
    let Some(bytes) = root else { return Ok(0) };
    let hash = fnv64(bytes);
    put(j, b'R', hash, bytes)?;
    Ok(hash)
}

/// Notes a lazily made pipeline stream the title created.
pub(in crate::device) fn stream(device: &Device, key: Option<&str>, stream: &OwnedStream) {
    let Some(key) = key else { return };
    let Some(root) = root_bytes(device, stream._root.as_ref().map_or(ptr::null_mut(), Interface::as_raw)) else { return };
    let hash = fnv64(key.as_bytes());
    with(|j| {
        if j.seen.contains(&(b'G', hash)) {
            return Ok(());
        }
        let Some(payload) = (unsafe { serialize(j, stream, root.as_deref()) })? else { return Ok(()) };
        put(j, b'G', hash, &payload)
    });
}

/// Notes a lazily made compute pipeline the title created.
pub(in crate::device) fn compute(device: &Device, key: Option<&str>, compute: &OwnedCompute) {
    let Some(key) = key else { return };
    let Some(root) = root_bytes(device, compute.root.as_ref().map_or(ptr::null_mut(), Interface::as_raw)) else { return };
    let hash = fnv64(key.as_bytes());
    with(|j| {
        if j.seen.contains(&(b'C', hash)) {
            return Ok(());
        }
        let mut payload = Vec::with_capacity(24);
        payload.extend_from_slice(&put_root(j, root.as_deref())?.to_le_bytes());
        let cs = fnv64(&compute.code);
        put(j, b'S', cs, &compute.code)?;
        payload.extend_from_slice(&cs.to_le_bytes());
        u32s(&mut payload, &[compute.node_mask, compute.flags.0 as u32]);
        put(j, b'C', hash, &payload)
    });
}

/// The pipeline with this key was bound: the builder makes it first.
pub(in crate::device) fn used(key: &str) {
    with(|j| put(j, b'U', fnv64(key.as_bytes()), &[]));
}

/// A stream frame's payload: u32 size, the stream with each pointer replaced
/// (a root or shader by its hash, arrays by zero), then each array in record
/// order. `None`: a record this cannot own.
unsafe fn serialize(j: &mut Journal, stream: &OwnedStream, root: Option<&[u8]>) -> std::io::Result<Option<Vec<u8>>> {
    let size = stream.size;
    let mut bytes = vec![0u8; size];
    unsafe { ptr::copy_nonoverlapping(stream.storage.as_ptr().cast::<u8>(), bytes.as_mut_ptr(), size) };
    let mut extra = Vec::new();
    let mut at = 0;
    while at < size {
        let tag = i32::from_le_bytes(bytes[at..at + 4].try_into().unwrap());
        let Some((offset, length)) = layout(tag) else { return Ok(None) };
        if length > size - at {
            return Ok(None);
        }
        let payload = unsafe { bytes.as_mut_ptr().add(at + offset) };
        match tag {
            0 => {
                let hash = put_root(j, root)?;
                unsafe { payload.cast::<u64>().write_unaligned(hash) };
            }
            1..=6 | AMPLIFICATION_X | MESH_X => {
                let shader = unsafe { payload.cast::<D3D12_SHADER_BYTECODE>().read_unaligned() };
                let hash = if shader.BytecodeLength == 0 || shader.pShaderBytecode.is_null() {
                    0
                } else {
                    let code = unsafe { slice::from_raw_parts(shader.pShaderBytecode.cast::<u8>(), shader.BytecodeLength) };
                    let hash = fnv64(code);
                    put(j, b'S', hash, code)?;
                    hash
                };
                unsafe { payload.cast::<u64>().write_unaligned(hash) };
            }
            7 => {
                let mut so = unsafe { payload.cast::<D3D12_STREAM_OUTPUT_DESC>().read_unaligned() };
                for i in 0..so.NumEntries as usize {
                    let e = unsafe { &*so.pSODeclaration.add(i) };
                    u32s(&mut extra, &[e.Stream]);
                    name(&mut extra, e.SemanticName);
                    u32s(&mut extra, &[e.SemanticIndex, e.StartComponent.into(), e.ComponentCount.into(), e.OutputSlot.into()]);
                }
                for i in 0..so.NumStrides as usize {
                    u32s(&mut extra, &[unsafe { *so.pBufferStrides.add(i) }]);
                }
                so.pSODeclaration = ptr::null();
                so.pBufferStrides = ptr::null();
                unsafe { payload.cast::<D3D12_STREAM_OUTPUT_DESC>().write_unaligned(so) };
            }
            12 => {
                let mut input = unsafe { payload.cast::<D3D12_INPUT_LAYOUT_DESC>().read_unaligned() };
                for i in 0..input.NumElements as usize {
                    let e = unsafe { &*input.pInputElementDescs.add(i) };
                    name(&mut extra, e.SemanticName);
                    u32s(&mut extra, &[e.SemanticIndex, e.Format.0 as u32, e.InputSlot, e.AlignedByteOffset, e.InputSlotClass.0 as u32, e.InstanceDataStepRate]);
                }
                input.pInputElementDescs = ptr::null();
                unsafe { payload.cast::<D3D12_INPUT_LAYOUT_DESC>().write_unaligned(input) };
            }
            22 => {
                let mut views = unsafe { payload.cast::<D3D12_VIEW_INSTANCING_DESC>().read_unaligned() };
                for i in 0..views.ViewInstanceCount as usize {
                    let v = unsafe { &*views.pViewInstanceLocations.add(i) };
                    u32s(&mut extra, &[v.ViewportArrayIndex, v.RenderTargetArrayIndex]);
                }
                views.pViewInstanceLocations = ptr::null();
                unsafe { payload.cast::<D3D12_VIEW_INSTANCING_DESC>().write_unaligned(views) };
            }
            STENCIL_X => break,
            _ => {}
        }
        at += length;
    }
    let mut out = Vec::with_capacity(4 + size + extra.len());
    out.extend_from_slice(&(size as u32).to_le_bytes());
    out.extend_from_slice(&bytes);
    out.extend_from_slice(&extra);
    Ok(Some(out))
}

struct Reader<'a>(&'a [u8], usize);

impl Reader<'_> {
    fn u32(&mut self) -> Option<u32> {
        let v = u32::from_le_bytes(self.0.get(self.1..self.1 + 4)?.try_into().ok()?);
        self.1 += 4;
        Some(v)
    }
    fn u64(&mut self) -> Option<u64> {
        let v = u64::from_le_bytes(self.0.get(self.1..self.1 + 8)?.try_into().ok()?);
        self.1 += 8;
        Some(v)
    }
    fn name(&mut self, names: &mut Vec<CString>) -> Option<PCSTR> {
        let n = self.u32()?;
        if n == u32::MAX {
            return Some(PCSTR::null());
        }
        let owned = CString::new(self.0.get(self.1..self.1 + n as usize)?).ok()?;
        self.1 += n as usize;
        let pointer = PCSTR(owned.as_ptr().cast());
        names.push(owned);
        Some(pointer)
    }
}

/// A stream frame's pipeline, owning everything it points to; its root and
/// shaders from `root` and `shader` by hash.
unsafe fn deserialize(
    payload: &[u8],
    root: &dyn Fn(u64) -> Option<IUnknown>,
    shader: &dyn Fn(u64) -> Option<Vec<u8>>,
) -> Option<OwnedStream> {
    let mut r = Reader(payload, 0);
    let size = r.u32()? as usize;
    if size == 0 || size > 4096 || !size.is_multiple_of(size_of::<usize>()) {
        return None;
    }
    let bytes = payload.get(4..4 + size)?;
    r.1 = 4 + size;
    let mut storage = vec![MaybeUninit::<usize>::uninit(); size / size_of::<usize>()];
    let output = storage.as_mut_ptr().cast::<u8>();
    unsafe { ptr::copy_nonoverlapping(bytes.as_ptr(), output, size) };
    let mut owned = OwnedStream {
        storage: Vec::new(),
        size,
        _bytes: Vec::new(),
        _names: Vec::new(),
        _elements: Vec::new(),
        _declarations: Vec::new(),
        _strides: Vec::new(),
        _views: Vec::new(),
        _root: None,
        quad: false,
    };
    let mut at = 0;
    while at < size {
        let record = unsafe { output.add(at) };
        let tag = unsafe { record.cast::<i32>().read() };
        let (offset, length) = layout(tag)?;
        if length > size - at {
            return None;
        }
        let payload = unsafe { record.add(offset) };
        match tag {
            0 => {
                let hash = unsafe { payload.cast::<u64>().read() };
                let guest = if hash == 0 { None } else { Some(root(hash)?) };
                unsafe { payload.cast::<*mut c_void>().write(guest.as_ref().map_or(ptr::null_mut(), Interface::as_raw)) };
                owned._root = guest;
            }
            1..=6 | AMPLIFICATION_X | MESH_X => {
                let hash = unsafe { payload.cast::<u64>().read() };
                let shader_record = unsafe { &mut *payload.cast::<D3D12_SHADER_BYTECODE>() };
                if hash == 0 {
                    *shader_record = D3D12_SHADER_BYTECODE::default();
                } else {
                    let code = shader(hash)?;
                    if code.len() != shader_record.BytecodeLength {
                        return None;
                    }
                    shader_record.pShaderBytecode = code.as_ptr().cast();
                    owned._bytes.push(code);
                }
            }
            7 => {
                let so = unsafe { &mut *payload.cast::<D3D12_STREAM_OUTPUT_DESC>() };
                if so.NumEntries > 4096 || so.NumStrides > 4096 {
                    return None;
                }
                let mut entries = Vec::with_capacity(so.NumEntries as usize);
                for _ in 0..so.NumEntries {
                    let stream = r.u32()?;
                    let semantic = r.name(&mut owned._names)?;
                    let (index, start, count, slot) = (r.u32()?, r.u32()?, r.u32()?, r.u32()?);
                    entries.push(D3D12_SO_DECLARATION_ENTRY {
                        Stream: stream,
                        SemanticName: semantic,
                        SemanticIndex: index,
                        StartComponent: start as u8,
                        ComponentCount: count as u8,
                        OutputSlot: slot as u8,
                    });
                }
                let strides = (0..so.NumStrides).map(|_| r.u32()).collect::<Option<Vec<_>>>()?;
                so.pSODeclaration = if entries.is_empty() { ptr::null() } else { entries.as_ptr() };
                so.pBufferStrides = if strides.is_empty() { ptr::null() } else { strides.as_ptr() };
                owned._declarations.push(entries);
                owned._strides.push(strides);
            }
            12 => {
                let input = unsafe { &mut *payload.cast::<D3D12_INPUT_LAYOUT_DESC>() };
                if input.NumElements > 4096 {
                    return None;
                }
                let mut elements = Vec::with_capacity(input.NumElements as usize);
                for _ in 0..input.NumElements {
                    let semantic = r.name(&mut owned._names)?;
                    let w = [r.u32()?, r.u32()?, r.u32()?, r.u32()?, r.u32()?, r.u32()?];
                    elements.push(D3D12_INPUT_ELEMENT_DESC {
                        SemanticName: semantic,
                        SemanticIndex: w[0],
                        Format: windows::Win32::Graphics::Dxgi::Common::DXGI_FORMAT(w[1] as i32),
                        InputSlot: w[2],
                        AlignedByteOffset: w[3],
                        InputSlotClass: D3D12_INPUT_CLASSIFICATION(w[4] as i32),
                        InstanceDataStepRate: w[5],
                    });
                }
                input.pInputElementDescs = if elements.is_empty() { ptr::null() } else { elements.as_ptr() };
                owned._elements.push(elements);
            }
            22 => {
                let views = unsafe { &mut *payload.cast::<D3D12_VIEW_INSTANCING_DESC>() };
                if views.ViewInstanceCount > 4096 {
                    return None;
                }
                let locations = (0..views.ViewInstanceCount)
                    .map(|_| Some(D3D12_VIEW_INSTANCE_LOCATION { ViewportArrayIndex: r.u32()?, RenderTargetArrayIndex: r.u32()? }))
                    .collect::<Option<Vec<_>>>()?;
                views.pViewInstanceLocations = if locations.is_empty() { ptr::null() } else { locations.as_ptr() };
                owned._views.push(locations);
            }
            19 => unsafe { payload.cast::<D3D12_CACHED_PIPELINE_STATE>().write(Default::default()) },
            STENCIL_X => break,
            _ => {}
        }
        at += length;
    }
    owned.storage = storage;
    Some(owned)
}

/// Commit the system can still give, in bytes.
fn commit_free() -> u64 {
    use windows::Win32::System::SystemInformation::{GlobalMemoryStatusEx, MEMORYSTATUSEX};
    let mut status = MEMORYSTATUSEX { dwLength: size_of::<MEMORYSTATUSEX>() as u32, ..Default::default() };
    if unsafe { GlobalMemoryStatusEx(&mut status) }.is_err() {
        return u64::MAX;
    }
    status.ullAvailPageFile
}

/// The guest device and root proxies, used from the build threads.
struct Shared<T>(T);

// SAFETY: D3D12 devices, root signatures and their proxies are free-threaded.
unsafe impl<T> Send for Shared<T> {}
unsafe impl<T> Sync for Shared<T> {}

impl<T> Shared<T> {
    fn get(&self) -> &T {
        &self.0
    }
}

struct Frame {
    tag: u8,
    hash: u64,
    start: usize,
    end: usize,
}

/// Builds the journal's pipelines into the pipeline library on `threads`
/// threads: those a run bound first, then the rest in creation order. Each
/// waits while less than TUXBOX_PIPELINE_BUILD_FREE_GB (6) of commit is free;
/// the build stops once the library reaches 90% of TUXBOX_PIPELINE_LIBRARY_MB,
/// past which a run would start it afresh. Returns the pipelines made.
pub(in crate::device) fn build(threads: u32) -> u32 {
    let mut out = ptr::null_mut();
    let status = unsafe { crate::tuxbox_create_device(ptr::null_mut(), 0, &ID3D12Device::IID, &mut out) };
    if status.is_err() || out.is_null() {
        crate::diagnostics::log_line(format_args!("pipeline build: no device ({:#x})", status.0 as u32));
        return 0;
    }
    let owner = unsafe { IUnknown::from_raw(out) };
    let Some(folder) = crate::device::pipeline_cache::folder() else {
        crate::diagnostics::log_line(format_args!("pipeline build: no translation cache folder (no shader worker session)"));
        return 0;
    };
    let path = folder.join(FILE);
    let Ok(bytes) = std::fs::read(&path) else {
        crate::diagnostics::log_line(format_args!("pipeline build: no journal at {}; run the title once to write it", path.display()));
        return 0;
    };
    if bytes.get(..MAGIC.len()) != Some(MAGIC) {
        crate::diagnostics::log_line(format_args!("pipeline build: {} is not a pipeline journal", path.display()));
        return 0;
    }
    let mut frames = Vec::new();
    let mut at = MAGIC.len();
    while at + HEAD <= bytes.len() {
        let n = u32::from_le_bytes(bytes[at + 4..at + 8].try_into().unwrap()) as usize;
        if at + HEAD + n > bytes.len() {
            break;
        }
        let hash = u64::from_le_bytes(bytes[at + 8..at + 16].try_into().unwrap());
        frames.push(Frame { tag: bytes[at], hash, start: at + HEAD, end: at + HEAD + n });
        at += HEAD + n;
    }
    let lookup = |tag: u8| -> HashMap<u64, (usize, usize)> {
        frames.iter().filter(|f| f.tag == tag).map(|f| (f.hash, (f.start, f.end))).collect()
    };
    let (shaders, roots_at) = (lookup(b'S'), lookup(b'R'));
    let used: HashSet<u64> = frames.iter().filter(|f| f.tag == b'U').map(|f| f.hash).collect();
    let mut order: Vec<&Frame> = frames.iter().filter(|f| matches!(f.tag, b'G' | b'C')).collect();
    order.sort_by_key(|f| !used.contains(&f.hash));
    let bound = order.iter().filter(|f| used.contains(&f.hash)).count();
    let floor = crate::env_toggle!("TUXBOX_PIPELINE_BUILD_FREE_GB").and_then(|v| v.trim().parse::<u64>().ok()).unwrap_or(6) << 30;
    let cap = crate::device::pipeline_cache::library_cap_mb() << 20;
    crate::diagnostics::log_line(format_args!(
        "pipeline build: {} pipelines in {} ({bound} bound by a run, built first) on {threads} threads; waits while under {} GiB of commit is free",
        order.len(), path.display(), floor >> 30));
    let device = Shared(unsafe { &*owner.as_raw().cast::<Device>() });
    let roots: Shared<Mutex<HashMap<u64, Option<IUnknown>>>> = Shared(Mutex::new(HashMap::new()));
    let root = |hash: u64| -> Option<IUnknown> {
        let mut roots = roots.get().lock().unwrap_or_else(|e| e.into_inner());
        let made = roots
            .entry(hash)
            .or_insert_with(|| {
                let &(start, end) = roots_at.get(&hash)?;
                let blob = &bytes[start..end];
                let mut made = ptr::null_mut();
                let status = unsafe {
                    compute::create_root_signature(device.get(), 0, blob.as_ptr(), blob.len(), &ID3D12RootSignature::IID, &mut made)
                };
                (status.is_ok() && !made.is_null()).then(|| unsafe { IUnknown::from_raw(made) })
            })
            .clone();
        made
    };
    let root = Shared(root);
    let shader = |hash: u64| shaders.get(&hash).map(|&(start, end)| bytes[start..end].to_vec());
    let next = AtomicUsize::new(0);
    let (made, failed) = (AtomicU64::new(0), AtomicU64::new(0));
    // test143's --pipelines said "83497 made" while the library grew 0.2 MB:
    // every pipeline the driver accepted counted, stored or not. Each outcome
    // is counted apart now, and the export returns only what is in the library.
    let (nulled, unanswered, unreadable, refused) = (AtomicU64::new(0), AtomicU64::new(0), AtomicU64::new(0), AtomicU64::new(0));
    let (loaded_before, stored_before) = crate::device::pipeline_cache::library_counts();
    let capped_before = crate::device::pipeline_cache::capped();
    let full = std::sync::atomic::AtomicBool::new(false);
    let started = Instant::now();
    std::thread::scope(|scope| {
        for _ in 0..threads.max(1) {
            scope.spawn(|| {
                WORKER.with(|w| w.set(true));
                crate::device::shader_cache::wait_unbounded();
                crate::device::pipeline_cache::offline_thread();
                loop {
                    let i = next.fetch_add(1, Ordering::Relaxed);
                    let Some(frame) = order.get(i) else { break };
                    if full.load(Ordering::Relaxed) {
                        break;
                    }
                    let mut waited = false;
                    while commit_free() < floor {
                        if !waited {
                            crate::diagnostics::log_line(format_args!("pipeline build: under {} GiB of commit free; waiting", floor >> 30));
                            waited = true;
                        }
                        std::thread::sleep(Duration::from_secs(2));
                    }
                    let payload = &bytes[frame.start..frame.end];
                    let _ = crate::device::shader_cache::take_misses();
                    let outcome = crate::big_stack::run(|| unsafe { build_one(device.get(), frame.tag, payload, root.get(), &shader) });
                    if !crate::device::shader_cache::take_misses().is_empty() {
                        unanswered.fetch_add(1, Ordering::Relaxed);
                    }
                    match outcome {
                        Built::Made => {}
                        Built::Nulled => { nulled.fetch_add(1, Ordering::Relaxed); }
                        Built::Unreadable => { unreadable.fetch_add(1, Ordering::Relaxed); }
                        Built::Refused => { refused.fetch_add(1, Ordering::Relaxed); }
                    }
                    let ok = matches!(outcome, Built::Made);
                    let n = if ok { made.fetch_add(1, Ordering::Relaxed) + 1 } else { failed.fetch_add(1, Ordering::Relaxed); made.load(Ordering::Relaxed) };
                    if ok && n.is_multiple_of(64) {
                        let size = crate::device::pipeline_cache::library_bytes();
                        if cap != 0 && size >= cap / 10 * 9 {
                            crate::diagnostics::log_line(format_args!(
                                "pipeline build: library at {} MiB, 90% of TUXBOX_PIPELINE_LIBRARY_MB={}; stopping", size >> 20, cap >> 20));
                            full.store(true, Ordering::Relaxed);
                        }
                    }
                    if ok && (n.is_power_of_two() || n.is_multiple_of(500)) {
                        crate::diagnostics::log_line(format_args!(
                            "pipeline build: {n} made, {} failed, {} left, {:.0} s",
                            failed.load(Ordering::Relaxed), order.len().saturating_sub(i + 1), started.elapsed().as_secs_f64()));
                        crate::device::pipeline_cache::save_now();
                    }
                }
            });
        }
    });
    crate::device::pipeline_cache::save_now();
    let made = made.load(Ordering::Relaxed);
    let (loaded_after, stored_after) = crate::device::pipeline_cache::library_counts();
    let (loaded, stored) = (loaded_after - loaded_before, stored_after - stored_before);
    let g = |c: &AtomicU64| c.load(Ordering::Relaxed);
    crate::diagnostics::log_line(format_args!(
        "pipeline build: done in {:.0} s, {} of {} journal pipelines tried: {stored} stored into the library, {loaded} already in it,          {} made but not in the library; {} with null shaders (untranslated), {} with a translation request unanswered,          {} unreadable (shader or root signature missing from the journal), {} refused by the driver,          of them {} left to the runtime for their DXIL size (TUXBOX_PIPELINE_MAX_KB)",
        started.elapsed().as_secs_f64(), next.load(Ordering::Relaxed).min(order.len()), order.len(),
        made.saturating_sub(stored + loaded), g(&nulled), g(&unanswered), g(&unreadable), g(&refused),
        crate::device::pipeline_cache::capped() - capped_before));
    drop(roots);
    drop(owner);
    (stored + loaded).min(u64::from(u32::MAX)) as u32
}

/// What became of one journal pipeline.
enum Built {
    Made,
    /// Made with null shaders: a stage had no translation.
    Nulled,
    /// Its record, root signature or shader is missing from the journal.
    Unreadable,
    /// The driver (or our create path) refused it.
    Refused,
}

/// Makes one journal pipeline on the worker path (its library store included).
unsafe fn build_one(
    device: &Device,
    tag: u8,
    payload: &[u8],
    root: &dyn Fn(u64) -> Option<IUnknown>,
    shader: &dyn Fn(u64) -> Option<Vec<u8>>,
) -> Built {
    let iid = ID3D12PipelineState::IID;
    let mut out = ptr::null_mut();
    NULLED.with(|n| n.set(false));
    let status = match tag {
        b'G' => {
            let Some(mut stream) = (unsafe { deserialize(payload, root, shader) }) else { return Built::Unreadable };
            let desc = stream.desc();
            unsafe { super::super::create_pipeline(device, &desc, &iid, &mut out) }
        }
        _ => {
            let mut r = Reader(payload, 0);
            let (Some(root_hash), Some(cs), Some(node_mask), Some(flags)) = (r.u64(), r.u64(), r.u32(), r.u32()) else { return Built::Unreadable };
            let guest_root = if root_hash == 0 { None } else {
                // The guest root proxy, typed as the title passes it (no QueryInterface).
                let proxy = root(root_hash);
                let raw = proxy.as_ref().map(Interface::as_raw);
                match raw.as_ref().and_then(|raw| unsafe { ID3D12RootSignature::from_raw_borrowed(raw) }).cloned() {
                    Some(r) => Some(r),
                    None => return Built::Unreadable,
                }
            };
            let Some(code) = shader(cs) else { return Built::Unreadable };
            let compute = OwnedCompute { code, root: guest_root, node_mask, flags: D3D12_PIPELINE_STATE_FLAGS(flags as i32) };
            let desc = compute.desc();
            unsafe { create_pipeline_x(device, &desc, 0, ptr::null(), &iid, &mut out) }
        }
    };
    let nulled = NULLED.with(|n| n.replace(false));
    let made = status.is_ok() && !out.is_null();
    if made {
        drop(unsafe { IUnknown::from_raw(out) });
    }
    match (made, nulled) {
        (true, false) => Built::Made,
        (true, true) => Built::Nulled,
        _ => Built::Refused,
    }
}

