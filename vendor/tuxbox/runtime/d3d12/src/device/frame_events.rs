//! Per-device frame origins and display-flip notifications (Scarlett slots 78–81).
use super::Device;
use core::ffi::c_void;
use std::collections::VecDeque;
use std::os::windows::io::{AsRawHandle, FromRawHandle, OwnedHandle};
use std::sync::{Condvar, Mutex};
use std::time::{Duration, Instant};
use windows::Win32::Foundation::{
    DUPLICATE_SAME_ACCESS, DuplicateHandle, E_INVALIDARG, E_NOTIMPL, HANDLE, S_FALSE, S_OK,
};
use windows::Win32::Graphics::Dxgi::{IDXGIAdapter, IDXGIOutput};
use windows::Win32::System::Performance::QueryPerformanceCounter;
use windows::Win32::System::Threading::{GetCurrentProcess, SetEvent};
use windows::core::{HRESULT, Interface};

#[derive(Default)]
pub(super) struct FrameEvents {
    state: Mutex<State>,
    changed: Condvar,
}

#[derive(Default)]
struct State {
    interval: Duration,
    period: usize,
    next_origin: Option<Instant>,
    last_token: u64,
    pending: Option<u64>,
    issued: VecDeque<u64>,
    displayed: VecDeque<(u64, i64)>,
    // Tokens increase monotonically; every token up to this one has flipped.
    last_displayed: u64,
    flip: Vec<OwnedHandle>,
    flip_scheduled: bool,
}

pub(super) unsafe extern "system" fn interval(
    device: &Device,
    output: *mut c_void,
    length_us: u32,
    period: u32,
    flags: u32,
) -> HRESULT {
    if flags != 0 {
        return E_NOTIMPL;
    }
    if !(1..=4).contains(&period) {
        return E_INVALIDARG;
    }
    // The packaged driver accepts small timing variations around each nominal
    // rate. Its 120/40 Hz modes require additional output capability checks.
    match length_us {
        16667..=16683 | 33333..=33367 | 50000..=50049 | 66667..=66734 => {}
        8333..=8342 | 25000..=25025 => return E_NOTIMPL,
        _ => return E_INVALIDARG,
    }
    if let Some(output) = unsafe { IDXGIOutput::from_raw_borrowed(&output) } {
        let adapter = match unsafe { output.GetParent::<IDXGIAdapter>().and_then(|a| a.GetDesc()) }
        {
            Ok(adapter) => adapter,
            Err(error) => return error.code(),
        };
        let luid = unsafe { device.native.GetAdapterLuid() };
        if adapter.AdapterLuid != luid {
            return E_INVALIDARG;
        }
    }
    let frames = &device.state.frames;
    let mut state = frames.state.lock().unwrap_or_else(|e| e.into_inner());
    let interval = Duration::from_micros(length_us.into());
    if state.interval != interval {
        state.next_origin = None;
        state.pending = None;
        state.issued.clear();
        state.displayed.clear();
        state.last_displayed = 0;
        state.flip.clear();
        state.flip_scheduled = false;
    }
    state.interval = interval;
    state.period = period as usize;
    frames.changed.notify_all();
    S_OK
}

#[repr(C)]
pub(super) struct SignalList {
    count: u32,
    handles: *const HANDLE,
}

unsafe fn duplicate_signals(list: *const SignalList) -> Result<Vec<OwnedHandle>, HRESULT> {
    let Some(list) = (unsafe { list.as_ref() }) else {
        return Ok(Vec::new());
    };
    if list.count > 7 || (list.count == 0) != list.handles.is_null() {
        return Err(E_INVALIDARG);
    }
    let process = unsafe { GetCurrentProcess() };
    let mut owned = Vec::with_capacity(list.count as usize);
    for index in 0..list.count as usize {
        let handle = unsafe { list.handles.add(index).read_unaligned() };
        if handle.is_invalid()
            || (0..index).any(|i| unsafe { list.handles.add(i).read_unaligned() } == handle)
        {
            return Err(E_INVALIDARG);
        }
        let mut copy = HANDLE::default();
        unsafe {
            DuplicateHandle(
                process,
                handle,
                process,
                &mut copy,
                0,
                false,
                DUPLICATE_SAME_ACCESS,
            )
        }
        .map_err(|e| e.code())?;
        // DuplicateHandle transferred a new, independently owned kernel handle.
        owned.push(unsafe { OwnedHandle::from_raw_handle(copy.0) });
    }
    Ok(owned)
}

pub(super) unsafe extern "system" fn schedule(
    device: &Device,
    kind: u32,
    offset_us: u32,
    signals: *const SignalList,
    flags: u32,
) -> HRESULT {
    let frames = &device.state.frames;
    let mut state = frames.state.lock().unwrap_or_else(|e| e.into_inner());
    let offset = Duration::from_micros(offset_us.into());
    if offset >= state.interval {
        return E_INVALIDARG;
    }
    match kind {
        0 => {
            if flags != 0 || !signals.is_null() {
                return E_NOTIMPL;
            }
            state.next_origin = Some(Instant::now() + state.interval + offset);
        }
        2 => {
            if state.next_origin.is_none() {
                return E_INVALIDARG;
            }
            if offset_us != 0 || flags > 1 {
                return E_NOTIMPL;
            }
            if flags == 1 && !signals.is_null() {
                return E_INVALIDARG;
            }
            state.flip = match unsafe { duplicate_signals(signals) } {
                Ok(handles) => handles,
                Err(error) => return error,
            };
            state.flip_scheduled = flags == 0;
        }
        _ => return E_NOTIMPL,
    }
    frames.changed.notify_all();
    S_OK
}

pub(super) unsafe extern "system" fn wait(
    device: &Device,
    kind: u32,
    timeout_ms: u32,
    objects: *const c_void,
    flags: u32,
    token: *mut u64,
) -> HRESULT {
    if token.is_null() {
        return E_INVALIDARG;
    }
    if !matches!(kind, 0 | 2) || !objects.is_null() || !matches!(flags, 0 | 2) {
        return E_NOTIMPL;
    }
    let mut input = unsafe { token.read_unaligned() };
    let frames = &device.state.frames;
    let mut state = frames.state.lock().unwrap_or_else(|e| e.into_inner());
    // ProjectScarlett clears its token before every timeout poll (RVA 0x248241e)
    // and asserts on failure. A fresh poll continues the outstanding origin.
    let continued = kind == 0 && input == 0 && flags == 2 && state.pending.is_some();
    if continued {
        input = state.pending.unwrap_or_default();
    }
    let timeout =
        (timeout_ms != u32::MAX).then(|| Instant::now() + Duration::from_millis(timeout_ms.into()));
    loop {
        if kind == 2 {
            if input == 0 || flags != 0 || !state.flip_scheduled {
                return E_INVALIDARG;
            }
            if input <= state.last_displayed || state.displayed.iter().any(|&(id, _)| id == input) {
                return S_OK;
            }
            if state.pending != Some(input) && !state.issued.contains(&input) {
                return E_INVALIDARG;
            }
            if timeout.is_some_and(|deadline| deadline <= Instant::now()) {
                return S_FALSE;
            }
            state = frames.wait_change(state, timeout);
            continue;
        }
        let Some(origin) = state.next_origin else {
            return E_INVALIDARG;
        };
        if !continued && ((input != 0 && (flags != 0 || state.pending != Some(input)))
            || (input == 0 && state.pending.is_some()))
        {
            return E_INVALIDARG;
        }
        let now = Instant::now();
        let has_room = state.issued.len() < state.period;
        let ready = now >= origin && has_room;
        if ready || timeout.is_some_and(|t| t <= now) {
            // The retail PROCESS_TOKEN_ON_TIMEOUT poll returns S_FALSE and a
            // token. A later unflagged wait on that token completes the origin.
            if ready || flags == 2 {
                let id = if input != 0 {
                    input
                } else {
                    state.last_token = state.last_token.wrapping_add(1).max(1);
                    state.last_token
                };
                if ready {
                    state.pending = None;
                    state.issued.push_back(id);
                    state.next_origin = Some(next_tick(origin, state.interval, now));
                } else {
                    state.pending = Some(id);
                }
                unsafe { token.write_unaligned(id) };
            }
            return if ready { S_OK } else { S_FALSE };
        }
        let wake = match (has_room, timeout) {
            (true, Some(deadline)) => Some(origin.min(deadline)),
            (true, None) => Some(origin),
            (false, deadline) => deadline,
        };
        state = frames.wait_change(state, wake);
    }
}

impl FrameEvents {
    fn wait_change<'a>(
        &self,
        state: std::sync::MutexGuard<'a, State>,
        wake: Option<Instant>,
    ) -> std::sync::MutexGuard<'a, State> {
        match wake {
            Some(deadline) => {
                self.changed
                    .wait_timeout(state, deadline.saturating_duration_since(Instant::now()))
                    .unwrap_or_else(|e| e.into_inner())
                    .0
            }
            None => self.changed.wait(state).unwrap_or_else(|e| e.into_inner()),
        }
    }
}

fn next_tick(origin: Instant, interval: Duration, now: Instant) -> Instant {
    if now < origin {
        return origin + interval;
    }
    let rem = now.duration_since(origin).as_nanos() % interval.as_nanos();
    now + interval - Duration::from_nanos(rem as u64)
}

// The queue holds a strong reference to its controlling guest device. Called
// only after a successful host presentation, with the token in the guest plane.
pub(crate) unsafe fn display_flip(identity: *mut c_void, token: u64) {
    let device = unsafe { &*identity.cast::<Device>() };
    let frames = &device.state.frames;
    let mut state = frames.state.lock().unwrap_or_else(|e| e.into_inner());
    if token != 0 && (state.pending == Some(token) || state.issued.contains(&token)) {
        // PresentX selects synchronized presentation. Sample the host present
        // for this token, independently of later queries. Earlier issued
        // origins that were never presented are superseded by this flip; a
        // title waiting on one of them (ProjectScarlett RVA 0x2482405, with an
        // infinite timeout) must not block forever.
        let mut qpc = 0;
        let _ = unsafe { QueryPerformanceCounter(&mut qpc) };
        let superseded: Vec<u64> = state.issued.iter().copied().take_while(|&v| v != token).collect();
        for id in superseded.into_iter().chain([token]) {
            if state.displayed.len() == 16 {
                state.displayed.pop_front();
            }
            state.displayed.push_back((id, qpc));
        }
        state.last_displayed = state.last_displayed.max(token);
    }
    if state.pending == Some(token) {
        state.pending = None;
        if let Some(origin) = state.next_origin {
            state.next_origin = Some(next_tick(origin, state.interval, Instant::now()));
        }
        state.issued.clear();
    } else if let Some(index) = state.issued.iter().position(|&v| v == token) {
        state.issued.drain(..=index);
    }
    for handle in &state.flip {
        let _ = unsafe { SetEvent(HANDLE(handle.as_raw_handle())) };
    }
    frames.changed.notify_all();
}

// Frosty's RHIWaitForFlip requests a single display statistic and reads the
// two QPC values at +0x10 and +0x18. For a synchronized single-plane present
// both report the same host completion. Display output is always plane zero.
#[repr(C)]
pub(super) struct DisplayStatistics {
    kind: u32,
    reserved: u32,
    plane_index: u8,
    padding: [u8; 7],
    sync_time: i64,
    flip_time: i64,
    percent_scanned: f32,
    reserved_tail: [u8; 20],
}

const _: () = assert!(size_of::<DisplayStatistics>() == 0x38);

pub(super) unsafe extern "system" fn statistics(
    device: &Device,
    token: u64,
    kind: u32,
    count: *mut u32,
    record: *mut DisplayStatistics,
) -> HRESULT {
    if count.is_null() {
        return E_INVALIDARG;
    }
    let capacity = unsafe { count.read_unaligned() };
    unsafe { count.write_unaligned(0) };
    if (capacity != 0 && record.is_null()) || token == 0 || kind & 0xffee_eefc != 0 {
        return E_INVALIDARG;
    }
    if capacity != 0 {
        // The guest supplies this many writable 56-byte statistic records.
        unsafe { record.write_bytes(0, capacity as usize) };
    }
    if kind == 0 {
        return S_OK;
    }
    // 0x100 requests hardware GPU rendering duration. Completion of a CPU
    // presentation does not measure it. S_FALSE means no statistics available.
    if kind == 0x100 {
        return S_FALSE;
    }
    if !matches!(kind, 0x10000 | 0x10100) {
        return E_NOTIMPL;
    }
    let state = device
        .state
        .frames
        .state
        .lock()
        .unwrap_or_else(|e| e.into_inner());
    let Some(&(_, time)) = state.displayed.iter().find(|(id, _)| *id == token) else {
        return S_FALSE;
    };
    // The driver counts available records even when the caller supplies no
    // array. Its output stride is 0x38 on both the counting and writing paths.
    if capacity != 0 {
        unsafe {
            record.write_unaligned(DisplayStatistics {
                kind: 0x10000,
                reserved: 0,
                plane_index: 0,
                padding: [0; 7],
                sync_time: time,
                flip_time: time,
                percent_scanned: 0.0,
                reserved_tail: [0; 20],
            })
        };
    }
    unsafe { count.write_unaligned(1) };
    S_OK
}
