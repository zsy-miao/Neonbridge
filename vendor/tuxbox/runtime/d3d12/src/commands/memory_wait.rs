//! Translate waits on host-visible Series fence words at a native list boundary.
//! The game can write markers and sample timestamps before waiting. Keep these
//! prefix operations ordered without disturbing bindings on the main command list.
use super::*;
use std::sync::atomic::{AtomicI32, AtomicU64};
use std::sync::mpsc;
use std::time::Duration;
use windows::Win32::Foundation::HANDLE;
use windows::core::Result;

pub(super) enum Packet {
    Marker(D3D12_WRITEBUFFERIMMEDIATE_PARAMETER),
    Timestamp(ID3D12QueryHeap, u32, ID3D12Resource, u64),
}

impl Packet {
    fn emit(&self, list: &ID3D12GraphicsCommandList) -> Result<()> {
        unsafe {
            match self {
                Self::Marker(write) => {
                    list.cast::<ID3D12GraphicsCommandList2>()?
                        .WriteBufferImmediate(1, write, None);
                }
                Self::Timestamp(heap, index, dest, offset) => {
                    list.EndQuery(heap, D3D12_QUERY_TYPE_TIMESTAMP, *index);
                    list.ResolveQueryData(
                        heap,
                        D3D12_QUERY_TYPE_TIMESTAMP,
                        *index,
                        1,
                        dest,
                        *offset,
                    );
                }
            }
        }
        Ok(())
    }
}

#[derive(Default)]
pub(super) struct Prefix {
    pending: Vec<Packet>,
    in_body: bool,
    pub(super) segments: Vec<Arc<Segment>>,
}

impl Prefix {
    pub(super) fn push(&mut self, list: &ID3D12GraphicsCommandList, packet: Packet) -> Result<()> {
        if self.in_body {
            return packet.emit(list);
        }
        self.pending.try_reserve(1).map_err(|_| E_OUTOFMEMORY)?;
        self.pending.push(packet);
        Ok(())
    }

    pub(super) fn finish(&mut self, list: &ID3D12GraphicsCommandList) -> Result<()> {
        if self.in_body {
            return Ok(());
        }
        self.in_body = true;
        for packet in self.pending.drain(..) {
            packet.emit(list)?;
        }
        Ok(())
    }
}

struct Word {
    resource: ID3D12Resource,
    mapped: usize,
}

impl Drop for Word {
    fn drop(&mut self) {
        unsafe { self.resource.Unmap(0, Some(&D3D12_RANGE::default())) };
    }
}

impl Word {
    fn map(list: &List, address: u64) -> Result<Self> {
        if address == 0 || !address.is_multiple_of(8) {
            return Err(E_INVALIDARG.into());
        }
        let (buffer, gpu) =
            unsafe { device::command_buffer_address(parent(&list.parent), address, 8)? };
        let resource = buffer.ok_or(E_NOTIMPL)?;
        let offset = gpu
            .checked_sub(unsafe { resource.GetGPUVirtualAddress() })
            .ok_or(E_INVALIDARG)? as usize;
        let range = D3D12_RANGE {
            Begin: offset,
            End: offset.checked_add(8).ok_or(E_INVALIDARG)?,
        };
        let mut base = ptr::null_mut();
        unsafe { resource.Map(0, Some(&range), Some(&mut base))? };
        if base.is_null() {
            unsafe { resource.Unmap(0, Some(&D3D12_RANGE::default())) };
            return Err(E_FAIL.into());
        }
        Ok(Self {
            resource,
            mapped: base.addr() + offset,
        })
    }

    fn reached(&self, value: u64) -> bool {
        // X64 aligned 64-bit load. The COM resource and its CPU mapping stay alive
        // until all queued waits release their segment.
        unsafe { AtomicU64::from_ptr(self.mapped as *mut u64).load(Ordering::Acquire) >= value }
    }
}

pub(super) struct Segment {
    prefix: Option<(ID3D12CommandList, ID3D12CommandAllocator)>,
    word: Word,
    target: u64,
}

pub(super) unsafe extern "system" fn wait_value64(
    list: &List,
    address: u64,
    comparison: u32,
    target: u64,
    flags: u32,
) {
    let mut state = recording(list);
    if !state.ensure_open(&list.native) {
        return;
    }
    let result = (|| -> Result<()> {
        if comparison != 7 || flags > 1 {
            return Err(E_NOTIMPL.into());
        }
        let word = Word::map(list, address)?;
        if target == 0 {
            return Ok(());
        }
        if state.prefix.in_body {
            crate::diagnostics::recording_error(format_args!(
                "GPU memory wait after rendering commands"
            ));
            return Err(E_NOTIMPL.into());
        }
        state
            .prefix
            .segments
            .try_reserve(1)
            .map_err(|_| E_OUTOFMEMORY)?;
        let prefix = if state.prefix.pending.is_empty() {
            None
        } else {
            let mut device: Option<ID3D12Device> = None;
            unsafe {
                list.native.GetDevice(&mut device)?;
            }
            let device = device.ok_or(E_FAIL)?;
            let kind = unsafe { list.native.GetType() };
            let allocator: ID3D12CommandAllocator = unsafe { device.CreateCommandAllocator(kind)? };
            let native: ID3D12GraphicsCommandList =
                unsafe { device.CreateCommandList(0, kind, &allocator, None)? };
            for packet in state.prefix.pending.drain(..) {
                packet.emit(&native)?;
            }
            unsafe {
                native.Close()?;
            }
            Some((native.into(), allocator))
        };
        state.prefix.segments.push(Arc::new(Segment {
            prefix,
            word,
            target,
        }));
        Ok(())
    })();
    if let Err(error) = result {
        crate::diagnostics::recording_error(format_args!(
            "64-bit GPU memory wait {address:#x} >= {target:#x} ({:?})",
            error.code()
        ));
        state.error = error.code();
    }
}

struct Job {
    fence: ID3D12Fence,
    segment: Arc<Segment>,
    device: ID3D12Device,
}

pub(super) struct Worker(mpsc::Sender<Job>);

impl Worker {
    fn new(error: Arc<AtomicI32>) -> Result<Self> {
        let (sender, jobs) = mpsc::channel::<Job>();
        std::thread::Builder::new()
            .name("TuxBox GPU memory waits".into())
            .spawn(move || {
                for job in jobs {
                    let result = (|| -> Result<()> {
                        unsafe { job.fence.SetEventOnCompletion(1, HANDLE::default())? };
                        while !job.segment.word.reached(job.segment.target) {
                            unsafe {
                                job.device.GetDeviceRemovedReason()?;
                            }
                            std::thread::sleep(Duration::from_millis(1));
                        }
                        unsafe { job.fence.Signal(2)?; }
                        static FIRST: std::sync::Once = std::sync::Once::new();
                        crate::diagnostics::log_once(&FIRST, || format!(
                            "GPU memory wait completed target={:#x}", job.segment.target
                        ));
                        Ok(())
                    })();
                    if let Err(cause) = result {
                        let _ = error.compare_exchange(
                            S_OK.0,
                            cause.code().0,
                            Ordering::AcqRel,
                            Ordering::Acquire,
                        );
                        // Fail the device rather than releasing work behind a broken gate.
                        if let Ok(device) = job.device.cast::<ID3D12Device5>() {
                            unsafe { device.RemoveDevice() };
                        }
                        break;
                    }
                }
            })
            .map_err(|_| E_OUTOFMEMORY)?;
        Ok(Self(sender))
    }
}

pub(super) type Splits = Vec<(usize, Arc<Segment>)>;

pub(super) unsafe fn submit(queue: &Queue, host: &[*mut c_void], splits: Splits) -> Result<()> {
    let run = |lists: &[*mut c_void]| {
        if !lists.is_empty() {
            unsafe {
                (queue.native.vtable().ExecuteCommandLists)(
                    queue.native.as_raw(),
                    lists.len() as u32,
                    lists.as_ptr(),
                )
            };
        }
    };
    let mut start = 0;
    for (index, segment) in splits {
        run(&host[start..index]);
        start = index;
        let mut device: Option<ID3D12Device> = None;
        unsafe {
            queue.native.GetDevice(&mut device)?;
        }
        let device = device.ok_or(E_FAIL)?;
        let fence = unsafe { device.CreateFence::<ID3D12Fence>(0, D3D12_FENCE_FLAG_NONE)? };
        let worker = match queue.state.wait_worker.get() {
            Some(worker) => worker,
            None => {
                let new = Worker::new(queue.state.placement.error.clone())?;
                queue.state.wait_worker.get_or_init(|| new)
            }
        };
        if let Some((prefix, _)) = &segment.prefix {
            run(&[prefix.as_raw()]);
        }
        // Fence 1 retires the entire prefix before the CPU reads the guest word.
        // Fence 2 holds the suffix on the GPU without blocking the guest submission thread.
        unsafe {
            queue.native.Signal(&fence, 1)?;
        }
        worker
            .0
            .send(Job {
                fence: fence.clone(),
                segment,
                device,
            })
            .map_err(|_| E_FAIL)?;
        unsafe {
            queue.native.Wait(&fence, 2)?;
        }
    }
    run(&host[start..]);
    Ok(())
}
