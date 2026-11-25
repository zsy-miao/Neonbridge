//! GPU time per submitted segment (TUXBOX_GPU_SEGMENTS=1, diagnostic).
//! Each ExecuteCommandLists of title lists is bracketed by two timestamp
//! lists of our own on the same queue, so a segment's GPU time is the
//! title work it ran, and the time from one segment's end to the next one's
//! start is what the queue spent elsewhere (our refresh/upload lists, waits
//! on fences). Every 2 s each queue logs its busy time per second, the
//! segment count, the longest segments with their list counts and the
//! longest gaps. test149: the intro's GPU latency stayed ~100 ms a frame
//! with shadows ruled out; this says where it goes.
use std::collections::HashMap;
use std::sync::{Mutex, OnceLock};
use std::time::Instant;
use windows::Win32::Graphics::Direct3D12::*;
use windows::core::Interface;

const SLOTS: u32 = 4096;
const LISTS: usize = 512;

pub(crate) fn enabled() -> bool {
    static ON: OnceLock<bool> = OnceLock::new();
    *ON.get_or_init(|| crate::env_toggle!("TUXBOX_GPU_SEGMENTS") == Some("1"))
}

struct Stamp {
    allocator: ID3D12CommandAllocator,
    list: ID3D12GraphicsCommandList,
    /// Fence value after which this stamp list may be reset.
    done: u64,
}

struct Segment {
    start: u32,
    end: u32,
    lists: usize,
    fence: u64,
}

struct Queue {
    heap: ID3D12QueryHeap,
    readback: ID3D12Resource,
    mapped: *const u64,
    fence: ID3D12Fence,
    value: u64,
    frequency: f64,
    stamps: Vec<Stamp>,
    next_stamp: usize,
    next_slot: u32,
    pending: Vec<Segment>,
    last_end: Option<u64>,
    window: Instant,
    busy_ms: f64,
    count: u64,
    longest: Vec<(f64, usize)>,
    gaps: Vec<f64>,
}

unsafe impl Send for Queue {}

static QUEUES: Mutex<Option<HashMap<usize, Option<Queue>>>> = Mutex::new(None);

fn make(queue: &ID3D12CommandQueue) -> Option<Queue> {
    unsafe {
        let kind = queue.GetDesc().Type;
        if kind != D3D12_COMMAND_LIST_TYPE_DIRECT && kind != D3D12_COMMAND_LIST_TYPE_COMPUTE {
            return None;
        }
        let mut device: Option<ID3D12Device> = None;
        queue.GetDevice(&mut device).ok()?;
        let device = device?;
        let mut heap: Option<ID3D12QueryHeap> = None;
        device
            .CreateQueryHeap(&D3D12_QUERY_HEAP_DESC { Type: D3D12_QUERY_HEAP_TYPE_TIMESTAMP, Count: SLOTS, NodeMask: 0 }, &mut heap)
            .ok()?;
        let desc = D3D12_RESOURCE_DESC {
            Dimension: D3D12_RESOURCE_DIMENSION_BUFFER,
            Width: u64::from(SLOTS) * 8,
            Height: 1,
            DepthOrArraySize: 1,
            MipLevels: 1,
            SampleDesc: windows::Win32::Graphics::Dxgi::Common::DXGI_SAMPLE_DESC { Count: 1, Quality: 0 },
            Layout: D3D12_TEXTURE_LAYOUT_ROW_MAJOR,
            ..Default::default()
        };
        let props = D3D12_HEAP_PROPERTIES { Type: D3D12_HEAP_TYPE_READBACK, ..Default::default() };
        let mut readback: Option<ID3D12Resource> = None;
        device
            .CreateCommittedResource(&props, D3D12_HEAP_FLAG_NONE, &desc, D3D12_RESOURCE_STATE_COPY_DEST, None, &mut readback)
            .ok()?;
        let readback = readback?;
        let mut mapped = std::ptr::null_mut();
        readback.Map(0, None, Some(&mut mapped)).ok()?;
        let fence: ID3D12Fence = device.CreateFence(0, D3D12_FENCE_FLAG_NONE).ok()?;
        let frequency = queue.GetTimestampFrequency().ok()? as f64;
        let mut stamps = Vec::with_capacity(LISTS);
        for _ in 0..LISTS {
            let allocator: ID3D12CommandAllocator = device.CreateCommandAllocator(kind).ok()?;
            let list: ID3D12GraphicsCommandList = device.CreateCommandList(0, kind, &allocator, None).ok()?;
            list.Close().ok()?;
            stamps.push(Stamp { allocator, list, done: 0 });
        }
        crate::diagnostics::log_line(format_args!("gpu segments: timing queue {:#x} (type {}) at {frequency} Hz (TUXBOX_GPU_SEGMENTS)", queue.as_raw() as usize, kind.0));
        Some(Queue {
            heap: heap?,
            readback,
            mapped: mapped.cast(),
            fence,
            value: 0,
            frequency,
            stamps,
            next_stamp: 0,
            next_slot: 0,
            pending: Vec::new(),
            last_end: None,
            window: Instant::now(),
            busy_ms: 0.0,
            count: 0,
            longest: Vec::new(),
            gaps: Vec::new(),
        })
    }
}

impl Queue {
    /// Records one timestamp on `queue`; None when the ring is still busy.
    fn stamp(&mut self, queue: &ID3D12CommandQueue) -> Option<u32> {
        let completed = unsafe { self.fence.GetCompletedValue() };
        let s = &mut self.stamps[self.next_stamp];
        if s.done > completed {
            return None;
        }
        let slot = self.next_slot;
        unsafe {
            s.allocator.Reset().ok()?;
            s.list.Reset(&s.allocator, None).ok()?;
            s.list.EndQuery(&self.heap, D3D12_QUERY_TYPE_TIMESTAMP, slot);
            s.list.ResolveQueryData(&self.heap, D3D12_QUERY_TYPE_TIMESTAMP, slot, 1, &self.readback, u64::from(slot) * 8);
            s.list.Close().ok()?;
            queue.ExecuteCommandLists(&[Some(s.list.cast().ok()?)]);
            self.value += 1;
            queue.Signal(&self.fence, self.value).ok()?;
        }
        s.done = self.value;
        self.next_stamp = (self.next_stamp + 1) % LISTS;
        self.next_slot = (self.next_slot + 1) % SLOTS;
        Some(slot)
    }

    fn drain(&mut self) {
        let completed = unsafe { self.fence.GetCompletedValue() };
        let mut i = 0;
        while i < self.pending.len() {
            if self.pending[i].fence > completed {
                i += 1;
                continue;
            }
            let seg = self.pending.remove(i);
            let (a, b) = unsafe { (*self.mapped.add(seg.start as usize), *self.mapped.add(seg.end as usize)) };
            let ms = b.saturating_sub(a) as f64 * 1000.0 / self.frequency;
            if let Some(last) = self.last_end
                && a > last
            {
                self.gaps.push((a - last) as f64 * 1000.0 / self.frequency);
            }
            self.last_end = Some(b);
            self.busy_ms += ms;
            self.count += 1;
            self.longest.push((ms, seg.lists));
        }
    }

    fn report(&mut self, queue: usize) {
        let secs = self.window.elapsed().as_secs_f64();
        if secs < 2.0 {
            return;
        }
        self.longest.sort_by(|x, y| y.0.total_cmp(&x.0));
        self.gaps.sort_by(|x, y| y.total_cmp(x));
        let top: Vec<String> = self.longest.iter().take(5).map(|(ms, n)| format!("{ms:.2} ms ({n} lists)")).collect();
        let gaps: Vec<String> = self.gaps.iter().take(5).map(|ms| format!("{ms:.2}")).collect();
        let gap_sum: f64 = self.gaps.iter().sum();
        crate::diagnostics::log_line(format_args!(
            "gpu segments: queue {queue:#x}: {} segments in {secs:.1} s, title work {:.1} ms/s, between segments {:.1} ms/s; longest {}; longest gaps ms {}",
            self.count, self.busy_ms / secs, gap_sum / secs, top.join(", "), gaps.join(", ")
        ));
        self.window = Instant::now();
        self.busy_ms = 0.0;
        self.count = 0;
        self.longest.clear();
        self.gaps.clear();
    }
}

/// Brackets one ExecuteCommandLists of `lists` title lists on `queue`.
pub(crate) fn around(queue: &ID3D12CommandQueue, lists: usize, execute: impl FnOnce()) {
    if !enabled() {
        return execute();
    }
    let key = queue.as_raw() as usize;
    let mut map = QUEUES.lock().unwrap_or_else(|e| e.into_inner());
    let entry = map.get_or_insert_with(HashMap::new).entry(key).or_insert_with(|| make(queue));
    let Some(q) = entry.as_mut() else {
        drop(map);
        return execute();
    };
    let start = q.stamp(queue);
    execute();
    if let (Some(start), Some(end)) = (start, q.stamp(queue)) {
        let fence = q.value;
        q.pending.push(Segment { start, end, lists, fence });
    }
    q.drain();
    q.report(key);
}
