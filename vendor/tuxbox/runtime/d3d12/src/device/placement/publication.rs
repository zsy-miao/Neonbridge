//! Publish finished GPU copies on a per-queue thread. A native queue wait
//! places that CPU publication before the title's next fence signal.
use super::writeback::Readback;
use std::sync::atomic::{AtomicI32, Ordering};
use std::sync::{Arc, OnceLock, mpsc};
use windows::Win32::Foundation::{E_FAIL, E_OUTOFMEMORY, HANDLE, S_OK};
use windows::Win32::Graphics::Direct3D12::*;
use windows::core::Result;

struct Job {
    readback: Readback,
    ready: ID3D12Fence,
    tick: u64,
}

/// Whether a failed publication means the queue cannot go on. Any error was
/// remembered for good, and a remembered error makes the queue drop every
/// later ExecuteCommandLists and fence signal unseen (commands.rs checks
/// `placement.error`): one write-back that ran out of memory over the
/// device-local budget, or met a guarded title page (E_ACCESSDENIED), stopped
/// the title's GPU work for the rest of the run and left it waiting on fences
/// (five-star a654736, GPL-3.0: a refused texture fill fails without throwing).
/// Only a lost device poisons the queue now; any other failure is logged and
/// that write-back is skipped, its title bytes stale until the next one.
/// TUXBOX_PUBLICATION_STRICT=1 remembers every failure as before.
fn queue_lost(error: &windows::core::Error) -> bool {
    use windows::Win32::Graphics::Dxgi::{DXGI_ERROR_DEVICE_HUNG, DXGI_ERROR_DEVICE_REMOVED, DXGI_ERROR_DEVICE_RESET, DXGI_ERROR_DRIVER_INTERNAL_ERROR};
    crate::env_toggle!("TUXBOX_PUBLICATION_STRICT") == Some("1")
        || [DXGI_ERROR_DEVICE_REMOVED, DXGI_ERROR_DEVICE_HUNG, DXGI_ERROR_DEVICE_RESET, DXGI_ERROR_DRIVER_INTERNAL_ERROR].contains(&error.code())
}

fn skipped(error: &windows::core::Error) {
    static N: std::sync::atomic::AtomicU64 = std::sync::atomic::AtomicU64::new(0);
    let n = N.fetch_add(1, Ordering::Relaxed) + 1;
    if n <= 8 || n.is_power_of_two() {
        crate::diagnostics::log_line(format_args!(
            "placed texture write-back failed {:#010x}: skipped, the queue goes on ({n} so far; TUXBOX_PUBLICATION_STRICT=1 stops the queue)",
            error.code().0));
    }
}

pub(super) struct Worker {
    pub(super) published: ID3D12Fence,
    sender: mpsc::Sender<Job>,
}

impl Worker {
    pub(super) fn enqueue(&self, readback: Readback, ready: &ID3D12Fence, tick: u64) -> Result<()> {
        let job = Job {
            readback,
            ready: ready.clone(),
            tick,
        };
        // Losing the worker is fatal to this queue. Keep the recording alive
        // until the driver retires the already-issued work.
        if let Err(error) = self.sender.send(job) {
            let job = error.0;
            if unsafe { job.ready.GetCompletedValue() } < tick {
                std::mem::forget(job);
            }
            return Err(E_FAIL.into());
        }
        Ok(())
    }
}

#[derive(Default)]
pub(crate) struct QueueAccess {
    pub(crate) error: Arc<AtomicI32>,
    reader: OnceLock<ID3D12Fence>,
    worker: OnceLock<Worker>,
}

impl QueueAccess {
    pub(super) fn reader(&self, device: &ID3D12Device) -> Result<&ID3D12Fence> {
        if let Some(fence) = self.reader.get() {
            return Ok(fence);
        }
        let created = unsafe { device.CreateFence(0, D3D12_FENCE_FLAG_NONE)? };
        Ok(self.reader.get_or_init(|| created))
    }

    pub(super) fn worker(&self, device: &ID3D12Device, failed: &Arc<AtomicI32>) -> Result<&Worker> {
        if let Some(worker) = self.worker.get() {
            return Ok(worker);
        }
        let published: ID3D12Fence = unsafe { device.CreateFence(0, D3D12_FENCE_FLAG_NONE)? };
        let gate = published.clone();
        let error = self.error.clone();
        let failed = failed.clone();
        let (sender, jobs) = mpsc::channel::<Job>();
        std::thread::Builder::new()
            .name("TuxBox texture readback".into())
            .spawn(move || {
                let remember = |result: Result<()>| {
                    if let Err(cause) = result {
                        for slot in [&error, &failed] {
                            let _ = slot.compare_exchange(
                                S_OK.0,
                                cause.code().0,
                                Ordering::AcqRel,
                                Ordering::Acquire,
                            );
                        }
                        crate::diagnostics::recording_error(format_args!(
                            "placed texture publication failed: {:?}",
                            cause.code()
                        ));
                    }
                };
                for job in jobs {
                    if unsafe { job.ready.GetCompletedValue() } < job.tick
                        && unsafe { job.ready.SetEventOnCompletion(job.tick, HANDLE::default()) }
                            .is_err()
                    {
                        // Event registration can run out of OS resources. The
                        // recording still belongs to the GPU until this fence.
                        while unsafe { job.ready.GetCompletedValue() } < job.tick {
                            std::thread::sleep(std::time::Duration::from_millis(2));
                        }
                    }
                    if unsafe { job.ready.GetCompletedValue() } == u64::MAX {
                        remember(Err(E_FAIL.into()));
                    } else {
                        let result = job.readback.complete();
                        match &result {
                            Ok(()) => {
                                static FIRST_PUBLICATION: std::sync::Once = std::sync::Once::new();
                                crate::diagnostics::log_once(&FIRST_PUBLICATION, || {
                                    "completed GPU writeback for a guest-backed placed texture".into()
                                });
                                remember(result);
                            }
                            Err(cause) if !queue_lost(cause) => skipped(cause),
                            Err(_) => remember(result),
                        }
                    }
                    // Release this queue's GPU gate even after failed mapping or
                    // publication, so guest fence waiters can observe its error.
                    remember(unsafe { gate.Signal(job.tick) });
                }
            })
            .map_err(|_| E_OUTOFMEMORY)?;
        Ok(self.worker.get_or_init(|| Worker { published, sender }))
    }
}
