//! Presented frames written to PNG files, to see what reaches the screen:
//! with `VICTORY_CAPTURE=<folder>`, every frame whose number is a power of
//! two (the frames the log reports) is saved as `frame-<n>.png`, at half
//! size. Only in builds with diagnostics.

use std::path::PathBuf;

use windows::Win32::Graphics::Direct3D12::*;
use windows::Win32::Graphics::Dxgi::Common::*;

/// The folder frames go to, if capturing.
pub(crate) fn folder() -> Option<&'static PathBuf> {
    static FOLDER: std::sync::OnceLock<Option<PathBuf>> = std::sync::OnceLock::new();
    FOLDER
        .get_or_init(|| std::env::var_os("VICTORY_CAPTURE").map(PathBuf::from))
        .as_ref()
}

/// Frames listed in `VICTORY_CAPTURE_FRAMES` (comma-separated): saved
/// too, and their draws traced at debug level. `VICTORY_CAPTURE_HEAVY_FRAMES`
/// lists frames counted from the heavy scene's first ([`heavy_frame`]).
pub(crate) fn listed(n: u64) -> bool {
    static LIST: std::sync::OnceLock<Vec<u64>> = std::sync::OnceLock::new();
    let named = LIST
        .get_or_init(|| {
            let list = std::env::var("VICTORY_CAPTURE_FRAMES").unwrap_or_default();
            list.split(',')
                .filter_map(|f| f.trim().parse().ok())
                .collect()
        })
        .contains(&n);
    // `VICTORY_CAPTURE_WORLD`: also the 2nd and the 6th frame after the first
    // one that ran 500 draws or more (a title's first 3D frames: their
    // numbers depend on how long the menus were shown), and the 30th and
    // the 80th (the title opens with a cutscene made in the
    // engine: later shots of it, in case the first is dark by design).
    //
    // `VICTORY_CAPTURE_WORLD_ALSO` (comma-separated): more frames counted
    // from that first one (the title's blue patches show from
    // about the 118th frame on, when the window's model has arrived and
    // the reflections hold values: none of the four above has them).
    static ALSO: std::sync::OnceLock<Vec<u64>> = std::sync::OnceLock::new();
    let also = ALSO.get_or_init(|| {
        let list = std::env::var("VICTORY_CAPTURE_WORLD_ALSO").unwrap_or_default();
        list.split(',')
            .filter_map(|f| f.trim().parse().ok())
            .collect()
    });
    let first = WORLD.load(std::sync::atomic::Ordering::Relaxed);
    let after = n.wrapping_sub(first);
    named || first != u64::MAX && ([2, 6, 30, 80].contains(&after) || also.contains(&after)) || heavy_listed(n)
}

/// `VICTORY_CAPTURE_HEAVY_FRAMES` (comma-separated): frames counted from
/// the heavy scene's first, whose number differs from build to build.
fn heavy_offsets() -> &'static [u64] {
    static OFFSETS: std::sync::OnceLock<Vec<u64>> = std::sync::OnceLock::new();
    OFFSETS.get_or_init(|| {
        let list = std::env::var("VICTORY_CAPTURE_HEAVY_FRAMES").unwrap_or_default();
        list.split(',').filter_map(|f| f.trim().parse().ok()).collect()
    })
}

/// The heavy scene's first presented frame, once seen.
#[derive(Debug, Default)]
struct Heavy {
    first: Option<u64>,
}

static HEAVY: std::sync::Mutex<Heavy> = std::sync::Mutex::new(Heavy { first: None });

/// The title draws and mesh dispatches a frame records, run or dropped
/// (`crate::draw::attempted`), that make it the heavy scene's first.
const HEAVY_ATTEMPTS: u64 = 1_000;

/// One presented frame against the heavy list: `attempts` is the title
/// draws and mesh dispatches it recorded, whether they ran or were dropped
/// (never compute dispatches: counted only when dropped, they made a
/// compute-heavy frame heavy on one build and not on another, Sol's
/// review), `state` the frames so far. Sets the scene's first frame when
/// this one reaches [`HEAVY_ATTEMPTS`], and returns whether `n` is one of
/// the listed frames after it.
fn heavy_frame(n: u64, attempts: u64, state: &mut Heavy, offsets: &[u64]) -> bool {
    if state.first.is_none() && attempts >= HEAVY_ATTEMPTS {
        state.first = Some(n);
    }
    state.first.is_some_and(|first| offsets.iter().any(|&k| first.saturating_add(k) == n))
}

/// Whether `n` is one of `VICTORY_CAPTURE_HEAVY_FRAMES` after the heavy
/// scene's first frame; before that frame nothing is.
fn heavy_listed(n: u64) -> bool {
    let offsets = heavy_offsets();
    !offsets.is_empty() && heavy_frame(n, 0, &mut HEAVY.lock().unwrap_or_else(|e| e.into_inner()), offsets)
}

/// With a capture folder, for the frames [`listed`]: `what` is added to
/// the frame's `frame-<n>-ops.txt`, one line per draw, dispatch, copy or
/// drop in the order recorded (the title records on several threads, so
/// lines of different lists interleave; each names its list). The file
/// says which pass writes which target, to find the pass whose output
/// stays empty.
#[cfg(feature = "diagnostics")]
pub(crate) fn op(list: usize, what: impl FnOnce() -> String) {
    use std::io::Write;
    let frame = crate::queue::presented() + 1;
    if !listed(frame) {
        // Before the world: the last lines are kept, and written out when
        // it starts (`before-world-ops.txt`): what the title drew just
        // before (the title shows a title card there, whose text
        // did not show).
        if world_on() && WORLD.load(std::sync::atomic::Ordering::Relaxed) == u64::MAX {
            // At most 300 lines of a frame: the world's first frame alone
            // (thousands of operations, recorded before it is known to be
            // the world's) pushed everything before it out.
            static IN_FRAME: std::sync::Mutex<(u64, u32)> = std::sync::Mutex::new((0, 0));
            {
                let mut in_frame = IN_FRAME.lock().unwrap_or_else(|e| e.into_inner());
                if in_frame.0 != frame {
                    *in_frame = (frame, 0);
                }
                in_frame.1 += 1;
                if in_frame.1 > 300 {
                    return;
                }
            }
            let what = what();
            let mut ring = RING.lock().unwrap_or_else(|e| e.into_inner());
            if ring.len() >= RING_LINES {
                ring.pop_front();
            }
            ring.push_back(format!("frame {frame} list {list:#x} {what}"));
        }
        return;
    }
    let Some(folder) = folder() else {
        return;
    };
    static FILE: std::sync::Mutex<Option<(u64, u64, std::fs::File)>> = std::sync::Mutex::new(None);
    let what = what();
    let mut file = FILE.lock().unwrap_or_else(|e| e.into_inner());
    if file.as_ref().is_none_or(|f| f.0 != frame) {
        let Ok(made) = std::fs::File::create(folder.join(format!("frame-{frame}-ops.txt"))) else {
            return;
        };
        *file = Some((frame, 0, made));
    }
    if let Some((_, n, f)) = file.as_mut() {
        *n += 1;
        let _ = writeln!(f, "{n} list {list:#x} {what}");
    }
}

/// The last lines of [`op`] before the world.
#[cfg(feature = "diagnostics")]
static RING: std::sync::Mutex<std::collections::VecDeque<String>> =
    std::sync::Mutex::new(std::collections::VecDeque::new());
#[cfg(feature = "diagnostics")]
const RING_LINES: usize = 6000;

/// Whether `VICTORY_CAPTURE_WORLD` is set.
fn world_on() -> bool {
    static ON: std::sync::OnceLock<bool> = std::sync::OnceLock::new();
    *ON.get_or_init(|| std::env::var_os("VICTORY_CAPTURE_WORLD").is_some())
}

/// The first frame that ran 500 draws or more (`VICTORY_CAPTURE_WORLD`).
static WORLD: std::sync::atomic::AtomicU64 = std::sync::atomic::AtomicU64::new(u64::MAX);

/// At the present of `frame`: the draws run on the host so far.
pub(crate) fn frame_drew(frame: u64, drawn: u64) {
    use std::sync::atomic::Ordering::Relaxed;
    static LAST: std::sync::atomic::AtomicU64 = std::sync::atomic::AtomicU64::new(0);
    static LAST_ATTEMPTS: std::sync::atomic::AtomicU64 = std::sync::atomic::AtomicU64::new(0);
    let attempted = crate::draw::attempted();
    let attempts = attempted.saturating_sub(LAST_ATTEMPTS.swap(attempted, Relaxed));
    let offsets = heavy_offsets();
    if !offsets.is_empty() {
        let mut heavy = HEAVY.lock().unwrap_or_else(|e| e.into_inner());
        let before = heavy.first;
        heavy_frame(frame, attempts, &mut heavy, offsets);
        if before.is_none() && heavy.first.is_some() {
            tracing::info!(frame, attempts, "the heavy scene's first frame (VICTORY_CAPTURE_HEAVY_FRAMES counts from it)");
        }
    }
    let before = LAST.swap(drawn, Relaxed);
    if world_on()
        && drawn.saturating_sub(before) >= 500
        && WORLD.compare_exchange(u64::MAX, frame, Relaxed, Relaxed).is_ok()
    {
        tracing::info!(frame, "the first frame of 500 draws or more: frames {} and {} are captured", frame + 2, frame + 6);
        #[cfg(feature = "diagnostics")]
        if let Some(folder) = folder() {
            let lines: Vec<String> = std::mem::take(&mut *RING.lock().unwrap_or_else(|e| e.into_inner())).into();
            let _ = std::fs::write(folder.join("before-world-ops.txt"), lines.join("
"));
        }
    }
}

/// Whether the frame being recorded is listed and a folder is set: then
/// indirect executions' records are read ([`read_indirect`]).
pub(crate) fn recording_listed() -> bool {
    folder().is_some() && listed(crate::queue::presented() + 1)
}

/// Where an indirect execution's records are.
pub(crate) enum Records<'a> {
    /// In the argument clamp's scratch buffer at an offset, in the
    /// indirect-argument state (`crate::indirect::bound`).
    Clamped(&'a ID3D12Resource, u64),
    /// In a title buffer's host copy at an offset, whose state the host
    /// does not name (buffers are promoted from COMMON on use).
    Title(&'a ID3D12Resource, u64),
}

/// The work `records` of `kind` ask for, in `words` (`stride` bytes
/// apart): how many ask for some, and how much in all. Draws: vertices
/// (or indices) times instances; dispatches and mesh dispatches: groups.
fn records_with_work(words: &[u32], kind: u32, stride: u32, records: usize) -> (usize, u64) {
    let step = (stride as usize / 4).max(1);
    let work = |r: usize| -> u64 {
        let w = words.get(r * step..(r * step + step.min(3)).min(words.len())).unwrap_or(&[]);
        match kind {
            0 | 1 => u64::from(w.first().copied().unwrap_or(0)) * u64::from(w.get(1).copied().unwrap_or(1)),
            _ => w.iter().take(3).map(|&x| u64::from(x)).product::<u64>() * u64::from(!w.is_empty()),
        }
    };
    let busy = (0..records).filter(|&r| work(r) > 0).count();
    (busy, (0..records).map(work).sum())
}

/// In a listed frame: copies an indirect execution's records and its count
/// (a title buffer's copy and offset) to read-back memory, logged once the
/// list has run, as our line logs it: "indirect kind K max M count C: B of
/// R records with work, W in all; first [...] <pipeline>". An execution
/// whose records ask for nothing draws nothing, which a dropped one does
/// for another reason; the line tells the two apart.
pub(crate) fn read_indirect(
    list: &crate::host::HostList,
    records: Records<'_>,
    count: Option<(&ID3D12Resource, u64)>,
    (kind, stride, max): (u32, u32, u32),
    name: String,
) {
    struct Readback(ID3D12Resource);
    // SAFETY: D3D12 objects are free-threaded.
    unsafe impl Send for Readback {}
    let Some(host) = crate::host::host() else { return };
    let len = u64::from(stride) * u64::from(max.max(1));
    let desc = D3D12_RESOURCE_DESC {
        Dimension: D3D12_RESOURCE_DIMENSION_BUFFER,
        Width: len + 4,
        Height: 1,
        DepthOrArraySize: 1,
        MipLevels: 1,
        SampleDesc: DXGI_SAMPLE_DESC { Count: 1, Quality: 0 },
        Layout: D3D12_TEXTURE_LAYOUT_ROW_MAJOR,
        ..Default::default()
    };
    let heap = D3D12_HEAP_PROPERTIES { Type: D3D12_HEAP_TYPE_READBACK, ..Default::default() };
    let mut buffer: Option<ID3D12Resource> = None;
    let l = &list.list;
    // SAFETY: plain creation; an open list. The clamp's scratch buffer goes
    // from the indirect-argument state to copy source and back; a title
    // buffer's copy is promoted to copy source by the copy itself.
    unsafe {
        if host
            .device
            .CreateCommittedResource(&heap, D3D12_HEAP_FLAG_NONE, &desc, D3D12_RESOURCE_STATE_COPY_DEST, None, &mut buffer)
            .is_err()
        {
            return;
        }
        let Some(buffer) = buffer else { return };
        match records {
            Records::Clamped(scratch, at) => {
                let (args, copy) = (D3D12_RESOURCE_STATE_INDIRECT_ARGUMENT, D3D12_RESOURCE_STATE_COPY_SOURCE);
                l.ResourceBarrier(&[crate::host::transition(scratch, args, copy)]);
                l.CopyBufferRegion(&buffer, 0, scratch, at, len);
                l.ResourceBarrier(&[crate::host::transition(scratch, copy, args)]);
            }
            Records::Title(copy, at) => l.CopyBufferRegion(&buffer, 0, copy, at, len),
        }
        if let Some((c, at)) = count {
            l.CopyBufferRegion(&buffer, len, c, at, 4);
        }
        list.keep(&buffer);
        let buffer = Readback(buffer);
        let counted = count.is_some();
        list.defer_after(move || {
            let mut data = std::ptr::null_mut();
            if buffer.0.Map(0, None, Some(&mut data)).is_err() || data.is_null() {
                return;
            }
            let words = std::slice::from_raw_parts(data.cast::<u32>(), (len as usize + 4) / 4);
            let n = counted.then(|| words[len as usize / 4]);
            let records = n.unwrap_or(max).min(max) as usize;
            let (busy, total) = records_with_work(words, kind, stride, records);
            let step = (stride as usize / 4).max(1);
            let shown: Vec<&[u32]> = (0..records.min(4)).map(|r| &words[r * step..r * step + step.min(4)]).collect();
            tracing::info!(
                target: "victory_gpu::trace",
                "indirect kind {kind} max {max} count {n:?}: {busy} of {records} records with work, {total} in all; first {shown:?} {name}"
            );
            buffer.0.Unmap(0, None);
        });
    }
}

/// `VICTORY_DUMP_AFTER=<hash>[,<hash>]`: in the frames [`listed`], with a
/// capture folder, the textures a draw or dispatch of a pipeline with one
/// of these shaders renders to or writes are saved straight after it, as
/// the GPU leaves them there (the copies are in the same list, behind the
/// draw): `frame-<n>-after-<hash>-<k>-<i>-<resource>-<w>x<h>-f<format>.png`,
/// the `k`th such draw of the frame, its `i`th texture. What
/// `VICTORY_DUMP_TARGETS` saves is each texture at the frame's end, after
/// every pass that writes it.
fn dump_after_hashes() -> &'static [String] {
    static HASHES: std::sync::OnceLock<Vec<String>> = std::sync::OnceLock::new();
    HASHES.get_or_init(|| {
        let list = std::env::var("VICTORY_DUMP_AFTER").unwrap_or_default();
        list.split(',').map(|h| h.trim().to_ascii_lowercase()).filter(|h| !h.is_empty()).collect()
    })
}

/// The textures the unordered access views of the draw or dispatch being
/// bound write, by host list: what [`dump_after`] saves besides render
/// targets.
static WRITTEN: std::sync::Mutex<Vec<(usize, usize)>> = std::sync::Mutex::new(Vec::new());

/// A draw or dispatch being bound on `list` writes `resource` through an
/// unordered access view.
pub(crate) fn note_written(list: usize, resource: crate::object::This) {
    if !dump_after_hashes().is_empty() && recording_listed() {
        WRITTEN.lock().unwrap_or_else(|e| e.into_inner()).push((list, resource as usize));
    }
}

/// The hash of `hashes` that `shaders` (a pipeline's, as
/// `crate::pipeline::shaders_of` gives them) has, if any.
fn named_after<'a>(hashes: &'a [String], shaders: &str) -> Option<&'a String> {
    let shaders = shaders.to_ascii_lowercase();
    hashes.iter().find(|h| shaders.contains(h.as_str()))
}

/// Which draw of `hash`'s pipeline this is in `frame`, from 0: `seen`
/// holds this frame's counts (and forgets those of earlier frames).
fn nth_after(seen: &mut Vec<(u64, String, u32)>, frame: u64, hash: &str) -> u32 {
    seen.retain(|s| s.0 == frame);
    match seen.iter_mut().find(|s| s.1 == hash) {
        Some(s) => {
            s.2 += 1;
            s.2
        }
        None => {
            seen.push((frame, hash.to_owned(), 0));
            0
        }
    }
}

/// Straight after a draw or dispatch of `pipeline` on `list` (see
/// [`dump_after_hashes`]): `targets` (render targets and depth) and the
/// textures it writes through unordered access are copied out in the list
/// and saved once the list has run.
pub(crate) fn dump_after(list: &crate::host::HostList, pipeline: crate::object::This, targets: &[usize]) {
    let hashes = dump_after_hashes();
    if hashes.is_empty() {
        return;
    }
    let key = std::ptr::from_ref(list) as usize;
    let mut textures: Vec<usize> = targets.iter().copied().filter(|&t| t != 0).collect();
    WRITTEN.lock().unwrap_or_else(|e| e.into_inner()).retain(|&(l, r)| {
        if l == key && !textures.contains(&r) {
            textures.push(r);
        }
        l != key
    });
    if !recording_listed() {
        return;
    }
    let Some(hash) = named_after(hashes, &crate::pipeline::shaders_of(pipeline)) else {
        return;
    };
    let frame = crate::queue::presented() + 1;
    static SEEN: std::sync::Mutex<Vec<(u64, String, u32)>> = std::sync::Mutex::new(Vec::new());
    let k = nth_after(&mut SEEN.lock().unwrap_or_else(|e| e.into_inner()), frame, hash);
    let Some(host) = crate::host::host() else { return };
    for (i, &resource) in textures.iter().enumerate() {
        let Some(copy) = crate::resource::host_copy(resource as crate::object::This) else { continue };
        // SAFETY: a live resource.
        let d = unsafe { copy.texture.GetDesc() };
        if d.SampleDesc.Count > 1 || d.Dimension != D3D12_RESOURCE_DIMENSION_TEXTURE2D {
            continue;
        }
        // In the list's own account of the texture's state: the next draw
        // that uses it moves it back.
        crate::list::ensure_state(list, resource as crate::object::This, D3D12_RESOURCE_STATE_COPY_SOURCE.0 as u32);
        let Some((buffer, footprint)) = read_back(&host.device, &list.list, &copy.texture) else { continue };
        list.keep(&buffer);
        struct Readback(ID3D12Resource);
        // SAFETY: D3D12 objects are free-threaded.
        unsafe impl Send for Readback {}
        let buffer = Readback(buffer);
        let name = format!("frame-{frame}-after-{hash}-{k}-{i:02}-{resource:#x}-{}x{}-f{}", d.Width, d.Height, d.Format.0);
        list.defer_after(move || {
            let f = footprint.Footprint;
            let mut data = std::ptr::null_mut();
            // SAFETY: the list has run; the mapping spans the footprint's rows.
            unsafe {
                if buffer.0.Map(0, None, Some(&mut data)).is_err() || data.is_null() {
                    return;
                }
                let pitch = f.RowPitch as usize;
                let rows = std::slice::from_raw_parts(data.cast::<u8>(), pitch * f.Height as usize);
                match to_rgba(f.Format.0 as u32, f.Width as usize, f.Height as usize, pitch, rows) {
                    Some(rgba) => save_as(&name, f.Width, f.Height, f.Width as usize * 4, false, &rgba),
                    None => tracing::info!(format = f.Format.0, name, "texture after a draw not dumped: format"),
                }
                buffer.0.Unmap(0, None);
            }
        });
    }
}

/// Whether frame `n` is saved.
pub(crate) fn wanted(n: u64) -> bool {
    // Two frames in a row each time: the title draws to two buffers in turn,
    // and every 8th or 16th frame alone only ever showed one of them.
    // Before the world, with `VICTORY_CAPTURE_WORLD` and
    // `VICTORY_CAPTURE_MENUS=1`: every 16th frame too (small pictures of
    // menus, loading screens and title cards). Asked for apart: saving them
    // makes the menus and their music stutter.
    static MENUS: std::sync::OnceLock<bool> = std::sync::OnceLock::new();
    let menus = *MENUS.get_or_init(|| std::env::var_os("VICTORY_CAPTURE_MENUS").is_some_and(|v| v == "1"));
    let before_world = menus && world_on() && n % 16 < 2 && WORLD.load(std::sync::atomic::Ordering::Relaxed) == u64::MAX;
    // In the world: every 8th frame's picture, a timeline of the opening
    // cutscene (its shots do not fall on the same frame numbers from run to
    // run, so single frames cannot be compared between runs).
    let first = WORLD.load(std::sync::atomic::Ordering::Relaxed);
    let in_world = world_on() && first != u64::MAX && n > first && (n - first) % 8 < 2;
    folder().is_some() && (n.is_power_of_two() || listed(n) || before_world || in_world)
}

/// Saves 8-bit RGBA or BGRA rows (`pitch` bytes apart) as frame `n`.
pub(crate) fn save(n: u64, width: u32, height: u32, pitch: usize, bgra: bool, rows: &[u8]) {
    save_as(&format!("frame-{n}"), width, height, pitch, bgra, rows);
}

/// Saves 8-bit RGBA or BGRA rows as `<name>.png`, at half size.
fn save_as(name: &str, width: u32, height: u32, pitch: usize, bgra: bool, rows: &[u8]) {
    let Some(folder) = folder() else { return };
    let (w, h) = ((width / 2).max(1), (height / 2).max(1));
    // Filter byte 0 per row, then RGB of every other pixel of every other row.
    let mut raw = Vec::with_capacity((w as usize * 3 + 1) * h as usize);
    for y in 0..h as usize {
        raw.push(0);
        let row = &rows[(2 * y).min(height as usize - 1) * pitch..];
        for x in 0..w as usize {
            let p = &row[(2 * x).min(width as usize - 1) * 4..][..3];
            if bgra {
                raw.extend([p[2], p[1], p[0]]);
            } else {
                raw.extend_from_slice(p);
            }
        }
    }
    let mut png = b"\x89PNG\r\n\x1a\n".to_vec();
    let mut header = Vec::new();
    header.extend(w.to_be_bytes());
    header.extend(h.to_be_bytes());
    header.extend([8, 2, 0, 0, 0]); // 8-bit RGB
    chunk(&mut png, b"IHDR", &header);
    let mut compressed = vec![0u8; zlib_rs::compress_bound(raw.len())];
    let (idat, rc) = zlib_rs::compress_slice(&mut compressed, &raw, zlib_rs::DeflateConfig::new(3));
    debug_assert_eq!(rc, zlib_rs::ReturnCode::Ok);
    chunk(&mut png, b"IDAT", idat);
    chunk(&mut png, b"IEND", &[]);
    let path = folder.join(format!("{name}.png"));
    match std::fs::write(&path, png) {
        Ok(()) => tracing::info!(path = %path.display(), "frame saved"),
        Err(e) => tracing::warn!(error = %e, path = %path.display(), "frame not saved"),
    }
}

fn chunk(png: &mut Vec<u8>, kind: &[u8; 4], data: &[u8]) {
    png.extend((data.len() as u32).to_be_bytes());
    let start = png.len();
    png.extend(kind);
    png.extend(data);
    let crc = crc32(&png[start..]);
    png.extend(crc.to_be_bytes());
}

fn crc32(bytes: &[u8]) -> u32 {
    let mut crc = !0u32;
    for &b in bytes {
        crc ^= u32::from(b);
        for _ in 0..8 {
            crc = (crc >> 1) ^ (0xedb8_8320 & (crc & 1).wrapping_neg());
        }
    }
    !crc
}

/// Records a copy of `texture` (in the copy-source state) into a
/// readback buffer.
pub(crate) fn read_back(
    device: &ID3D12Device,
    list: &ID3D12GraphicsCommandList,
    texture: &ID3D12Resource,
) -> Option<(ID3D12Resource, D3D12_PLACED_SUBRESOURCE_FOOTPRINT)> {
    read_back_of(device, list, texture, 0)
}

/// As [`read_back`], of subresource `sub` (the stencil plane of a
/// one-level, one-slice depth-stencil texture is subresource 1).
fn read_back_of(
    device: &ID3D12Device,
    list: &ID3D12GraphicsCommandList,
    texture: &ID3D12Resource,
    sub: u32,
) -> Option<(ID3D12Resource, D3D12_PLACED_SUBRESOURCE_FOOTPRINT)> {
    let mut footprint = D3D12_PLACED_SUBRESOURCE_FOOTPRINT::default();
    let mut total = 0;
    // SAFETY: a live texture; outputs for one subresource.
    unsafe {
        let desc = texture.GetDesc();
        device.GetCopyableFootprints(
            &desc,
            sub,
            1,
            0,
            Some(&mut footprint),
            None,
            None,
            Some(&mut total),
        );
    }
    let heap = D3D12_HEAP_PROPERTIES {
        Type: D3D12_HEAP_TYPE_READBACK,
        ..Default::default()
    };
    let desc = D3D12_RESOURCE_DESC {
        Dimension: D3D12_RESOURCE_DIMENSION_BUFFER,
        Width: total,
        Height: 1,
        DepthOrArraySize: 1,
        MipLevels: 1,
        SampleDesc: DXGI_SAMPLE_DESC {
            Count: 1,
            Quality: 0,
        },
        Layout: D3D12_TEXTURE_LAYOUT_ROW_MAJOR,
        ..Default::default()
    };
    let mut buffer: Option<ID3D12Resource> = None;
    // SAFETY: plain creation; an open list; locations naming live resources.
    unsafe {
        device
            .CreateCommittedResource(
                &heap,
                D3D12_HEAP_FLAG_NONE,
                &desc,
                D3D12_RESOURCE_STATE_COPY_DEST,
                None,
                &mut buffer,
            )
            .ok()?;
        let buffer = buffer?;
        let to = D3D12_TEXTURE_COPY_LOCATION {
            pResource: std::mem::transmute_copy(&buffer),
            Type: D3D12_TEXTURE_COPY_TYPE_PLACED_FOOTPRINT,
            Anonymous: D3D12_TEXTURE_COPY_LOCATION_0 {
                PlacedFootprint: footprint,
            },
        };
        let from = D3D12_TEXTURE_COPY_LOCATION {
            pResource: std::mem::transmute_copy(texture),
            Type: D3D12_TEXTURE_COPY_TYPE_SUBRESOURCE_INDEX,
            Anonymous: D3D12_TEXTURE_COPY_LOCATION_0 {
                SubresourceIndex: sub,
            },
        };
        list.CopyTextureRegion(&to, 0, 0, 0, &from, None);
        Some((buffer, footprint))
    }
}

/// Saves a frame read back by [`read_back`] (8-bit color only).
pub(crate) fn save_read_back(
    frame: u64,
    buffer: &ID3D12Resource,
    footprint: &D3D12_PLACED_SUBRESOURCE_FOOTPRINT,
) {
    let f = footprint.Footprint;
    let bgra = match f.Format {
        DXGI_FORMAT_R8G8B8A8_UNORM | DXGI_FORMAT_R8G8B8A8_UNORM_SRGB => false,
        DXGI_FORMAT_B8G8R8A8_UNORM | DXGI_FORMAT_B8G8R8A8_UNORM_SRGB => true,
        other => {
            tracing::warn!(format = other.0, "frame not saved: not 8-bit color");
            return;
        }
    };
    let mut data = std::ptr::null_mut();
    // SAFETY: the copy finished (every submission is waited for); the
    // mapping spans the footprint's rows.
    unsafe {
        if buffer.Map(0, None, Some(&mut data)).is_err() {
            return;
        }
        let len = f.RowPitch as usize * f.Height as usize;
        let rows = std::slice::from_raw_parts(data.cast::<u8>(), len);
        save(frame, f.Width, f.Height, f.RowPitch as usize, bgra, rows);
        buffer.Unmap(0, None);
    }
}

/// With `VICTORY_DUMP_TARGETS` set: title textures the frames listed in
/// `VICTORY_CAPTURE_FRAMES` render to or write, in first-use order, saved
/// at that frame's present as `frame-<n>-<i>-<resource>-<w>x<h>-f<format>.png`
/// (what each holds at the end of the frame; formats normalized to show
/// structure: floats scaled by their largest value, depth by its range).
/// The host copies are held: the title may release a target before the
/// frame is presented.
static TARGETS: std::sync::Mutex<Vec<(usize, std::sync::Arc<crate::resource::HostTexture>)>> =
    std::sync::Mutex::new(Vec::new());

fn dump_on() -> bool {
    static ON: std::sync::OnceLock<bool> = std::sync::OnceLock::new();
    *ON.get_or_init(|| folder().is_some() && std::env::var_os("VICTORY_DUMP_TARGETS").is_some())
}

/// Notes a title texture the frame being built renders to or writes.
pub(crate) fn note_target(resource: crate::object::This) {
    if resource.is_null() || !dump_on() || !listed(crate::queue::presented() + 1) {
        return;
    }
    let mut targets = TARGETS.lock().unwrap_or_else(|e| e.into_inner());
    if !targets.iter().any(|&(r, _)| r == resource as usize)
        && let Some(copy) = crate::resource::host_copy(resource)
    {
        targets.push((resource as usize, copy));
    }
}

/// At the present of frame `frame`: saves the noted targets.
pub(crate) fn dump_targets(frame: u64) {
    use std::sync::atomic::Ordering::Relaxed;
    let targets = std::mem::take(&mut *TARGETS.lock().unwrap_or_else(|e| e.into_inner()));
    let Some(host) = crate::host::host() else { return };
    for (i, (resource, copy)) in targets.iter().enumerate() {
        let texture = &copy.texture;
        // SAFETY: a live resource.
        let d = unsafe { texture.GetDesc() };
        if d.SampleDesc.Count > 1 || d.Dimension != D3D12_RESOURCE_DIMENSION_TEXTURE2D {
            continue;
        }
        // Every submission has run; the dump's list moves it there and back.
        let state = copy.state.load(Relaxed);
        dump_one(host, texture, state, frame, i, *resource as crate::object::This, &d);
    }
}

fn dump_one(
    host: &crate::host::Host,
    texture: &ID3D12Resource,
    state: u32,
    frame: u64,
    i: usize,
    resource: crate::object::This,
    d: &D3D12_RESOURCE_DESC,
) {
    let Ok(list) = host.new_list() else { return };
    let state = D3D12_RESOURCE_STATES(crate::host::legal_state(texture, state) as i32);
    let copy = D3D12_RESOURCE_STATE_COPY_SOURCE;
    // SAFETY: an open list of ours and a live texture.
    let read = unsafe {
        if state != copy {
            list.list
                .ResourceBarrier(&[crate::host::transition(texture, state, copy)]);
        }
        let read = read_back(&host.device, &list.list, texture);
        // The stencil plane of a depth-stencil target, which the title's
        // lighting reads (material class in the low bits; 0x08 "inside a
        // room", which the lamps of a room are multiplied by): the plane
        // after the depth plane of a one-level, one-slice texture.
        let stencil = (matches!(d.Format.0, 19..=22 | 44..=47)
            && d.MipLevels == 1
            && d.DepthOrArraySize == 1)
            .then(|| read_back_of(&host.device, &list.list, texture, 1))
            .flatten();
        if state != copy {
            list.list
                .ResourceBarrier(&[crate::host::transition(texture, copy, state)]);
        }
        (read, stencil)
    };
    let (read, stencil) = read;
    list.close();
    host.execute(&[&list]);
    if let Some((buffer, footprint)) = stencil {
        stencil_values(&buffer, &footprint, frame, resource);
    }
    let Some((buffer, footprint)) = read else { return };
    let f = footprint.Footprint;
    let mut data = std::ptr::null_mut();
    // SAFETY: the copy finished; the mapping spans the footprint's rows.
    unsafe {
        if buffer.Map(0, None, Some(&mut data)).is_err() {
            return;
        }
        let pitch = f.RowPitch as usize;
        let rows = std::slice::from_raw_parts(data.cast::<u8>(), pitch * f.Height as usize);
        // A 24-bit depth plane comes back as 32-bit words: which depth values
        // (low 24 bits) and which top bytes it holds, the most common first.
        if matches!(d.Format.0, 44..=47) {
            let mut depths = std::collections::HashMap::<u32, u32>::new();
            let mut tops = std::collections::HashMap::<u32, u32>::new();
            for y in 0..f.Height as usize {
                for x in 0..f.Width as usize {
                    let at = y * pitch + x * 4;
                    let w = u32::from_le_bytes(rows[at..at + 4].try_into().unwrap_or([0; 4]));
                    *depths.entry(w & 0xff_ffff).or_default() += 1;
                    *tops.entry(w >> 24).or_default() += 1;
                }
            }
            let most = |m: std::collections::HashMap<u32, u32>| {
                let mut v: Vec<(u32, u32)> = m.into_iter().collect();
                v.sort_by_key(|e| std::cmp::Reverse(e.1));
                (v.len(), v.into_iter().take(12).collect::<Vec<_>>())
            };
            tracing::info!(?resource, depths = ?most(depths), top_bytes = ?most(tops), "24-bit depth plane values (value, texels)");
        }
        if let Some(rgba) = to_rgba(f.Format.0 as u32, f.Width as usize, f.Height as usize, pitch, rows) {
            let name = format!(
                "frame-{frame}-{i:02}-{resource:?}-{}x{}-f{}",
                d.Width, d.Height, d.Format.0
            );
            save_as(&name, f.Width, f.Height, f.Width as usize * 4, false, &rgba);
        } else {
            tracing::info!(format = f.Format.0, ?resource, "target not dumped: format");
        }
        buffer.Unmap(0, None);
    }
}

/// Logs which values a stencil plane read back by [`read_back_of`] holds
/// (the most common first), how many texels have each of its bits, and the
/// box of the texels with bit 3 (0x08).
fn stencil_values(
    buffer: &ID3D12Resource,
    footprint: &D3D12_PLACED_SUBRESOURCE_FOOTPRINT,
    frame: u64,
    resource: crate::object::This,
) {
    let f = footprint.Footprint;
    let (w, h, pitch) = (f.Width as usize, f.Height as usize, f.RowPitch as usize);
    let mut data = std::ptr::null_mut();
    // SAFETY: the copy finished; the mapping spans the footprint's rows of
    // one byte a texel, the last row without its padding.
    unsafe {
        if h == 0 || w == 0 || buffer.Map(0, None, Some(&mut data)).is_err() {
            return;
        }
        let rows = std::slice::from_raw_parts(data.cast::<u8>(), pitch * (h - 1) + w);
        let mut values = [0u32; 256];
        let mut bits = [0u32; 8];
        // Left, top, right, bottom of the texels with bit 3.
        let mut inside = [u32::MAX, u32::MAX, 0, 0];
        for y in 0..h {
            for (x, &v) in rows[y * pitch..y * pitch + w].iter().enumerate() {
                values[usize::from(v)] += 1;
                for (bit, count) in bits.iter_mut().enumerate() {
                    *count += u32::from(v >> bit & 1);
                }
                if v & 8 != 0 {
                    inside = [
                        inside[0].min(x as u32),
                        inside[1].min(y as u32),
                        inside[2].max(x as u32),
                        inside[3].max(y as u32),
                    ];
                }
            }
        }
        let mut most: Vec<(usize, u32)> = values
            .iter()
            .copied()
            .enumerate()
            .filter(|v| v.1 != 0)
            .collect();
        most.sort_by_key(|v| std::cmp::Reverse(v.1));
        most.truncate(16);
        tracing::info!(
            frame,
            ?resource,
            width = w,
            height = h,
            values = ?most,
            texels_with_bit = ?bits,
            bit3_box = ?inside,
            "stencil plane values (value, texels)"
        );
        buffer.Unmap(0, None);
    }
}

fn half(h: u16) -> f32 {
    let (sign, exp, man) = ((h >> 15) as u32, ((h >> 10) & 0x1f) as i32, (h & 0x3ff) as f32);
    let v = match exp {
        0 => man * 2f32.powi(-24),
        31 => f32::INFINITY,
        e => (1.0 + man / 1024.0) * 2f32.powi(e - 15),
    };
    if sign == 1 { -v } else { v }
}

/// A float packed in `bits` with a 5-bit exponent and `man` mantissa bits
/// (the 11- and 10-bit floats of R11G11B10).
fn small_float(bits: u32, man: u32) -> f32 {
    let (exp, m) = ((bits >> man) & 0x1f, (bits & ((1 << man) - 1)) as f32);
    match exp {
        0 => m / (1 << man) as f32 * 2f32.powi(-14),
        31 => f32::INFINITY,
        e => (1.0 + m / (1 << man) as f32) * 2f32.powi(e as i32 - 15),
    }
}

/// Rows of a host format as 8-bit RGBA: 8-bit color as is, everything else
/// as channel values scaled so the largest finite one is white (single
/// channels: lowest black, highest white, as grey). `None` for formats not
/// handled.
fn to_rgba(format: u32, w: usize, h: usize, pitch: usize, rows: &[u8]) -> Option<Vec<u8>> {
    let mut out = Vec::with_capacity(w * h * 4);
    let px = |x: usize, y: usize, n: usize| &rows[y * pitch + x * n..][..n];
    let u16_at = |b: &[u8], i: usize| u16::from_le_bytes([b[2 * i], b[2 * i + 1]]);
    let u32_at = |b: &[u8], i: usize| u32::from_le_bytes(b[4 * i..4 * i + 4].try_into().unwrap_or([0; 4]));
    // Channel values as floats, then scaled.
    type Decode = Box<dyn Fn(&[u8]) -> [f32; 3]>;
    let (n, decode): (usize, Decode) = match format {
        27..=32 => {
            for y in 0..h {
                for x in 0..w {
                    let p = px(x, y, 4);
                    out.extend([p[0], p[1], p[2], 255]);
                }
            }
            return Some(out);
        }
        // B8G8R8A8 and B8G8R8X8 (89 is R10G10B10_XR_BIAS_A2, below).
        87 | 88 | 90..=93 => {
            for y in 0..h {
                for x in 0..w {
                    let p = px(x, y, 4);
                    out.extend([p[2], p[1], p[0], 255]);
                }
            }
            return Some(out);
        }
        23..=25 | 89 => (4, Box::new(move |b| {
            let v = u32_at(b, 0);
            [(v & 0x3ff) as f32, (v >> 10 & 0x3ff) as f32, (v >> 20 & 0x3ff) as f32]
        })),
        26 => (4, Box::new(move |b| {
            let v = u32_at(b, 0);
            [small_float(v & 0x7ff, 6), small_float(v >> 11 & 0x7ff, 6), small_float(v >> 22, 5)]
        })),
        9..=10 => (8, Box::new(move |b| [half(u16_at(b, 0)), half(u16_at(b, 1)), half(u16_at(b, 2))])),
        11..=14 => (8, Box::new(move |b| [0, 1, 2].map(|i| f32::from(u16_at(b, i))))),
        33..=34 => (4, Box::new(move |b| [half(u16_at(b, 0)), half(u16_at(b, 1)), 0.0])),
        1..=2 => (16, Box::new(move |b| {
            [0, 1, 2].map(|i| f32::from_bits(u32_at(b, i)))
        })),
        15..=16 => (8, Box::new(move |b| [f32::from_bits(u32_at(b, 0)), f32::from_bits(u32_at(b, 1)), 0.0])),
        35..=38 => (4, Box::new(move |b| [f32::from(u16_at(b, 0)), f32::from(u16_at(b, 1)), 0.0])),
        48..=52 => (2, Box::new(move |b| [f32::from(b[0]), f32::from(b[1]), 0.0])),
        // Single channels (and depth planes): shown as grey.
        39..=43 => (4, Box::new(move |b| [f32::from_bits(u32_at(b, 0)); 3])),
        19..=22 => (8, Box::new(move |b| [f32::from_bits(u32_at(b, 0)); 3])),
        44..=47 => (4, Box::new(move |b| [(u32_at(b, 0) & 0xff_ffff) as f32; 3])),
        53..=54 => (2, Box::new(move |b| [half(u16_at(b, 0)); 3])),
        55..=59 => (2, Box::new(move |b| [f32::from(u16_at(b, 0)); 3])),
        60..=65 => (1, Box::new(move |b| [f32::from(b[0]); 3])),
        _ => return None,
    };
    let values: Vec<[f32; 3]> = (0..h)
        .flat_map(|y| (0..w).map(move |x| (x, y)))
        .map(|(x, y)| decode(px(x, y, n)))
        .collect();
    let grey = matches!(format, 19..=22 | 39..=47 | 53..=65);
    let finite = values.iter().flatten().copied().filter(|v| v.is_finite());
    let (low, high) = if grey {
        finite.fold((f32::MAX, f32::MIN), |(l, h), v| (l.min(v), h.max(v)))
    } else {
        (0.0, finite.fold(0.0f32, f32::max))
    };
    let range = (high - low).max(1e-20);
    // How much of the target holds anything: a lit scene and one bright
    // lamp on black have the same highest value.
    let above = values.iter().filter(|v| v.iter().any(|c| c.is_finite() && *c > low)).count();
    let nonzero_percent = above as f32 * 100.0 / values.len().max(1) as f32;
    tracing::info!(format, low, high, nonzero_percent, "target values");
    // Small float colour targets (the 1x1 luminance and exposure targets):
    // every texel's four channels as numbers. The picture has no alpha and
    // eight bits a channel.
    if matches!(format, 1..=2 | 9..=10) && w * h <= 16 {
        for y in 0..h {
            for x in 0..w {
                let b = px(x, y, n);
                let texel: [f32; 4] = if n == 16 {
                    [0, 1, 2, 3].map(|i| f32::from_bits(u32_at(b, i)))
                } else {
                    [0, 1, 2, 3].map(|i| half(u16_at(b, i)))
                };
                tracing::info!(format, x, y, ?texel, "target texel");
            }
        }
    }
    // R32_UINT targets hold packed light (the title reads them back as
    // R9G9B9E5): as floats their low, high and share above the lowest say
    // nothing. How many texels hold any bit, and the highest channel value
    // of the packed reading.
    if format == 42 {
        let (mut set, mut packed_high) = (0usize, 0f32);
        for y in 0..h {
            for x in 0..w {
                let v = u32_at(px(x, y, 4), 0);
                set += usize::from(v != 0);
                let scale = 2f32.powi((v >> 27) as i32 - 24);
                for k in 0..3 {
                    packed_high = packed_high.max((v >> (9 * k) & 0x1ff) as f32 * scale);
                }
            }
        }
        let nonzero_bits_percent = set as f32 * 100.0 / (w * h).max(1) as f32;
        tracing::info!(format, nonzero_bits_percent, packed_high, "target values as packed light");
        // The same words as plain numbers, by class. Tables of indices and
        // masks say nothing as floats or as packed light: in the title's
        // shadow-mask table 0 is a block in full shadow, a word up to 255
        // one value for the block, above that a block of the atlas, and all
        // ones a texel never written.
        let mut classes = [0usize; 6];
        let mut highest_below_ones = 0u32;
        for y in 0..h {
            for x in 0..w {
                let v = u32_at(px(x, y, 4), 0);
                classes[match v {
                    0 => 0,
                    1..=15 => 1,
                    16..=255 => 2,
                    256..=0x00ff_ffff => 3,
                    u32::MAX => 5,
                    _ => 4,
                }] += 1;
                if v != u32::MAX {
                    highest_below_ones = highest_below_ones.max(v);
                }
            }
        }
        tracing::info!(
            format,
            width = w,
            height = h,
            zero = classes[0],
            one_to_15 = classes[1],
            to_255 = classes[2],
            to_24_bits = classes[3],
            larger = classes[4],
            all_ones = classes[5],
            highest_below_ones,
            "target values as words"
        );
    }
    // Float colours (scene light) go through a cube root: scaled linearly,
    // a night scene under one lamp of 50 units came out all black.
    let curved = matches!(format, 1..=2 | 9..=10 | 26);
    for v in values {
        let c = v.map(|c| {
            let t = ((c - low) / range).clamp(0.0, 1.0);
            ((if curved { t.cbrt() } else { t }) * 255.0) as u8
        });
        out.extend([c[0], c[1], c[2], 255]);
    }
    Some(out)
}

/// Pixels per side of the grid [`probe`] reads.
const GRID: u32 = 4;
/// Bytes between the grid's pixels in the readback buffer (placement
/// alignment).
const SLOT: u64 = 512;

/// Records copies of a grid of pixels of `texture` (8-bit color, in the
/// copy-source state) into a readback buffer, to tell black frames.
pub(crate) fn probe(
    device: &ID3D12Device,
    list: &ID3D12GraphicsCommandList,
    texture: &ID3D12Resource,
) -> Option<ID3D12Resource> {
    // SAFETY: a live resource.
    let d = unsafe { texture.GetDesc() };
    if !matches!(d.Format.0, 27..=29 | 87 | 90 | 91) {
        return None;
    }
    let desc = D3D12_RESOURCE_DESC {
        Dimension: D3D12_RESOURCE_DIMENSION_BUFFER,
        Width: SLOT * u64::from(GRID * GRID),
        Height: 1,
        DepthOrArraySize: 1,
        MipLevels: 1,
        SampleDesc: DXGI_SAMPLE_DESC {
            Count: 1,
            Quality: 0,
        },
        Layout: D3D12_TEXTURE_LAYOUT_ROW_MAJOR,
        ..Default::default()
    };
    let heap = D3D12_HEAP_PROPERTIES {
        Type: D3D12_HEAP_TYPE_READBACK,
        ..Default::default()
    };
    let mut buffer: Option<ID3D12Resource> = None;
    // SAFETY: plain creation; an open list; locations naming live resources.
    unsafe {
        device
            .CreateCommittedResource(
                &heap,
                D3D12_HEAP_FLAG_NONE,
                &desc,
                D3D12_RESOURCE_STATE_COPY_DEST,
                None,
                &mut buffer,
            )
            .ok()?;
        let buffer = buffer?;
        for i in 0..GRID * GRID {
            let (x, y) = (
                (2 * (i % GRID) + 1) * d.Width as u32 / (2 * GRID),
                (2 * (i / GRID) + 1) * d.Height / (2 * GRID),
            );
            let to = D3D12_TEXTURE_COPY_LOCATION {
                pResource: std::mem::transmute_copy(&buffer),
                Type: D3D12_TEXTURE_COPY_TYPE_PLACED_FOOTPRINT,
                Anonymous: D3D12_TEXTURE_COPY_LOCATION_0 {
                    PlacedFootprint: D3D12_PLACED_SUBRESOURCE_FOOTPRINT {
                        Offset: SLOT * u64::from(i),
                        Footprint: D3D12_SUBRESOURCE_FOOTPRINT {
                            Format: d.Format,
                            Width: 1,
                            Height: 1,
                            Depth: 1,
                            RowPitch: 256,
                        },
                    },
                },
            };
            let from = D3D12_TEXTURE_COPY_LOCATION {
                pResource: std::mem::transmute_copy(texture),
                Type: D3D12_TEXTURE_COPY_TYPE_SUBRESOURCE_INDEX,
                Anonymous: D3D12_TEXTURE_COPY_LOCATION_0 {
                    SubresourceIndex: 0,
                },
            };
            let pixel = D3D12_BOX {
                left: x,
                top: y,
                front: 0,
                right: x + 1,
                bottom: y + 1,
                back: 1,
            };
            list.CopyTextureRegion(&to, 0, 0, 0, &from, Some(&pixel));
        }
        Some(buffer)
    }
}

/// Whether every pixel [`probe`] read is (near) black.
pub(crate) fn black(buffer: &ID3D12Resource) -> bool {
    let mut data = std::ptr::null_mut();
    // SAFETY: the copies finished (every submission is waited for).
    unsafe {
        if buffer.Map(0, None, Some(&mut data)).is_err() {
            return false;
        }
        let bytes =
            std::slice::from_raw_parts(data.cast::<u8>(), (SLOT * u64::from(GRID * GRID)) as usize);
        let level = bytes
            .chunks(SLOT as usize)
            .flat_map(|pixel| pixel[..3].iter().copied())
            .max()
            .unwrap_or(0);
        buffer.Unmap(0, None);
        let mut levels = LEVELS.lock().unwrap_or_else(|e| e.into_inner());
        if levels.len() == 12 {
            levels.pop_front();
        }
        levels.push_back(level);
        level < 8
    }
}

/// The brightest probed channel of the last frames, oldest first: a fade
/// darkens over several, a glitch drops at once.
static LEVELS: std::sync::Mutex<std::collections::VecDeque<u8>> =
    std::sync::Mutex::new(std::collections::VecDeque::new());

pub(crate) fn recent_levels() -> Vec<u8> {
    let levels = LEVELS.lock().unwrap_or_else(|e| e.into_inner());
    levels.iter().copied().collect()
}

#[cfg(test)]
mod tests {
    /// `VICTORY_DUMP_AFTER` names pipelines by any of their shaders' hashes
    /// (in either case), and numbers each one's draws from 0 in a frame.
    #[test]
    fn dump_after_names_a_pipeline_by_a_shader_and_counts_its_draws() {
        let hashes = ["ce3f0432a08f1d35".to_owned(), "a1bf269d5b19f025".to_owned()];
        let shaders = "[mesh F5E50EB2108C425B, pixel CE3F0432A08F1D35]";
        assert_eq!(super::named_after(&hashes, shaders).map(String::as_str), Some("ce3f0432a08f1d35"));
        assert_eq!(super::named_after(&hashes, "[compute 02384a60fda6f2d0]"), None);
        assert_eq!(super::named_after(&hashes, "no pipeline"), None);
        let mut seen = Vec::new();
        assert_eq!(super::nth_after(&mut seen, 7, "a"), 0);
        assert_eq!(super::nth_after(&mut seen, 7, "b"), 0);
        assert_eq!(super::nth_after(&mut seen, 7, "a"), 1);
        assert_eq!(super::nth_after(&mut seen, 8, "a"), 0, "a new frame counts afresh");
        assert_eq!(seen.len(), 1);
    }

    /// The records an indirect execution logs as having work: a draw's
    /// vertices times instances, a dispatch's or mesh dispatch's groups,
    /// only as many records as the count allows.
    #[test]
    fn records_with_work_count_what_each_kind_asks_for() {
        // Draws (vertices, instances, first vertex, first instance), 16 bytes.
        let draws = [3, 2, 0, 0, 0, 5, 0, 0, 4, 1, 0, 0];
        assert_eq!(super::records_with_work(&draws, 0, 16, 3), (2, 10), "6 and 4, and one of no vertices");
        assert_eq!(super::records_with_work(&draws, 0, 16, 1), (1, 6), "a count of one");
        // Mesh dispatches (x, y, z), 12 bytes: an empty one and two of work.
        let meshes = [2, 1, 1, 0, 1, 1, 8, 2, 1];
        assert_eq!(super::records_with_work(&meshes, 10, 12, 3), (2, 18));
        assert_eq!(super::records_with_work(&meshes, 10, 12, 0), (0, 0), "a count of zero");
    }

    /// The reader records its copies into a real list, which runs, and its
    /// line is made once the list has run (a title buffer's copy, as for a
    /// dropped execution: no barrier of ours on it).
    #[test]
    fn read_indirect_copies_records_and_count_into_a_list_that_runs() {
        let Some(host) = crate::host::host() else {
            return; // no host GPU here
        };
        let (buffer, data) = host.shared_buffer(1 << 16).unwrap();
        // SAFETY: inside the mapping: two mesh records and a count.
        unsafe { (data as *mut [u32; 7]).write([2, 1, 1, 0, 1, 1, 1]) };
        let list = host.new_list().unwrap();
        super::read_indirect(&list, super::Records::Title(&buffer, 0), Some((&buffer, 24)), (10, 12, 2), "test".to_owned());
        list.close();
        host.execute(&[&list]);
        // SAFETY: a plain device query.
        assert!(unsafe { host.device.GetDeviceRemovedReason() }.is_ok());
    }

    /// A title draw or mesh dispatch counts for the heavy scene's detector
    /// whether it runs or is dropped, and a compute dispatch never: here
    /// all are dropped (no pipeline). Before, the detector took draws run
    /// plus drops, so a dropped compute dispatch counted and a run one did
    /// not (Sol's review).
    #[test]
    fn draw_attempts_count_title_draws_run_or_not_and_no_dispatches() {
        let Some(host) = crate::host::host() else {
            return; // no host GPU here
        };
        let list = host.new_list().unwrap();
        let mut state = crate::draw::State::default();
        let before = crate::draw::attempted();
        state.draw(&list, 3, 1, 0, 0);
        state.draw_indexed(&list, 3, 1, 0, 0, 0);
        state.dispatch_mesh(&list, 1);
        assert_eq!(crate::draw::attempted() - before, 3, "a draw, an indexed draw and a mesh dispatch, all dropped");
        state.dispatch(&list, 1, 1, 1);
        assert_eq!(crate::draw::attempted() - before, 3, "a compute dispatch, dropped, is not one");
        list.close();
    }

    /// `VICTORY_CAPTURE_HEAVY_FRAMES` counts presented frames from the first
    /// frame that recorded 1,000 title draws and mesh dispatches, run or
    /// dropped. A light frame later does not move that start.
    #[test]
    fn heavy_frames_count_from_the_first_heavy_one() {
        let offsets = [2, 5];
        let mut state = super::Heavy::default();
        assert!(!super::heavy_frame(10, 999, &mut state, &offsets), "999 attempts: not heavy");
        assert_eq!(state.first, None);
        // 1,000: the scene starts here; the list counts from the frame after.
        assert!(!super::heavy_frame(11, 1_000, &mut state, &offsets));
        assert_eq!(state.first, Some(11));
        for (n, listed) in [(12, false), (13, true), (14, false), (15, false), (16, true), (17, false)] {
            assert_eq!(super::heavy_frame(n, 0, &mut state, &offsets), listed, "{n}");
        }
        assert!(!super::heavy_frame(40, 10, &mut state, &offsets), "a later light frame");
        assert_eq!(state.first, Some(11));
    }

    #[test]
    fn crc_matches_the_png_check_value() {
        assert_eq!(super::crc32(b"IEND"), 0xae42_6082);
    }

    /// R16G16B16A16_UNORM is integers, not half floats; 89 is 10:10:10:2,
    /// not B8G8R8A8.
    #[test]
    fn unorm16_and_xr_bias_targets_decode_as_their_formats() {
        // Red 0x8000 (a half-float -0), green 0x4000, blue 0.
        let unorm16 = [0x00, 0x80, 0x00, 0x40, 0, 0, 0xff, 0xff];
        assert_eq!(super::to_rgba(11, 1, 1, 8, &unorm16), Some(vec![255, 127, 0, 255]));
        // Red 0x3ff, green 0x1ff, blue 0, alpha 3.
        let xr = (0x3ffu32 | 0x1ff << 10 | 3 << 30).to_le_bytes();
        assert_eq!(super::to_rgba(89, 1, 1, 4, &xr), Some(vec![255, 127, 0, 255]));
        // B8G8R8A8 still swaps to RGBA.
        assert_eq!(super::to_rgba(87, 1, 1, 4, &[1, 2, 3, 4]), Some(vec![3, 2, 1, 255]));
    }
}
