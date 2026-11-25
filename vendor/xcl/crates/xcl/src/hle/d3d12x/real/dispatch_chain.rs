//! PSOs for a split compute entry. The first piece is the real PSO returned
//! to the caller. Other pieces and the lazily built whole form belong to it.
use super::*;
use crate::gpu::{dxil::kernel::DispatchChain, recomp::pso};
use std::sync::{Arc, LazyLock, OnceLock};

static PROGRAMS: AtomicU64 = AtomicU64::new(0);
static PIECES: AtomicU64 = AtomicU64::new(0);
static DISPATCHES: AtomicU64 = AtomicU64::new(0);

pub(crate) fn log_counts() {
    crate::xlog_info!("compute split: {} programs split, {} pieces built, {} chained dispatches issued",
        PROGRAMS.load(Ordering::Relaxed), PIECES.load(Ordering::Relaxed), DISPATCHES.load(Ordering::Relaxed));
}

struct PipelineBuilder {
    device: ID3D12Device,
    create: PsoFn,
    signature: ID3D12RootSignature,
    flags: D3D12_PIPELINE_STATE_FLAGS,
    node: u32,
    record: u64,
}

impl PipelineBuilder {
    unsafe fn pipeline(&self, dxil: &[u8], iid: &GUID) -> Option<ID3D12PipelineState> {
        let desc = D3D12_COMPUTE_PIPELINE_STATE_DESC { NodeMask: self.node, Flags: self.flags, ..Default::default() };
        let mut raw = std::ptr::null_mut();
        let (hr, _, _) = compute_container(self.device.as_raw(), self.create, &desc, self.signature.as_raw(),
            dxil, self.record, iid, &mut raw, true);
        (hr >= 0 && !raw.is_null()).then(|| ID3D12PipelineState::from_raw(raw))
    }
}

struct Chain {
    builder: Arc<PipelineBuilder>,
    tail: Vec<ID3D12PipelineState>,
    words: u32,
    whole_code: Vec<u8>,
    whole: OnceLock<ID3D12PipelineState>,
    build_whole: Mutex<()>,
}

impl Chain {
    unsafe fn whole(&self) -> Result<ID3D12PipelineState, String> {
        if let Some(p) = self.whole.get() { return Ok(p.clone()); }
        let _guard = self.build_whole.lock().unwrap();
        if let Some(p) = self.whole.get() { return Ok(p.clone()); }
        let p = self.builder.pipeline(&self.whole_code, &pc_iid(Kind::PipelineState)).ok_or("whole compute pipeline")?;
        let _ = self.whole.set(p.clone());
        Ok(p)
    }
}

static CHAINS: LazyLock<Mutex<HashMap<usize, Arc<Chain>>>> = LazyLock::new(Default::default);

pub(super) unsafe fn build(this: *mut c_void, create: PsoFn, desc: &D3D12_COMPUTE_PIPELINE_STATE_DESC,
    iid: *const GUID, result: *mut *mut c_void, record: u64, whole: &[u8], samplers: &[pso::StaticSampler],
    plan: Arc<DispatchChain>) -> Option<usize> {
    if result.is_null() || iid.is_null() || plan.pieces.len() < 2 { return None; }
    let dev = ID3D12Device::from_raw_borrowed(&this)?;
    let signature = pso::dispatch_chain_root_signature(dev, samplers).ok()?;
    let rs = signature.as_raw() as usize;
    let builder = Arc::new(PipelineBuilder { device: dev.clone(), create, signature,
        flags: desc.Flags, node: desc.NodeMask, record });
    let jobs = (0..plan.pieces.len()).map(|i| {
        let plan = plan.clone();
        let builder = builder.clone();
        let iid = if i == 0 { *iid } else { pc_iid(Kind::PipelineState) };
        Box::new(move || builder.pipeline(&plan.pieces[i], &iid).map_or(0, |p| p.into_raw() as usize)) as super::super::pso_async::Job
    }).collect();
    // Take ownership of every success, including when a different piece failed.
    let built: Vec<_> = super::super::pso_async::build_pieces(jobs).into_iter()
        .map(|p| (p != 0).then(|| ID3D12PipelineState::from_raw(p as *mut c_void))).collect();
    let mut pieces = built.into_iter().collect::<Option<Vec<_>>>()?.into_iter();
    let first = pieces.next()?;
    let chain = Arc::new(Chain { builder, tail: pieces.collect(), words: plan.dwords_per_group,
        whole_code: whole.to_vec(), whole: OnceLock::new(), build_whole: Mutex::new(()) });
    PROGRAMS.fetch_add(1, Ordering::Relaxed);
    PIECES.fetch_add(plan.pieces.len() as u64, Ordering::Relaxed);
    *result = first.into_raw();
    CHAINS.lock().unwrap().insert(*result as usize, chain);
    Some(rs)
}

pub(super) fn forget(pso: usize) {
    let gone = CHAINS.lock().unwrap().remove(&pso);
    drop(gone); // releasing a tail PSO also goes through forget_pso
}

pub(in crate::hle::d3d12x) unsafe fn whole(pso: usize) -> Result<Option<ID3D12PipelineState>, String> {
    let chain = CHAINS.lock().unwrap().get(&pso).cloned();
    chain.map(|c| c.whole()).transpose()
}

struct Recording {
    scratch: ID3D12Resource,
    first: ID3D12PipelineState,
    chain: Arc<Chain>,
}

static RECORDINGS: LazyLock<Mutex<HashMap<usize, Vec<Arc<Recording>>>>> = LazyLock::new(Default::default);

pub(in crate::hle::d3d12x) fn allocator_reset(allocator: usize) {
    let recordings = RECORDINGS.lock().unwrap().remove(&allocator);
    drop(recordings);
}

unsafe fn scratch_buffer(dev: &ID3D12Device, bytes: u64) -> Option<ID3D12Resource> {
    let desc = D3D12_RESOURCE_DESC {
        Dimension: D3D12_RESOURCE_DIMENSION_BUFFER, Width: bytes, Height: 1, DepthOrArraySize: 1, MipLevels: 1,
        SampleDesc: windows::Win32::Graphics::Dxgi::Common::DXGI_SAMPLE_DESC { Count: 1, Quality: 0 },
        Layout: D3D12_TEXTURE_LAYOUT_ROW_MAJOR, Flags: D3D12_RESOURCE_FLAG_ALLOW_UNORDERED_ACCESS, ..Default::default()
    };
    let mut resource = None;
    dev.CreateCommittedResource(&D3D12_HEAP_PROPERTIES { Type: D3D12_HEAP_TYPE_DEFAULT, CreationNodeMask: 1, VisibleNodeMask: 1, ..Default::default() },
        D3D12_HEAP_FLAG_NONE, &desc, D3D12_RESOURCE_STATE_UNORDERED_ACCESS, None, &mut resource).ok()?;
    resource
}

/// Called with the original six ABI views bound. The extended signature is
/// common to every piece and the whole fallback. Choose before recording any
/// part of the dispatch, including when scratch allocation fails.
pub(in crate::hle::d3d12x) unsafe fn dispatch(list: &ID3D12GraphicsCommandList, pso: usize, allocator: usize, grid: [u32; 3]) -> Result<(), String> {
    let [x, y, z] = grid;
    let chain = CHAINS.lock().unwrap().get(&pso).cloned();
    let Some(chain) = chain else { list.Dispatch(x, y, z); return Ok(()) };
    if grid.contains(&0) { return Ok(()); }
    // A title Reset can switch to a different PC allocator while this
    // recording is still submitted on the old one. Retain by backing.
    let allocator = if super::super::alloc_pool::enabled() {
        super::super::alloc_pool::recording_backing(list.as_raw() as usize).unwrap_or(0)
    } else { allocator };
    let size = grid.into_iter().try_fold(u64::from(chain.words) * 4, |s, n| s.checked_mul(n as u64));
    let scratch = size.filter(|&n| n <= u32::MAX as u64 + 1 && allocator != 0 && list.GetType() != D3D12_COMMAND_LIST_TYPE_BUNDLE)
        .and_then(|n| scratch_buffer(&chain.builder.device, n));
    let Some(scratch) = scratch else {
        list.SetPipelineState(&chain.whole()?);
        list.Dispatch(x, y, z);
        return Ok(());
    };
    let first = ID3D12PipelineState::from_raw_borrowed(&(pso as *mut c_void)).ok_or("first compute piece")?.clone();
    let held = Arc::new(Recording { scratch, first, chain });
    RECORDINGS.lock().unwrap().entry(allocator).or_default().push(held.clone());
    DISPATCHES.fetch_add(1, Ordering::Relaxed);
    pso::record_dispatch_chain(list, grid, &held.scratch, std::iter::once(&held.first).chain(&held.chain.tail));
    Ok(())
}
