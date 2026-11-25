//! Submitting command lists to a queue, and queue fences.
use super::*;

// Validate and translate the entire batch before submission. Small batches use
// stack storage and larger ones can report E_OUTOFMEMORY to the guest.
pub(super) fn translate_batch<T: Default, G, C>(
    guest: &[G],
    mut context: C,
    mut translate: impl FnMut(&G, &mut C) -> Result<T, HRESULT>,
    submit: impl FnOnce(&[T], C) -> Result<(), HRESULT>,
) -> Result<(), HRESULT> {
    let mut stack: [T; 8];
    let mut heap = Vec::new();
    let host = if guest.len() <= 8 {
        stack = std::array::from_fn(|_| T::default());
        &mut stack[..guest.len()]
    } else {
        heap.try_reserve_exact(guest.len())
            .map_err(|_| E_OUTOFMEMORY)?;
        heap.resize_with(guest.len(), T::default);
        heap.as_mut_slice()
    };
    for (dst, src) in host.iter_mut().zip(guest) {
        *dst = translate(src, &mut context)?;
    }
    submit(host, context)
}

pub(super) fn reject(queue: &Queue, error: HRESULT) {
    let _ = queue.state.placement.error.compare_exchange(
        S_OK.0,
        error.0,
        Ordering::AcqRel,
        Ordering::Acquire,
    );
    unsafe {
        OutputDebugStringA(s!(
            "Project TuxBox: direct queue rejected a guest command-list batch\n"
        ))
    };
}

pub(super) unsafe extern "system" fn execute_frame(
    queue: &Queue, count: u32, lists: *const *mut c_void, frame_token: u64, flags: u32,
) {
    // Both flags select how the console assembles its GPU packets. Bit 0
    // suppresses a hardware buffer switch and bit 1 chains indirect buffers.
    // The desktop queue owns its packet storage and takes the ordered list
    // array in one submission. The frame token labels that submission.
    // Direct and compute queues both submit this way. Each list's type is
    // checked against the queue with the rest of the batch.
    if flags & !3 != 0 {
        crate::diagnostics::recording_error(format_args!("extended command-list submission flags {flags:#x}"));
        reject(queue, E_NOTIMPL);
        return;
    }
    unsafe { execute_batch(queue, count, lists, Some((frame_token, flags))) };
}

pub(super) unsafe extern "system" fn execute(queue: &Queue, count: u32, lists: *const *mut c_void) {
    unsafe { execute_batch(queue, count, lists, None) };
}

pub(super) unsafe fn execute_batch(queue: &Queue, count: u32, lists: *const *mut c_void, frame: Option<(u64,u32)>) {
    let mut submitted = queue
        .state
        .submission
        .lock()
        .unwrap_or_else(|e| e.into_inner());
    *submitted = true;
    if queue.state.placement.error.load(Ordering::Acquire) < 0 || count == 0 {
        return;
    }
    if lists.is_null() {
        reject(queue, E_INVALIDARG);
        return;
    }
    let guest = unsafe { slice::from_raw_parts(lists, count as usize) };
    // Mirrors exist only when a constant-buffer or address-SRV import failed.
    // The count is zero for every boot that has not hit that path.
    crate::device::snapshot_cbv_mirrors();
    let mut list_index = 0;
    let trace_samples = std::cell::RefCell::new(Vec::new());
    // The lists in this batch that write or wait on fence labels.
    let batch = std::cell::RefCell::new(String::new());
    let result = translate_batch(
        guest,
        (
            Vec::new(),
            Vec::new(),
            Vec::new(),
            Vec::new(),
            memory_wait::Splits::new(),
            device::descriptor_paging::PendingSubmissions::new(),
        ),
        |&item, (textures, writes, shader_views, scene_lists, splits, pending)| {
            let list = unsafe { guest_list(item, parent(&queue.parent)) }.ok_or(E_INVALIDARG)?;
            let state = recording(list);
            if !matches!(state.phase, ListState::Closed)
                || state.error.is_err()
                || unsafe { list.native.GetType() } != queue.state.kind
            {
                return Err(E_INVALIDARG);
            }
            // A descriptor write after Close can exhaust the map's native
            // heap. Never submit an indexed access with an unpublished entry.
            for arena in &state.indexed_arenas {
                arena.check_index_capacity().map_err(|e| e.code())?;
            }
            for descriptor in &state.scalar_descriptors {
                let owner = list.parent.as_ref().ok_or(E_INVALIDARG)?;
                if !unsafe { device::is_constant_buffer(owner, descriptor.cpu) } {
                    crate::diagnostics::recording_error(format_args!(
                        "indexed shader scalar load requires a CBV at logical resource descriptor {}", descriptor.index
                    ));
                    return Err(E_INVALIDARG);
                }
            }
            pending
                .pin_from(&state.descriptor_leases)
                .map_err(|e| e.code())?;
            memory_wait::note_list(
                ptr::from_ref(list).addr(),
                memory_wait::Fate::Submitted(queue.state.kind.0),
                &state.labels,
            );
            if !state.labels.is_empty() {
                use std::fmt::Write;
                let _ = write!(batch.borrow_mut(), " {:#x}", ptr::from_ref(list).addr());
            }
            splits.try_reserve(state.prefix.segments.len()).map_err(|_| E_OUTOFMEMORY)?;
            splits.extend(state.prefix.segments.iter().map(|s| (list_index, s.clone())));
            list_index += 1;
            if !state.scene_commands.is_empty() {
                scene_lists.try_reserve(1).map_err(|_| E_OUTOFMEMORY)?;
                scene_lists.push(list);
            }
            textures
                .try_reserve(state.placed_reads.len())
                .map_err(|_| E_OUTOFMEMORY)?;
            textures.extend(state.placed_reads.iter().cloned());
            writes
                .try_reserve(state.placed_writes.len())
                .map_err(|_| E_OUTOFMEMORY)?;
            writes.extend(state.placed_writes.iter().cloned());
            shader_views.extend(state.sampled_heaps.iter().cloned());
            trace_samples.borrow_mut().extend(state.trace_samples.iter().cloned());
            Ok(list.native.as_raw())
        },
        |host, (mut textures, mut writes, shader_views, scene_lists, splits, mut pending)| {
            let holds = (!splits.is_empty())
                .then(|| placement::held::Waits::new(splits.len().into()));
            let waits = holds.clone();
            unsafe {
                placement::submit(
                    parent(&queue.parent),
                    &queue.native,
                    &queue.state.placement,
                    &mut textures,
                    &mut writes,
                    &shader_views,
                    holds,
                    || {
                        // Placement dependencies are settled. Publish scene writes and
                        // snapshot each instance table in recorded order across lists.
                        let submission = if scene_lists.is_empty() {
                            None
                        } else {
                            let owner = queue.parent.as_ref().ok_or(E_INVALIDARG)?;
                            Some(device::blas::begin_submission(owner))
                        };
                        for list in scene_lists {
                            let state = recording(list);
                            for command in &state.scene_commands {
                                command.prepare(list)?;
                            }
                        }
                        let retire = if !pending.is_empty() {
                            Some(queue.state.retire_fence(&queue.native)?)
                        } else {
                            None
                        };
                        memory_wait::submit(queue, host, splits, waits)?;
                        if let Some(rf) = retire {
                            let val = rf.next.fetch_add(1, Ordering::Relaxed).wrapping_add(1);
                            let val = if val == u64::MAX { 1 } else { val };
                            if let Err(error) = queue.native.Signal(&rf.fence, val) {
                                // Native work already reached the queue; retain pins rather than
                                // exposing live GPU reads to immediate reuse.
                                pending.disarm();
                                return Err(error);
                            }
                            pending.commit(
                                device::descriptor_paging::RetireFence::Native(rf.fence.clone()),
                                val,
                            );
                        }
                        queue_trace::note(
                            &queue.native,
                            queue.state.kind.0,
                            format!("batch of {count} lists, with labels:{}", batch.borrow()),
                            None,
                        );
                        if !trace_samples.borrow().is_empty() {
                            let mut device: Option<ID3D12Device> = None;
                            queue.native.GetDevice(&mut device)?;
                            let device = device.ok_or(E_INVALIDARG)?;
                            let fence: ID3D12Fence = device.CreateFence(0, D3D12_FENCE_FLAG_NONE)?;
                            queue.native.Signal(&fence, 1)?;
                            fence.SetEventOnCompletion(1, windows::Win32::Foundation::HANDLE::default())?;
                            for (label, sample) in trace_samples.borrow().iter() {
                                sample.report(label);
                            }
                        }
                        if let Some(submission) = submission {
                            submission.commit();
                        }
                        crate::diagnostics::submitted(queue.state.kind, count);
                        Ok(())
                    },
                )
            }
            .map_err(|error| error.code())
        },
    );
    if let Err(error) = result {
        reject(queue, error);
    } else if let Some((token, flags)) = frame {
        static FIRST: [std::sync::Once;4] = [const { std::sync::Once::new() };4];
        crate::diagnostics::log_once(&FIRST[flags as usize], || format!(
            "submitted frame batch flags={flags:#x} token={token} count={count} first_lists={:?}",
            &guest[..guest.len().min(4)]
        ));
    }
}

pub(super) unsafe extern "system" fn queue_fence<const WAIT: bool>(
    queue: &Queue,
    fence: *mut c_void,
    value: u64,
) -> HRESULT {
    let mut submitted = queue
        .state
        .submission
        .lock()
        .unwrap_or_else(|e| e.into_inner());
    *submitted = true;
    let hr = HRESULT(queue.state.placement.error.load(Ordering::Acquire));
    if hr.is_err() {
        return hr;
    }
    let owner = parent(&queue.parent);
    let native;
    let fence = match unsafe { device::fence(fence, owner) } {
        Some(fence) => fence,
        None => {
            native = match unsafe { device::media::native_child::<ID3D12Fence>(fence, owner) } {
                Ok(fence) => fence,
                Err(error) => return error.code(),
            };
            &native
        }
    };
    let method = if WAIT {
        queue.native.vtable().Wait
    } else {
        queue.native.vtable().Signal
    };
    let status = unsafe { method(queue.native.as_raw(), fence.as_raw(), value) };
    if status.is_ok() {
        queue_trace::note(
            &queue.native,
            queue.state.kind.0,
            if WAIT { "title wait".into() } else { "title signal".into() },
            Some((fence.clone(), value)),
        );
    }
    if !WAIT && status.is_ok() {
        static FIRST: std::sync::Once = std::sync::Once::new();
        crate::diagnostics::log_once(&FIRST, || format!(
            "queued guest fence signal value={value} queue_type={} fence={:p}", queue.state.kind.0, fence.as_raw()
        ));
    }
    status
}

// Frosty's direct/compute path requests its flush before the fence and the
// copy path passes zero. Native queue Signal orders completion of prior work.
pub(super) unsafe extern "system" fn signal_x(
    queue: &Queue,
    fence: *mut c_void,
    value: u64,
    flush: u32,
) -> HRESULT {
    if !matches!(flush, 0 | 0x0800_0000) {
        return E_NOTIMPL;
    }
    unsafe { queue_fence::<false>(queue, fence, value) }
}
