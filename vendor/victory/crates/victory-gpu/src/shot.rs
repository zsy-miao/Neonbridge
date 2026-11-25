//! Diagnostics: one frame taken apart, by the layer itself ("frame shot").
//!
//! A frame debugger from outside (RenderDoc) wraps the host device and
//! hooks the process: run 263 ended ten seconds in, inside the hook of a
//! streaming program's game capture on that wrapped device, and the driver
//! refused the pipeline library made for the real one. Nothing of that
//! kind happens here: no library is loaded, the device is the one every run
//! uses, and the only commands added are the two kinds the layer records in
//! every run already (a copy of a texture into a read-back buffer, as the
//! saved pictures are made, and a timestamp, as the card meter's).
//!
//! `VICTORY_SHOT=<folder>` switches it on. A shot is taken of the frame
//! after a present at which
//!  * the file `take` is found in that folder (it is removed; the script
//!    `shot.ps1` makes it), or
//!  * the frame is one of `VICTORY_SHOT_AT` (comma-separated, counted from
//!    the first frame of 500 draws or more, as `VICTORY_RENDERDOC_AT`).
//!
//! Of that frame the shot keeps, in `<folder>\shot-<frame>`:
//!  * `events.txt`: every draw, dispatch, copy, submission and drop in the
//!    order recorded, with the list it was recorded into, the thread, and
//!    the place of its timestamp (the lines of `capture::op`: shaders,
//!    targets, states, what is read);
//!  * `stamps.txt`: the card's clock before each of those commands, so the
//!    time to the next one in the same list is that command's;
//!  * `snap-*.png`: what a texture held where it left a state shaders
//!    write in (render target, depth write, unordered access): the picture
//!    a pass left. The title gives its textures' states in a form without
//!    states (enhanced barriers), so the layer moves every texture itself,
//!    and that is where the copies are made: inside a list
//!    (`HostList::transition`) and between lists, in the list that runs
//!    before each one (`HostList::prelude`). `snaps.txt` lists them;
//!  * `info.txt`, and `done` when everything is written.
//!
//! **Pictures come in parts.** With the title running the PC has one to
//! four gigabytes free (run 269: 3.6 GB in the opening cutscene, 0.9 GB in
//! the first room), and a frame's targets are more than that. A place
//! where a picture can be taken is named by its texture and how many
//! times the texture left a written state before in the frame: the same
//! name in every frame that is built the same way. The shot's own frame
//! takes as many as fit in half of the memory that is free above 0.7 GB
//! (`VICTORY_SHOT_MB` at most, 1024 unless said); when they are written and
//! their buffers gone, a later frame takes the ones still missing, and so
//! on until none is, or a frame gets none. The lines and times are all of
//! the first frame, and are taken even when there is no room for any
//! picture; a picture says which frame it is of.
//!
//! The pictures are read two seconds and eight frames after their frame
//! (every list of it has long run) and written by a thread of their own on
//! the slow cores. Multisampled textures and volumes are not copied; of an
//! array the first six slices' first level; a texture at most eight times
//! a frame; at most `VICTORY_SHOT_MAX` (12) shots a run.

use std::path::PathBuf;
use std::sync::atomic::{AtomicBool, AtomicU8, AtomicU32, AtomicU64, Ordering::Relaxed};
use std::sync::{Mutex, OnceLock};

use windows::Win32::Graphics::Direct3D12::*;
use windows::Win32::Graphics::Dxgi::Common::DXGI_SAMPLE_DESC;

const IDLE: u8 = 0;
const RECORDING: u8 = 1;
const PENDING: u8 = 2;
const SAVING: u8 = 3;
static PHASE: AtomicU8 = AtomicU8::new(IDLE);
/// Whether the frame being recorded keeps its lines and times (a shot's
/// first frame; the later ones only take pictures).
static LINES: AtomicBool = AtomicBool::new(false);

/// Which frame of which shot is being taken (1, 2, ...): what a list holds
/// for a shot belongs to one.
static SHOT_ID: AtomicU32 = AtomicU32::new(0);

/// Timestamps of a shot, and how many a list takes at a time.
const SLOTS: u32 = 131_072;
const BLOCK: u32 = 128;
/// The next free timestamp of the shot being taken.
static NEXT: AtomicU32 = AtomicU32::new(0);

/// Lines a shot holds at most.
const MAX_EVENTS: usize = 400_000;
/// Copies of one texture in a frame.
const REPEATS: u32 = 8;
/// Frames a shot takes its pictures in, at most.
const MAX_PARTS: u32 = 48;
/// When this many lists recorded for a shot's frame have been submitted,
/// the frame is the title's to finish: lists begun from then on are the
/// next frame's (the title records it while the layer still runs this
/// one: run 269's first shot held frame 0x781 and, behind its submissions,
/// all of 0x782), and the present that follows ends the shot's frame. A
/// present before that is of a frame recorded before the shot began, and
/// the shot goes on through it, three presents at most.
const FRAME_LISTS: u32 = 20;
/// Lists of the frame being recorded that the title has submitted.
static SUBMITTED: AtomicU32 = AtomicU32::new(0);

/// Whether the title has begun to submit the frame being recorded.
fn closing() -> bool {
    SUBMITTED.load(Relaxed) >= FRAME_LISTS
}

/// The title submits `list`, while a shot records.
pub(crate) fn submitted(list: &crate::host::HostList) {
    let mark = *list.shot_mark.lock().unwrap_or_else(|e| e.into_inner());
    if mark.0 != 0 && mark.0 == SHOT_ID.load(Relaxed) {
        SUBMITTED.fetch_add(1, Relaxed);
    }
}
/// Memory left alone (MB), and the least a frame's pictures need.
const RESERVE_MB: u64 = 700;
const LEAST_MB: u64 = 48;
/// The host state copies are read in.
const COPY_SOURCE: u32 = 0x800;

/// The folder shots go to, if the tool is on.
fn folder() -> Option<&'static PathBuf> {
    static FOLDER: OnceLock<Option<PathBuf>> = OnceLock::new();
    FOLDER
        .get_or_init(|| {
            let folder = PathBuf::from(std::env::var_os("VICTORY_SHOT")?);
            if !folder.is_absolute() {
                tracing::warn!(folder = %folder.display(), "VICTORY_SHOT is not a full path: no frame shots");
                return None;
            }
            if let Err(e) = std::fs::create_dir_all(&folder) {
                tracing::warn!(error = %e, folder = %folder.display(), "the folder for frame shots could not be made: none are taken");
                return None;
            }
            tracing::info!(
                folder = %folder.display(),
                at = ?at(),
                "frame shots are on (VICTORY_SHOT): one is taken when the file `take` appears in the folder, and at the frames of VICTORY_SHOT_AT"
            );
            Some(folder)
        })
        .as_ref()
}

fn number(name: &str, default: u64) -> u64 {
    std::env::var(name).ok().and_then(|v| v.trim().parse().ok()).unwrap_or(default)
}

/// `VICTORY_SHOT_AT`: frames counted from the first one of 500 draws.
fn at() -> &'static [u64] {
    static AT: OnceLock<Vec<u64>> = OnceLock::new();
    AT.get_or_init(|| {
        std::env::var("VICTORY_SHOT_AT").unwrap_or_default().split(',').filter_map(|f| f.trim().parse().ok()).collect()
    })
}

/// The read-back memory (MB) a frame of a shot may take now.
fn room_mb() -> u64 {
    (crate::framelog::free_mb().saturating_sub(RESERVE_MB) / 2).min(number("VICTORY_SHOT_MB", 1024))
}

/// A line of the shot.
struct Event {
    list: usize,
    thread: u32,
    /// The place of the timestamp recorded before the command, or none.
    stamp: u32,
    text: String,
}

/// A picture of the shot, still in its read-back buffer.
struct Snap {
    /// How many times its texture left a written state before in the
    /// frame.
    occurrence: u32,
    name: String,
    buffer: ID3D12Resource,
    footprint: D3D12_SUBRESOURCE_FOOTPRINT,
    bytes: u64,
    /// The title's texture (or, unknown, the host copy), its own size and
    /// format, the slice copied, and the states it left and went to.
    texture: usize,
    width: u64,
    height: u32,
    format: i32,
    slice: u32,
    before: u32,
    after: u32,
}

/// What a shot carries from one of its frames to the next.
#[derive(Clone, Default)]
struct Carry {
    /// The shot's first frame (its folder's name) and why it was taken.
    first: u64,
    why: String,
    /// Frames taken so far; pictures saved and failed in them.
    parts: u32,
    saved: u32,
    failed: u32,
    /// The places done: host copy, occurrence, slice.
    done: std::collections::HashSet<(usize, u32, u32)>,
    /// Of the first frame: lines, draws and dispatches, presents, places.
    lines: usize,
    work: u32,
    presents: u32,
    places: u32,
    not_taken: u32,
    read_back_mb: u64,
}

struct Shot {
    id: u32,
    /// The frame this part is of.
    frame: u64,
    carry: Carry,
    presents: u32,
    /// Draws and dispatches among its lines.
    work: u32,
    events: Vec<Event>,
    snaps: Vec<Snap>,
    /// Read-back bytes so far, and the most allowed.
    bytes: u64,
    budget: u64,
    /// Places met so far in this frame; how many are left for a later
    /// frame; the ones never taken (repeats, too large).
    places: u32,
    later: u32,
    not_taken: u32,
    /// Times a texture (host copy) left a written state so far.
    per_texture: std::collections::HashMap<usize, u32>,
    /// The title's texture by host copy.
    titles: std::collections::HashMap<usize, usize>,
    /// The ranges of timestamps lists resolved: first place and count.
    resolved: Vec<(u32, u32)>,
    /// When recording ended, and the frame's number.
    stopped: Option<(std::time::Instant, u64)>,
}

// SAFETY: host objects (free-threaded), handed from the recording threads
// to the one that writes the shot out, behind a lock.
unsafe impl Send for Shot {}

static SHOT: Mutex<Option<Shot>> = Mutex::new(None);
/// A shot between two of its frames.
static WAITING: Mutex<Option<Carry>> = Mutex::new(None);

/// The timestamps' heap and where they are read back.
struct Clock {
    heap: ID3D12QueryHeap,
    back: ID3D12Resource,
    /// The read-back buffer's bytes, mapped for good.
    mapped: usize,
    frequency: u64,
}

// SAFETY: host objects (free-threaded) and a mapping that is only read.
unsafe impl Send for Clock {}
// SAFETY: as above.
unsafe impl Sync for Clock {}

static CLOCK: OnceLock<Option<Clock>> = OnceLock::new();

fn make_clock() -> Option<Clock> {
    if std::env::var_os("VICTORY_SHOT_TIMES").is_some_and(|v| v == "0") {
        return None;
    }
    let host = crate::host::host()?;
    let device = host.device();
    let mut heap: Option<ID3D12QueryHeap> = None;
    let mut back: Option<ID3D12Resource> = None;
    let readback = D3D12_HEAP_PROPERTIES { Type: D3D12_HEAP_TYPE_READBACK, ..Default::default() };
    let desc = D3D12_RESOURCE_DESC {
        Dimension: D3D12_RESOURCE_DIMENSION_BUFFER,
        Width: u64::from(SLOTS) * 8,
        Height: 1,
        DepthOrArraySize: 1,
        MipLevels: 1,
        SampleDesc: DXGI_SAMPLE_DESC { Count: 1, Quality: 0 },
        Layout: D3D12_TEXTURE_LAYOUT_ROW_MAJOR,
        ..Default::default()
    };
    // SAFETY: plain object creation; the buffer is mapped once and stays so
    // (the card meter's heap and buffer, made the same way).
    unsafe {
        device
            .CreateQueryHeap(&D3D12_QUERY_HEAP_DESC { Type: D3D12_QUERY_HEAP_TYPE_TIMESTAMP, Count: SLOTS, NodeMask: 0 }, &mut heap)
            .ok()?;
        device
            .CreateCommittedResource(&readback, D3D12_HEAP_FLAG_NONE, &desc, D3D12_RESOURCE_STATE_COPY_DEST, None, &mut back)
            .ok()?;
        let (heap, back) = (heap?, back?);
        let mut mapped = std::ptr::null_mut();
        back.Map(0, None, Some(&mut mapped)).ok()?;
        let frequency = host.timestamp_frequency();
        if mapped.is_null() || frequency == 0 {
            return None;
        }
        Some(Clock { heap, back, mapped: mapped as usize, frequency })
    }
}

/// Whether a frame is being recorded for a shot. One load: asked at every
/// state change of a texture in every diagnostics run.
pub(crate) fn recording() -> bool {
    PHASE.load(Relaxed) == RECORDING
}

/// Whether the frame being recorded keeps its lines ([`line`]). Asked at
/// every noted command of every diagnostics run.
pub(crate) fn keeping_lines() -> bool {
    recording() && LINES.load(Relaxed)
}

thread_local! {
    /// The timestamp [`stamp`] recorded for the line this thread is about
    /// to give to [`line`].
    static STAMP: std::cell::Cell<u32> = const { std::cell::Cell::new(u32::MAX) };
    /// The line this thread is about to give to [`line`] is of a list of
    /// the next frame ([`stamp`]): not kept.
    static NEXT_FRAME: std::cell::Cell<bool> = const { std::cell::Cell::new(false) };
}

/// A fresh block of timestamps, if any are left.
fn take_block() -> Option<u32> {
    let base = NEXT.fetch_add(BLOCK, Relaxed);
    (base <= SLOTS - BLOCK).then_some(base)
}

/// Resolves `count` timestamps from `base` in `list` and notes the range.
fn resolve(list: &ID3D12GraphicsCommandList, clock: &Clock, id: u32, base: u32, count: u32) {
    // SAFETY: an open list, our heap and our read-back buffer; the places
    // are the block's own, and each was ended in this list before.
    unsafe { list.ResolveQueryData(&clock.heap, D3D12_QUERY_TYPE_TIMESTAMP, base, count, &clock.back, u64::from(base) * 8) };
    let mut shot = SHOT.lock().unwrap_or_else(|e| e.into_inner());
    if let Some(s) = shot.as_mut().filter(|s| s.id == id) {
        s.resolved.push((base, count));
    }
}

/// Before a command of the title is recorded into `list`, while a shot
/// records: the list is marked as one of the shot's frame, and in a frame
/// that keeps its lines the card's clock goes into the list here, for the
/// line that follows ([`line`]).
pub(crate) fn stamp(list: &crate::host::HostList) {
    if !recording() {
        return;
    }
    let id = SHOT_ID.load(Relaxed);
    {
        let mut mark = list.shot_mark.lock().unwrap_or_else(|e| e.into_inner());
        if mark.0 != id {
            // A list begun after the frame's lists were submitted is the
            // next frame's.
            if closing() {
                NEXT_FRAME.set(true);
                return;
            }
            let lines = SHOT.lock().unwrap_or_else(|e| e.into_inner()).as_ref().map_or(0, |s| s.events.len());
            *mark = (id, lines as u32);
        }
    }
    if !LINES.load(Relaxed) {
        return;
    }
    let Some(Some(clock)) = CLOCK.get() else {
        return;
    };
    let mut block = list.shot_block.lock().unwrap_or_else(|e| e.into_inner());
    let (base, used, of) = *block;
    let end = |slot: u32| {
        // SAFETY: an open list and a place of our heap.
        unsafe { list.list.EndQuery(&clock.heap, D3D12_QUERY_TYPE_TIMESTAMP, slot) };
    };
    let slot = if used == 0 || of != id {
        let Some(base) = take_block() else { return };
        *block = (base, 1, id);
        base
    } else if used == BLOCK - 1 {
        // The block's last place ends it; the command opens the next.
        end(base + used);
        resolve(&list.list, clock, id, base, BLOCK);
        let Some(next) = take_block() else {
            *block = (0, 0, 0);
            return;
        };
        *block = (next, 1, id);
        next
    } else {
        *block = (base, used + 1, id);
        base + used
    };
    end(slot);
    STAMP.set(slot);
}

/// `list` is about to be closed: the block of timestamps it holds is ended
/// and resolved.
pub(crate) fn close(list: &crate::host::HostList) {
    let mut block = list.shot_block.lock().unwrap_or_else(|e| e.into_inner());
    let (base, used, of) = std::mem::take(&mut *block);
    if used == 0 || of != SHOT_ID.load(Relaxed) {
        return;
    }
    let Some(Some(clock)) = CLOCK.get() else {
        return;
    };
    // SAFETY: an open list and a place of our heap (a block always keeps
    // its last place for this).
    unsafe { list.list.EndQuery(&clock.heap, D3D12_QUERY_TYPE_TIMESTAMP, base + used) };
    resolve(&list.list, clock, of, base, used + 1);
}

/// A list of the layer's own (the one that runs before a title list, the
/// one that sends changed bytes to the card and hashes them, the one that
/// compares) begins in `cmd`, while a shot's first frame records: the
/// card's clock, for [`span_end`]. The title's commands are a part of the
/// card's frame; these are the rest of it.
pub(crate) fn span_begin(cmd: &ID3D12GraphicsCommandList) -> u32 {
    if !keeping_lines() {
        return u32::MAX;
    }
    let Some(Some(clock)) = CLOCK.get() else {
        return u32::MAX;
    };
    let slot = NEXT.fetch_add(2, Relaxed);
    if slot > SLOTS - 2 {
        return u32::MAX;
    }
    // SAFETY: an open list and a place of our heap.
    unsafe { cmd.EndQuery(&clock.heap, D3D12_QUERY_TYPE_TIMESTAMP, slot) };
    slot
}

/// The list [`span_begin`] timed ends here; `what` is its line, for the
/// title list `list` or none.
pub(crate) fn span_end(cmd: &ID3D12GraphicsCommandList, slot: u32, list: usize, what: impl FnOnce() -> String) {
    if slot == u32::MAX {
        return;
    }
    let Some(Some(clock)) = CLOCK.get() else {
        return;
    };
    // SAFETY: an open list, our heap and read-back buffer, the two places
    // `span_begin` took.
    unsafe {
        cmd.EndQuery(&clock.heap, D3D12_QUERY_TYPE_TIMESTAMP, slot + 1);
        cmd.ResolveQueryData(&clock.heap, D3D12_QUERY_TYPE_TIMESTAMP, slot, 2, &clock.back, u64::from(slot) * 8);
    }
    let mut shot = SHOT.lock().unwrap_or_else(|e| e.into_inner());
    let Some(s) = shot.as_mut() else { return };
    if s.carry.parts != 0 || s.events.len() >= MAX_EVENTS {
        return;
    }
    s.resolved.push((slot, 2));
    // SAFETY: a plain query.
    let thread = unsafe { windows_sys::Win32::System::Threading::GetCurrentThreadId() };
    s.events.push(Event { list, thread, stamp: slot, text: what() });
}

/// A line of `capture::op`, while a shot's first frame records.
pub(crate) fn line(list: usize, text: &str) {
    let stamp = STAMP.replace(u32::MAX);
    if NEXT_FRAME.replace(false) {
        return;
    }
    let mut shot = SHOT.lock().unwrap_or_else(|e| e.into_inner());
    let Some(s) = shot.as_mut() else { return };
    if s.stopped.is_some() || s.carry.parts != 0 || s.events.len() >= MAX_EVENTS {
        return;
    }
    if ["draw ", "indexed ", "mesh ", "indirect ", "dispatch "].iter().any(|k| text.starts_with(k)) {
        s.work += 1;
    }
    // SAFETY: a plain query.
    let thread = unsafe { windows_sys::Win32::System::Threading::GetCurrentThreadId() };
    s.events.push(Event { list, thread, stamp, text: text.to_owned() });
}

/// Whether pictures of host format `format` can be shown
/// (`capture::to_rgba`'s formats, by the texture's own format).
fn shows(format: i32) -> bool {
    matches!(format, 1..=2 | 9..=16 | 19..=65 | 87..=93)
}

/// `list` says which title texture a host copy is of, while a shot
/// records (pictures are named after the title's texture, as the lines
/// name targets).
pub(crate) fn known(copy: usize, title: usize) {
    let mut shot = SHOT.lock().unwrap_or_else(|e| e.into_inner());
    if let Some(s) = shot.as_mut() {
        s.titles.insert(copy, title);
    }
}

/// Whether `list` was recorded in the frame of the shot being taken, and
/// if so the line its recording began at: for the list that runs before it
/// at submission ([`leaves`] there), which may be made after the frame's
/// present.
pub(crate) fn of_the_frame(list: &crate::host::HostList) -> Option<u32> {
    if !matches!(PHASE.load(Relaxed), RECORDING | PENDING) {
        return None;
    }
    let mark = *list.shot_mark.lock().unwrap_or_else(|e| e.into_inner());
    (mark.0 != 0 && mark.0 == SHOT_ID.load(Relaxed)).then_some(mark.1)
}

/// Whether a move from host state `before` to `after` takes a texture out
/// of a state shaders write in (render target, unordered access, depth
/// write): a place for a picture.
pub(crate) fn might_leave(before: u32, after: u32) -> bool {
    const WRITES: u32 = 0x4 | 0x8 | 0x10;
    before & WRITES != 0 && before != after && before != COPY_SOURCE
}

/// Host texture `copy` is about to be moved from state `before` to `after`
/// by commands recorded into `cmd`: either the title's list `recording`
/// itself, or (`begins`: the line that list's recording began at) the list
/// that runs before it at submission. If the texture leaves a state
/// shaders write in, this is a place for a picture: where one is taken,
/// the texture is moved to the copy-source state, copied, and moved back
/// to `before` here, so the caller's own move is recorded as in any
/// other frame (a caller may read "no change of state" as "not used
/// since": the alias copies of `crate::texture` do).
pub(crate) fn leaves(
    cmd: &ID3D12GraphicsCommandList,
    list: usize,
    recording: Option<&crate::host::HostList>,
    begins: Option<u32>,
    copy: &std::sync::Arc<crate::resource::HostTexture>,
    before: u32,
    after: u32,
) {
    if !might_leave(before, after) {
        return;
    }
    let texture = &copy.texture;
    // SAFETY: a live resource.
    let d = unsafe { texture.GetDesc() };
    if d.Dimension != D3D12_RESOURCE_DIMENSION_TEXTURE2D || d.SampleDesc.Count > 1 || !shows(d.Format.0) {
        return;
    }
    let Some(host) = crate::host::host() else { return };
    let key = std::sync::Arc::as_ptr(copy) as usize;
    let slices = if d.DepthOrArraySize <= 6 { u32::from(d.DepthOrArraySize) } else { 1 };
    // In the title's own list the copy gets a timestamp of its own, so
    // its time is not the command's before it. (A list of the next frame
    // has no places.)
    if let Some(recording) = recording {
        stamp(recording);
        if NEXT_FRAME.replace(false) {
            return;
        }
    }
    let stamp = STAMP.replace(u32::MAX);
    // SAFETY: a plain query.
    let thread = unsafe { windows_sys::Win32::System::Threading::GetCurrentThreadId() };
    // Which place this is: the texture's n-th in the frame.
    let (occurrence, title) = {
        let mut shot = SHOT.lock().unwrap_or_else(|e| e.into_inner());
        let Some(s) = shot.as_mut() else { return };
        let met = s.per_texture.entry(key).or_default();
        *met += 1;
        (*met - 1, s.titles.get(&key).copied().unwrap_or(key))
    };
    let most = number("VICTORY_SHOT_MB", 1024) << 20;
    let mut moved = false;
    for slice in 0..slices {
        let sub = slice * u32::from(d.MipLevels);
        let mut total = 0u64;
        // SAFETY: a live device; outputs for one subresource.
        unsafe { host.device().GetCopyableFootprints(&d, sub, 1, 0, None, None, None, Some(&mut total)) };
        // Whether the place's picture is taken in this frame; the line of
        // a frame that keeps lines says which.
        {
            let mut shot = SHOT.lock().unwrap_or_else(|e| e.into_inner());
            let Some(s) = shot.as_mut() else { break };
            let first = s.carry.parts == 0;
            if first {
                s.places += 1;
            }
            let what = format!(
                "texture {title:#x} n {occurrence} slice {slice} {}x{} f{} leaves state {before:#x} for {after:#x}{}",
                d.Width,
                d.Height,
                d.Format.0,
                begins.map_or(String::new(), |b| format!(" at the start of the recording that begins at line {b}"))
            );
            let say = |s: &mut Shot, word: &str, why: &str| {
                if first && s.events.len() < MAX_EVENTS {
                    s.events.push(Event { list, thread, stamp: if slice == 0 { stamp } else { u32::MAX }, text: format!("{word} {what}{why}") });
                }
            };
            // The same in every frame: a texture's ninth place on, and a
            // picture larger than a frame may ever take.
            if occurrence >= REPEATS {
                if first {
                    s.not_taken += 1;
                }
                say(s, "SNAP-NOT", " (the texture's ninth place or later)");
                continue;
            }
            if total == 0 || total > most {
                if first {
                    s.not_taken += 1;
                }
                say(s, "SNAP-NOT", " (too large)");
                continue;
            }
            if s.carry.done.contains(&(key, occurrence, slice)) {
                // Taken by an earlier frame of the shot.
                continue;
            }
            if s.bytes + total > s.budget {
                s.later += 1;
                say(s, "SNAP-LATER", "");
                continue;
            }
            s.bytes += total;
            s.carry.done.insert((key, occurrence, slice));
            say(s, "SNAP", "");
        }
        if !moved {
            // SAFETY: an open list and a live texture in state `before`.
            unsafe {
                cmd.ResourceBarrier(&[crate::host::transition(
                    texture,
                    D3D12_RESOURCE_STATES(before as i32),
                    D3D12_RESOURCE_STATES(COPY_SOURCE as i32),
                )]);
            }
            moved = true;
        }
        let Some((buffer, footprint)) = crate::capture::read_back_of(host.device(), cmd, texture, sub) else {
            continue;
        };
        let name = format!("snap-{title:x}-n{occurrence}-s{slice}-{}x{}-f{}", d.Width, d.Height, d.Format.0);
        let mut shot = SHOT.lock().unwrap_or_else(|e| e.into_inner());
        let Some(s) = shot.as_mut() else { break };
        s.snaps.push(Snap {
            occurrence,
            name,
            buffer,
            footprint: footprint.Footprint,
            bytes: total,
            texture: title,
            width: d.Width,
            height: d.Height,
            format: d.Format.0,
            slice,
            before,
            after,
        });
    }
    if moved {
        // SAFETY: an open list and a live texture in the copy-source state.
        unsafe {
            cmd.ResourceBarrier(&[crate::host::transition(
                texture,
                D3D12_RESOURCE_STATES(COPY_SOURCE as i32),
                D3D12_RESOURCE_STATES(before as i32),
            )]);
        }
    }
}

/// The first frame of 500 draws or more, for `VICTORY_SHOT_AT`.
static WORLD: AtomicU64 = AtomicU64::new(u64::MAX);

/// Why a shot is taken of the frame after `frame`, if one is.
fn trigger(folder: &std::path::Path, frame: u64) -> Option<String> {
    let world = WORLD.load(Relaxed);
    if world != u64::MAX && at().contains(&(frame + 1 - world)) {
        return Some(format!("frame {} after the first one of 500 draws (VICTORY_SHOT_AT)", frame + 1 - world));
    }
    // The file is looked for at every fourth present.
    if frame.is_multiple_of(4) {
        let take = folder.join("take");
        if take.exists() {
            // A file that cannot be removed would ask again at once.
            return match std::fs::remove_file(&take) {
                Ok(()) => Some("asked for (the file `take`)".to_string()),
                Err(e) => {
                    static ONCE: std::sync::Once = std::sync::Once::new();
                    ONCE.call_once(|| tracing::warn!(error = %e, "the file `take` could not be removed: no frame shot for it"));
                    None
                }
            };
        }
    }
    None
}

/// At the present of `frame`, on the thread that presents; `drawn` is the
/// number of draws run on the host so far.
pub(crate) fn present(frame: u64, drawn: u64) {
    let Some(folder) = folder() else { return };
    static LAST: AtomicU64 = AtomicU64::new(0);
    if drawn.saturating_sub(LAST.swap(drawn, Relaxed)) >= 500 {
        let _ = WORLD.compare_exchange(u64::MAX, frame, Relaxed, Relaxed);
    }
    match PHASE.load(Relaxed) {
        IDLE => {
            let waiting = WAITING.lock().unwrap_or_else(|e| e.into_inner()).clone();
            if let Some(carry) = waiting {
                // The next frame of a shot: one built like its first (the
                // title's frames come in pairs: an even number of frames on).
                if (frame + 1 - carry.first).is_multiple_of(2) {
                    *WAITING.lock().unwrap_or_else(|e| e.into_inner()) = None;
                    arm(frame + 1, carry, folder);
                }
            } else if let Some(why) = trigger(folder, frame) {
                static TAKEN: AtomicU32 = AtomicU32::new(0);
                let most = number("VICTORY_SHOT_MAX", 12);
                if u64::from(TAKEN.fetch_add(1, Relaxed)) >= most {
                    tracing::warn!(most, "frame shot not taken: the run has its most already (VICTORY_SHOT_MAX)");
                } else {
                    arm(frame + 1, Carry { first: frame + 1, why, ..Default::default() }, folder);
                }
            }
        }
        RECORDING => {
            let mut shot = SHOT.lock().unwrap_or_else(|e| e.into_inner());
            let Some(s) = shot.as_mut() else {
                PHASE.store(IDLE, Relaxed);
                return;
            };
            s.presents += 1;
            // The frame's own present is the one after its lists were
            // submitted.
            let goes_on = !closing() && s.presents < 3;
            if s.carry.parts == 0 {
                let text = format!(
                    "PRESENT frame {frame}: {} draws and dispatches recorded since the shot began{}",
                    s.work,
                    if goes_on { "; the shot goes on through the next frame" } else { "" }
                );
                s.events.push(Event { list: 0, thread: 0, stamp: u32::MAX, text });
            }
            if !goes_on {
                s.stopped = Some((std::time::Instant::now(), frame));
                PHASE.store(PENDING, Relaxed);
                tracing::info!(
                    frame,
                    part = s.carry.parts,
                    lines = s.events.len(),
                    places = s.places,
                    pictures = s.snaps.len(),
                    for_later = s.later,
                    read_back_mb = s.bytes >> 20,
                    "frame shot: a frame is recorded; its pictures are read in two seconds"
                );
            }
        }
        PENDING => {
            let mut shot = SHOT.lock().unwrap_or_else(|e| e.into_inner());
            let ready = shot
                .as_ref()
                .and_then(|s| s.stopped)
                .is_some_and(|(when, of)| when.elapsed() >= std::time::Duration::from_secs(2) && frame >= of + 8);
            if ready && let Some(s) = shot.take() {
                PHASE.store(SAVING, Relaxed);
                let to = folder.join(format!("shot-{}", s.carry.first));
                let spawned = std::thread::Builder::new().name("victory-shot".into()).spawn(move || {
                    crate::framelog::keep_on_slow_cores("frame shot");
                    let next = save(s, &to);
                    *WAITING.lock().unwrap_or_else(|e| e.into_inner()) = next;
                    PHASE.store(IDLE, Relaxed);
                });
                if let Err(e) = spawned {
                    tracing::warn!(error = %e, "frame shot: no thread to write it out; dropped");
                    PHASE.store(IDLE, Relaxed);
                }
            }
        }
        _ => {}
    }
}

/// Starts a frame of a shot: frame `frame`, the shot's first or (`carry`
/// says) a later one.
fn arm(frame: u64, carry: Carry, folder: &std::path::Path) {
    let mut room = room_mb();
    if room < LEAST_MB {
        if carry.parts != 0 {
            tracing::warn!(
                free_mb = crate::framelog::free_mb(),
                part = carry.parts,
                "frame shot ends here: too little memory free for more of its pictures"
            );
            finish(&carry, &folder.join(format!("shot-{}", carry.first)), "ended early: too little memory free for more pictures", 0);
            return;
        }
        // The lines and times need none of it.
        room = 0;
    }
    let lines = carry.parts == 0;
    let clock = lines && CLOCK.get_or_init(make_clock).is_some();
    NEXT.store(0, Relaxed);
    SUBMITTED.store(0, Relaxed);
    let id = SHOT_ID.fetch_add(1, Relaxed) + 1;
    tracing::info!(
        frame,
        part = carry.parts,
        pictures_so_far = carry.done.len(),
        why = carry.why,
        room_mb = room,
        free_mb = crate::framelog::free_mb(),
        timestamps = clock,
        "frame shot: recording the next frame"
    );
    *SHOT.lock().unwrap_or_else(|e| e.into_inner()) = Some(Shot {
        id,
        frame,
        carry,
        presents: 0,
        work: 0,
        events: Vec::with_capacity(if lines { 32768 } else { 0 }),
        snaps: Vec::new(),
        bytes: 0,
        budget: room << 20,
        places: 0,
        later: 0,
        not_taken: 0,
        per_texture: Default::default(),
        titles: Default::default(),
        resolved: Vec::new(),
        stopped: None,
    });
    LINES.store(lines, Relaxed);
    PHASE.store(RECORDING, Relaxed);
}

/// Writes one frame of a shot out and lets its buffers go. What the shot
/// carries on, if pictures are left for a later frame.
fn save(shot: Shot, to: &std::path::Path) -> Option<Carry> {
    use std::fmt::Write;
    use std::io::Write as _;
    let started = std::time::Instant::now();
    if let Err(e) = std::fs::create_dir_all(to) {
        tracing::warn!(error = %e, folder = %to.display(), "frame shot not saved: no folder");
        return None;
    }
    let mut carry = shot.carry.clone();
    let part = carry.parts;
    if part == 0 {
        // Lines: place, list, thread, timestamp's place (-1: none), text.
        let mut events = String::with_capacity(shot.events.iter().map(|e| e.text.len() + 40).sum());
        for (seq, e) in shot.events.iter().enumerate() {
            let stamp = if e.stamp == u32::MAX { -1 } else { i64::from(e.stamp) };
            let _ = writeln!(events, "{seq}\t{:#x}\t{}\t{stamp}\t{}", e.list, e.thread, e.text.replace(['\n', '\r', '\t'], " "));
        }
        if let Err(e) = std::fs::write(to.join("events.txt"), events) {
            tracing::warn!(error = %e, folder = %to.display(), "frame shot: its lines could not be written");
        }
        // Timestamps: the places lists resolved, as the card wrote them.
        let mut stamps = String::new();
        if let Some(Some(clock)) = CLOCK.get() {
            let _ = writeln!(stamps, "frequency\t{}", clock.frequency);
            for &(base, count) in &shot.resolved {
                for slot in base..(base + count).min(SLOTS) {
                    // SAFETY: a place inside the mapped read-back buffer,
                    // which the card wrote long ago (or never: then what
                    // it held).
                    let ticks = unsafe { std::ptr::read_volatile((clock.mapped as *const u64).add(slot as usize)) };
                    let _ = writeln!(stamps, "{slot}\t{ticks}");
                }
            }
        }
        let _ = std::fs::write(to.join("stamps.txt"), stamps);
        let _ = std::fs::write(
            to.join("snaps.txt"),
            "texture\tn\tslice\tfile\tpart\tframe\twidth\theight\tformat\tbefore\tafter\tnot_black_percent\tmean_rgb\n",
        );
        carry.lines = shot.events.len();
        carry.work = shot.work;
        carry.presents = shot.presents;
        carry.places = shot.places;
        carry.not_taken = shot.not_taken;
    }
    // Pictures.
    let full = std::env::var_os("VICTORY_SHOT_FULL").is_some_and(|v| v == "1");
    let mut list = String::new();
    let (mut saved, mut failed) = (0u32, 0u32);
    for snap in shot.snaps {
        let f = snap.footprint;
        let mut data = std::ptr::null_mut();
        // SAFETY: the copy finished long ago; the mapping spans the bytes
        // the footprint was sized for.
        let rgba = unsafe {
            if snap.buffer.Map(0, None, Some(&mut data)).is_err() || data.is_null() {
                None
            } else {
                let rows = std::slice::from_raw_parts(data.cast::<u8>(), snap.bytes as usize);
                let rgba = crate::capture::to_rgba(f.Format.0 as u32, f.Width as usize, f.Height as usize, f.RowPitch as usize, rows);
                snap.buffer.Unmap(0, None);
                rgba
            }
        };
        let Some(rgba) = rgba else {
            failed += 1;
            continue;
        };
        // Every `step`th pixel each way: 1280 across at most, unless asked.
        let step = if full { 1 } else { (f.Width.max(f.Height) as usize).div_ceil(1280).max(1) };
        let (w, h) = ((f.Width as usize).div_ceil(step), (f.Height as usize).div_ceil(step));
        let mut raw = Vec::with_capacity((w * 3 + 1) * h);
        let (mut lit, mut sum) = (0u64, [0u64; 3]);
        for y in 0..h {
            raw.push(0);
            for x in 0..w {
                let p = &rgba[((y * step) * f.Width as usize + x * step) * 4..][..3];
                raw.extend_from_slice(p);
                lit += u64::from(p.iter().any(|&c| c > 8));
                for (s, &c) in sum.iter_mut().zip(p) {
                    *s += u64::from(c);
                }
            }
        }
        let n = (w * h).max(1) as u64;
        match std::fs::write(to.join(format!("{}.png", snap.name)), crate::capture::png_rgb(w as u32, h as u32, &raw)) {
            Ok(()) => saved += 1,
            Err(_) => {
                failed += 1;
                continue;
            }
        }
        let _ = writeln!(
            list,
            "{:#x}\t{}\t{}\t{}.png\t{part}\t{}\t{}\t{}\t{}\t{:#x}\t{:#x}\t{:.1}\t{},{},{}",
            snap.texture,
            snap.occurrence,
            snap.slice,
            snap.name,
            shot.frame,
            snap.width,
            snap.height,
            snap.format,
            snap.before,
            snap.after,
            lit as f64 * 100.0 / n as f64,
            sum[0] / n,
            sum[1] / n,
            sum[2] / n
        );
    }
    if let Ok(mut file) = std::fs::OpenOptions::new().append(true).create(true).open(to.join("snaps.txt")) {
        let _ = file.write_all(list.as_bytes());
    }
    carry.parts += 1;
    carry.saved += saved;
    carry.failed += failed;
    carry.read_back_mb += shot.bytes >> 20;
    tracing::info!(
        folder = %to.display(),
        part,
        frame = shot.frame,
        pictures = saved,
        failed,
        for_later = shot.later,
        seconds = started.elapsed().as_secs_f64(),
        "frame shot: a frame's part is saved"
    );
    // More pictures in a later frame, while frames still get some.
    if shot.later == 0 {
        finish(&carry, to, "complete", 0);
        None
    } else if (saved == 0 && part != 0) || carry.parts >= MAX_PARTS {
        finish(&carry, to, "ended early: a frame got no further picture", shot.later);
        None
    } else {
        Some(carry)
    }
}

/// A shot's last files: `info.txt` and `done`.
fn finish(carry: &Carry, to: &std::path::Path, how: &str, missing: u32) {
    let info = format!(
        "frame\t{}\nwhy\t{}\nended\t{how}\npresents\t{}\ndraws_and_dispatches\t{}\nlines\t{}\nplaces_for_pictures\t{}\nplaces_never_taken\t{}\n\
         frames_taken_in\t{}\npictures_saved\t{}\npictures_failed\t{}\npictures_missing\t{missing}\nread_back_mb\t{}\n",
        carry.first,
        carry.why,
        carry.presents,
        carry.work,
        carry.lines,
        carry.places,
        carry.not_taken,
        carry.parts,
        carry.saved,
        carry.failed,
        carry.read_back_mb
    );
    let _ = std::fs::write(to.join("info.txt"), info);
    let _ = std::fs::write(to.join("done"), "");
    tracing::info!(
        folder = %to.display(),
        how,
        frames = carry.parts,
        lines = carry.lines,
        places = carry.places,
        pictures = carry.saved,
        failed = carry.failed,
        missing,
        "frame shot saved"
    );
}

#[cfg(test)]
mod tests {
    #[test]
    fn the_formats_shown_are_the_ones_the_pictures_decode() {
        // Every format `shows` names decodes, and one it does not name
        // does not (a block-compressed one: never a target).
        for format in (1..=2).chain(9..=16).chain(19..=65).chain(87..=93) {
            assert!(super::shows(format));
            let pixel = [0u8; 16];
            assert!(crate::capture::to_rgba(format as u32, 1, 1, 16, &pixel).is_some(), "format {format}");
        }
        assert!(!super::shows(71));
        assert!(crate::capture::to_rgba(71, 1, 1, 16, &[0u8; 16]).is_none());
    }
}
