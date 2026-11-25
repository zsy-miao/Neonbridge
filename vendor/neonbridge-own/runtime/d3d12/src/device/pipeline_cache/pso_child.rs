//! TUXBOX_PSO_CHILD (default on, =0 off): during the journal build
//! (`neonbridge --pipelines` / --prewarm), a pipeline carrying 1 MiB+ of
//! DXIL is compiled in a child `neonbridge --pso-child <frame>` process that
//! writes the pipeline's cached blob and exits; this process then creates it
//! from that blob (CachedPSO) and stores it in the library. prewarm182: the
//! driver's compile arenas are process-wide and never given back (fresh
//! threads kept 3748 and 1075 MiB; one 4076 KiB compile grew three 2016 MiB
//! regions), so the commit guard ended the build at 838 s. A child's arenas
//! go with the child. Each parent create is timed: over ~100 ms means the
//! driver did not take the blob and compiled again.
use std::cell::Cell;
use std::path::PathBuf;
use std::sync::atomic::{AtomicU64, Ordering};
use std::time::Instant;
use windows::Win32::Graphics::Direct3D12::*;
use windows::core::Result;

thread_local! {
    static FRAME: Cell<Option<u64>> = const { Cell::new(None) };
    /// FRAME came from the title's own journal (with_game_frame).
    static GAME: Cell<bool> = const { Cell::new(false) };
}

/// TUXBOX_PSO_CHILD_GAME (default on, =0 off): in the title too, a
/// background worker's 1 MiB+ pipeline is compiled in a `neonbridge
/// --pso-child` built from the frame the title's journal wrote for it, and
/// created here from the child's blob, so the title process never holds the
/// driver's compile arenas. test182: seven such compiles never started (an
/// 8-13 GiB estimate against 251-1901 MiB of free commit, the arenas of
/// earlier compiles kept by the title) and failed E_OUTOFMEMORY.
/// The launcher names the child exe (TUXBOX_PSO_CHILD_EXE) and DLL.
pub(crate) fn with_game_frame<R>(frame: Option<u64>, f: impl FnOnce() -> R) -> R {
    let Some(frame) = frame.filter(|_| crate::env_toggle!("TUXBOX_PSO_CHILD_GAME") != Some("0") && out().is_none()) else { return f() };
    let before = GAME.with(|g| g.replace(true));
    let r = with_frame(frame, f);
    GAME.with(|g| g.set(before));
    r
}

/// Runs `f` with the journal frame being built marked on this thread.
pub(crate) fn with_frame<R>(frame: u64, f: impl FnOnce() -> R) -> R {
    let before = FRAME.with(|c| c.replace(Some(frame)));
    let r = f();
    FRAME.with(|c| c.set(before));
    r
}

fn on() -> bool {
    crate::env_toggle!("TUXBOX_PSO_CHILD") != Some("0")
}

/// The blob path when this process is a --pso-child.
pub(super) fn out() -> Option<PathBuf> {
    std::env::var_os("TUXBOX_PSO_CHILD_OUT").map(PathBuf::from)
}

/// In the child: compile, then write the pipeline's cached blob to `out`.
pub(super) fn compile_and_write(out: &std::path::Path, shader_bytes: u64, make: impl FnOnce() -> Result<ID3D12PipelineState>) -> Result<ID3D12PipelineState> {
    let start = Instant::now();
    let made = make();
    let written = match &made {
        Ok(p) => unsafe { p.GetCachedBlob() }.ok().map(|blob| {
            let bytes = unsafe { core::slice::from_raw_parts(blob.GetBufferPointer().cast::<u8>(), blob.GetBufferSize()) };
            std::fs::write(out, bytes).map(|_| bytes.len())
        }),
        Err(_) => None,
    };
    crate::diagnostics::log_line(format_args!(
        "pso child: compiled {} KiB of DXIL in {} ms: {}", shader_bytes >> 10, start.elapsed().as_millis(),
        match (&made, &written) {
            (Err(e), _) => format!("refused {:#x}", e.code().0 as u32),
            (Ok(_), Some(Ok(n))) => format!("{} KiB cached blob written", n >> 10),
            (Ok(_), Some(Err(e))) => format!("blob not written ({e})"),
            (Ok(_), None) => "no cached blob".to_string(),
        }));
    made
}

/// What became of a child compile.
pub(super) enum Child {
    Made(ID3D12PipelineState),
    /// The child ran out of commit: leave the pipeline to a later prewarm or
    /// a draw-time compile (TUXBOX_PSO_CHILD_DEFER).
    Deferred,
    /// Off, no frame, or the child or cached create failed otherwise: compile here.
    Here,
}

/// TUXBOX_PSO_CHILD_DEFER (default on, =0 compiles here): prewarm183's child
/// for a 4076 KiB pipeline waited 117 s for commit, exited 0xc000012d
/// (STATUS_COMMITMENT_LIMIT) with no blob, and the parent compiled it in its
/// own process, the arena growth the child exists to keep out of it.
fn defer_on() -> bool {
    crate::env_toggle!("TUXBOX_PSO_CHILD_DEFER") != Some("0")
}

/// The child's exit says it ran out of memory: STATUS_COMMITMENT_LIMIT,
/// STATUS_NO_MEMORY, or our export's E_OUTOFMEMORY refusal.
fn out_of_commit(status: &std::io::Result<std::process::ExitStatus>) -> bool {
    matches!(status.as_ref().ok().and_then(|s| s.code()).map(|c| c as u32), Some(0xc000_012d | 0xc000_0017 | 0x8007_000e))
}

/// In the parent: compile the marked frame's pipeline in a child and create
/// it here from the child's blob.
pub(super) fn from_child(shader_bytes: u64, create: impl FnOnce(&[u8]) -> Result<ID3D12PipelineState>) -> Child {
    static N: AtomicU64 = AtomicU64::new(0);
    let Some(frame) = FRAME.with(Cell::get) else { return Child::Here };
    if !on() || out().is_some() {
        return Child::Here;
    }
    let game = GAME.with(Cell::get);
    // In the title the current exe is the title's: the launcher names its own.
    let exe = if game { std::env::var_os("TUXBOX_PSO_CHILD_EXE").map(PathBuf::from) } else { std::env::current_exe().ok() };
    let Some(exe) = exe else { return Child::Here };
    if game && !super::super::stream::background::journal::frame_on_disk(frame) {
        return Child::Here;
    }
    let path = std::env::temp_dir().join(format!("tuxbox-pso-{}-{frame}.blob", std::process::id()));
    let _ = std::fs::remove_file(&path);
    let _room = Room::wait(shader_bytes);
    let start = Instant::now();
    // CREATE_NO_WINDOW: the game has no console, so without it the child
    // opened a console window over the game (test190).
    use std::os::windows::process::CommandExt;
    let status = std::process::Command::new(&exe).arg("--pso-child").arg(frame.to_string())
        .env("TUXBOX_PSO_CHILD_OUT", &path).creation_flags(0x0800_0000).spawn()
        .and_then(|mut child| {
            use std::os::windows::io::AsRawHandle;
            // Before the wait reaps it: the handle stays valid until `child` drops.
            let status = child.wait();
            let peak = crate::diagnostics::memory::peak_commit_of(child.as_raw_handle() as isize);
            if peak != 0 {
                PER_BYTE.fetch_max(peak / shader_bytes.max(1), Ordering::Relaxed);
            }
            status
        });
    let child_ms = start.elapsed().as_millis();
    let blob = std::fs::read(&path).ok().filter(|b| !b.is_empty());
    let _ = std::fs::remove_file(&path);
    let Some(blob) = blob else {
        let defer = defer_on() && out_of_commit(&status);
        crate::diagnostics::log_line(format_args!(
            "pso child: {} KiB pipeline (frame {frame}): the child gave no blob after {child_ms} ms ({}); {} (TUXBOX_PSO_CHILD=0 off)",
            shader_bytes >> 10, match &status { Ok(s) => format!("exit {s}"), Err(e) => format!("not started: {e}") },
            if defer { "out of commit: deferred to a later prewarm or a draw-time compile (TUXBOX_PSO_CHILD_DEFER=0 compiles here)" } else { "compiled here" }));
        return if defer { Child::Deferred } else { Child::Here };
    };
    let start = Instant::now();
    let made = create(&blob);
    let ms = start.elapsed().as_millis();
    let n = N.fetch_add(1, Ordering::Relaxed) + 1;
    if n <= 16 || n.is_power_of_two() || ms > 100 || made.is_err() {
        crate::diagnostics::log_line(format_args!(
            "pso child #{n}: {} KiB pipeline compiled in a child in {child_ms} ms, {} KiB blob; created here from it in {ms} ms{}",
            shader_bytes >> 10, blob.len() >> 10,
            match &made {
                Err(e) => format!(", refused {:#x}: compiled here", e.code().0 as u32),
                Ok(_) if ms > 100 => " (over 100 ms: the driver compiled it again)".to_string(),
                Ok(_) => String::new(),
            }));
    }
    made.map_or(Child::Here, Child::Made)
}

/// The worst child peak commit per DXIL byte seen (each child's
/// PeakPagefileUsage, which includes its runtime's own ~0.5 GiB); 0 none yet.
static PER_BYTE: AtomicU64 = AtomicU64::new(0);
/// Commit the running children are expected to take, in bytes.
static RESERVED: (std::sync::Mutex<u64>, std::sync::Condvar) = (std::sync::Mutex::new(0), std::sync::Condvar::new());

/// A child launch's share of the commit left: it starts once its expected
/// peak (DXIL bytes x the worst measured B/byte, 2210 until a child is
/// measured: prewarm182's worst alone) fits in the system commit left less
/// 2 GiB less what running children are expected to take, or when no other
/// child runs. TUXBOX_PSO_CHILD_RESERVE_MB sets the 2 GiB.
struct Room(u64);

impl Room {
    fn wait(shader_bytes: u64) -> Room {
        let per_byte = match PER_BYTE.load(Ordering::Relaxed) { 0 => 2210, n => n };
        let need = shader_bytes.saturating_mul(per_byte);
        let keep = crate::env_toggle!("TUXBOX_PSO_CHILD_RESERVE_MB").and_then(|v| v.trim().parse::<u64>().ok()).unwrap_or(2048) << 20;
        let start = Instant::now();
        let mut logged = false;
        let mut reserved = RESERVED.0.lock().unwrap_or_else(|e| e.into_inner());
        loop {
            let left = super::free_commit().saturating_sub(keep).saturating_sub(*reserved);
            if *reserved == 0 || need <= left {
                break;
            }
            if !logged {
                logged = true;
                crate::diagnostics::log_line(format_args!(
                    "pso child: a {} KiB pipeline (about {} MiB at {per_byte} B/byte) waits for commit: {} MiB left beside {} MiB of running children",
                    shader_bytes >> 10, need >> 20, left >> 20, *reserved >> 20));
            }
            reserved = RESERVED.1.wait_timeout(reserved, std::time::Duration::from_secs(2)).unwrap_or_else(|e| e.into_inner()).0;
        }
        *reserved += need;
        if logged {
            crate::diagnostics::log_line(format_args!("pso child: started after {} ms of waiting for commit", start.elapsed().as_millis()));
        }
        Room(need)
    }
}

impl Drop for Room {
    fn drop(&mut self) {
        let mut reserved = RESERVED.0.lock().unwrap_or_else(|e| e.into_inner());
        *reserved = reserved.saturating_sub(self.0);
        RESERVED.1.notify_all();
    }
}

/// A copy of a pipeline stream with its CACHED_PSO subobject set to `blob`
/// (replaced in place, or appended). Returns the storage and its size.
pub(super) unsafe fn stream_with_cached(desc: &D3D12_PIPELINE_STATE_STREAM_DESC, blob: &[u8]) -> Option<(Vec<usize>, usize)> {
    let size = desc.SizeInBytes;
    if desc.pPipelineStateSubobjectStream.is_null() || size == 0 || !size.is_multiple_of(size_of::<usize>()) {
        return None;
    }
    let (offset, length) = super::super::stream::layout(19)?;
    let mut storage = vec![0usize; size / size_of::<usize>() + length.div_ceil(size_of::<usize>())];
    let out = storage.as_mut_ptr().cast::<u8>();
    unsafe { core::ptr::copy_nonoverlapping(desc.pPipelineStateSubobjectStream.cast::<u8>(), out, size) };
    let cached = D3D12_CACHED_PIPELINE_STATE { pCachedBlob: blob.as_ptr().cast(), CachedBlobSizeInBytes: blob.len() };
    let mut at = 0;
    while at < size {
        let tag = unsafe { out.add(at).cast::<i32>().read() };
        let (_, len) = super::super::stream::layout(tag)?;
        if tag == 19 {
            unsafe { out.add(at + offset).cast::<D3D12_CACHED_PIPELINE_STATE>().write_unaligned(cached) };
            return Some((storage, size));
        }
        at += len;
    }
    unsafe {
        out.add(size).cast::<i32>().write(19);
        out.add(size + offset).cast::<D3D12_CACHED_PIPELINE_STATE>().write_unaligned(cached);
    }
    Some((storage, size + length))
}
