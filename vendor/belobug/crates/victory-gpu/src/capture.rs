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
/// too, and their draws traced at debug level.
pub(crate) fn listed(n: u64) -> bool {
    n == ASKED.load(std::sync::atomic::Ordering::Relaxed) || in_list(n)
}

/// The frame whose targets `wantall` in the folder asked for.
static ASKED: std::sync::atomic::AtomicU64 = std::sync::atomic::AtomicU64::new(0);

fn in_list(n: u64) -> bool {
    static LIST: std::sync::OnceLock<Vec<u64>> = std::sync::OnceLock::new();
    LIST.get_or_init(|| {
        let list = std::env::var("VICTORY_CAPTURE_FRAMES").unwrap_or_default();
        list.split(',')
            .filter_map(|f| f.trim().parse().ok())
            .collect()
    })
    .contains(&n)
}

/// Whether frame `n` is saved.
pub(crate) fn wanted(n: u64) -> bool {
    folder().is_some_and(|f| {
        // `wantall` in the folder: the targets of a frame soon, once;
        // `want`: the next frame.
        if std::fs::remove_file(f.join("wantall")).is_ok() {
            crate::resource::census_log();
            ASKED.store(n + 2, std::sync::atomic::Ordering::Relaxed);
        }
        n.is_power_of_two() || listed(n) || std::fs::remove_file(f.join("want")).is_ok()
    })
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
    chunk(
        &mut png,
        b"IDAT",
        &miniz_oxide::deflate::compress_to_vec_zlib(&raw, 3),
    );
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
    let mut footprint = D3D12_PLACED_SUBRESOURCE_FOOTPRINT::default();
    let mut total = 0;
    // SAFETY: a live texture; outputs for one subresource.
    unsafe {
        let desc = texture.GetDesc();
        device.GetCopyableFootprints(
            &desc,
            0,
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
                SubresourceIndex: 0,
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

/// What the frame whose targets are dumped runs, as recorded: list, text.
static PASSES: std::sync::Mutex<Vec<(usize, String)>> = std::sync::Mutex::new(Vec::new());

/// Copies of the first render target taken within the frame (`VICTORY_SNAP`:
/// names of programs, before the first draw of which), with the target the
/// tiled lighting drew to.
#[allow(clippy::type_complexity)]
static SNAPS: std::sync::Mutex<(
    usize,
    Vec<(
        String,
        ID3D12Resource,
        windows::Win32::Graphics::Direct3D12::D3D12_PLACED_SUBRESOURCE_FOOTPRINT,
    )>,
)> = std::sync::Mutex::new((0, Vec::new()));

fn snap(list: &crate::host::HostList, text: &str, target: usize) {
    let Ok(wanted) = std::env::var("VICTORY_SNAP") else {
        return;
    };
    let mut snaps = SNAPS.lock().unwrap_or_else(|e| e.into_inner());
    if snaps.0 == 0 && text.contains("TL_PS_") {
        snaps.0 = target;
    }
    let Some(name) = wanted
        .split(',')
        .find(|w| text.contains(*w) && !snaps.1.iter().any(|s| s.0 == **w))
    else {
        return;
    };
    let resource = snaps.0 as crate::object::This;
    let (Some(host), Some(texture)) = (
        crate::host::host(),
        (!resource.is_null())
            .then(|| crate::resource::host_resource(resource))
            .flatten(),
    ) else {
        return;
    };
    crate::list::ensure_state(list, resource, 0x800);
    if let Some((buffer, footprint)) = read_back(&host.device, &list.list, texture) {
        snaps.1.push((name.to_owned(), buffer, footprint));
    }
    if target == resource as usize {
        crate::list::ensure_state(list, resource, 0x4);
    }
}

fn save_snaps() {
    let snaps = std::mem::take(&mut *SNAPS.lock().unwrap_or_else(|e| e.into_inner()));
    let Some(folder) = folder() else { return };
    for (i, (name, buffer, footprint)) in snaps.1.iter().enumerate() {
        let f = footprint.Footprint;
        let mut data = std::ptr::null_mut();
        // SAFETY: the copy ran; the mapping spans the footprint's rows.
        unsafe {
            if buffer.Map(0, None, Some(&mut data)).is_err() {
                continue;
            }
            let rows = std::slice::from_raw_parts(
                data.cast::<u8>(),
                f.RowPitch as usize * f.Height as usize,
            );
            let file = format!(
                "raw-{:03}-{}x{}-f{}-p{}.raw",
                900 + i,
                f.Width,
                f.Height,
                f.Format.0,
                f.RowPitch
            );
            let _ = std::fs::write(folder.join(file), rows);
            buffer.Unmap(0, None);
        }
        tracing::info!(i, name, "snapshot saved");
    }
}

/// Notes a draw or dispatch of `list`.
pub(crate) fn pass(list: &crate::host::HostList, target: usize, text: impl FnOnce() -> String) {
    if dump_on()
        && std::env::var_os("VICTORY_DUMP_RAW").is_some()
        && listed(crate::queue::presented() + 1)
    {
        let text = text();
        snap(list, &text, target);
        PASSES
            .lock()
            .unwrap_or_else(|e| e.into_inner())
            .push((std::ptr::from_ref(list) as usize, text));
    }
}

fn dump_on() -> bool {
    static ON: std::sync::OnceLock<bool> = std::sync::OnceLock::new();
    *ON.get_or_init(|| folder().is_some() && std::env::var_os("VICTORY_DUMP_TARGETS").is_some())
}

/// `VICTORY_PRESENT_TARGET=<width>x<height>:<format>[:<n>]`: present the
/// `n`-th (first-use order, default 0) texture of that size and DXGI format
/// the frame renders to or writes, in place of what the title presents, to
/// see a pass the final image does not show (e.g. the G-buffer while lighting
/// is still black). Frames without such a texture present as usual.
fn present_spec() -> Option<(u64, u32, i32, usize)> {
    static SPEC: std::sync::OnceLock<Option<(u64, u32, i32, usize)>> = std::sync::OnceLock::new();
    *SPEC.get_or_init(|| {
        let v = std::env::var("VICTORY_PRESENT_TARGET").ok()?;
        let mut parts = v.split(':');
        let (w, h) = parts.next()?.split_once('x')?;
        let format = parts.next()?.trim().parse().ok()?;
        let n = parts.next().map_or(Some(0), |n| n.trim().parse().ok())?;
        let spec = (w.trim().parse().ok()?, h.trim().parse().ok()?, format, n);
        tracing::info!(
            ?spec,
            "presenting a render target in place of the title's image"
        );
        Some(spec)
    })
}

/// The frame being built and the textures it used that match
/// [`present_spec`], in first-use order.
#[allow(clippy::type_complexity)]
static PRESENTED: std::sync::Mutex<(
    u64,
    Vec<(usize, std::sync::Arc<crate::resource::HostTexture>)>,
)> = std::sync::Mutex::new((0, Vec::new()));

fn note_present(resource: crate::object::This) {
    let Some((w, h, format, _)) = present_spec() else {
        return;
    };
    let Some(copy) = crate::resource::host_copy(resource) else {
        return;
    };
    // SAFETY: a live resource.
    let d = unsafe { copy.texture.GetDesc() };
    if (d.Width, d.Height, d.Format.0) != (w, h, format)
        || d.SampleDesc.Count > 1
        || d.Dimension != D3D12_RESOURCE_DIMENSION_TEXTURE2D
    {
        return;
    }
    let frame = crate::queue::presented() + 1;
    let mut p = PRESENTED.lock().unwrap_or_else(|e| e.into_inner());
    if p.0 != frame {
        *p = (frame, Vec::new());
    }
    if !p.1.iter().any(|&(r, _)| r == resource as usize) {
        p.1.push((resource as usize, copy));
    }
}

/// At the present of `frame`: the texture to show in place of the title's,
/// if [`present_spec`] names one the frame used.
pub(crate) fn present_override(frame: u64) -> Option<std::sync::Arc<crate::resource::HostTexture>> {
    let (.., n) = present_spec()?;
    let mut p = PRESENTED.lock().unwrap_or_else(|e| e.into_inner());
    if p.0 != frame {
        return None;
    }
    let (_, list) = std::mem::take(&mut *p);
    list.into_iter().nth(n).map(|(_, t)| t)
}

/// Notes a title texture the frame being built renders to or writes.
pub(crate) fn note_target(resource: crate::object::This) {
    if !resource.is_null() {
        note_present(resource);
    }
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
    save_snaps();
    let passes = std::mem::take(&mut *PASSES.lock().unwrap_or_else(|e| e.into_inner()));
    if let Some(folder) = folder()
        && !passes.is_empty()
    {
        let text: Vec<String> = passes.iter().map(|(l, t)| format!("{l:x} {t}")).collect();
        // What indirect draws' arguments are now that everything ran.
        let mut args = String::new();
        for (i, (_, t)) in passes.iter().enumerate() {
            if let Some(at) = t
                .strip_prefix("indirect")
                .and_then(|t| t.split_once(" at 0x"))
                .and_then(|(_, t)| t.split_once(' '))
                .and_then(|(t, _)| u64::from_str_radix(t, 16).ok())
            {
                args.push_str(&format!("{i} {at:#x} {}
", crate::shadow::args_text(at)));
            }
        }
        // `VICTORY_PEEK=<program>:<table+slot>:<hex offset>,...`: what a placed
        // buffer a program reads holds.
        for peek in std::env::var("VICTORY_PEEK").unwrap_or_default().split(',') {
            let mut parts = peek.split(':');
            let (Some(program), Some(slot), Some(offset)) =
                (parts.next(), parts.next(), parts.next())
            else {
                continue;
            };
            let offset = u64::from_str_radix(offset, 16).unwrap_or(0);
            // A placed view's address, a constant buffer's or a buffer view's.
            let found = passes
                .iter()
                .find(|(_, t)| t.contains(program))
                .and_then(|(_, t)| t.split_once(&format!("{slot}: ")))
                .and_then(|(_, t)| {
                    if t.starts_with("placed") {
                        let (_, t) = t.split_once(" at 0x")?;
                        u64::from_str_radix(t.split_once('"')?.0, 16).ok()
                    } else if let Some(t) = t.strip_prefix("Constant(0x") {
                        u64::from_str_radix(t.split_once(',')?.0, 16).ok()
                    } else if let Some(t) = t.strip_prefix("buffer Some((") {
                        t.split_once(',')?.0.parse().ok()
                    } else {
                        None
                    }
                });
            // A fourth part, a hex length: that many bytes there, as a file.
            if let (Some(at), Some(len)) =
                (found, parts.next().and_then(|l| usize::from_str_radix(l, 16).ok()))
            {
                let mut bytes = vec![0u8; len.min(1 << 26)];
                if crate::host::read_into((at + offset) as *const u8, &mut bytes) {
                    let _ = std::fs::write(folder.join(format!("peek-{:x}.bin", at + offset)), bytes);
                }
            }
            if let Some(at) = found {
                args.push_str(&format!(
                    "peek {peek} {at:#x}: {}
",
                    crate::shadow::args_text(at + offset)
                ));
            }
        }
        args.push_str(&crate::resource::targets_text());
        let _ = std::fs::write(folder.join("args.txt"), args);
        let _ = std::fs::write(folder.join("passes.txt"), text.join("
"));
    }
    let Some(host) = crate::host::host() else {
        return;
    };
    for (i, (resource, copy)) in targets.iter().enumerate() {
        let texture = &copy.texture;
        // SAFETY: a live resource.
        let d = unsafe { texture.GetDesc() };
        if d.SampleDesc.Count > 1 || d.Dimension != D3D12_RESOURCE_DIMENSION_TEXTURE2D {
            continue;
        }
        // Every submission has run; the dump's list moves it there and back.
        let state = copy.state.load(Relaxed);
        dump_one(
            host,
            texture,
            state,
            frame,
            i,
            *resource as crate::object::This,
            &d,
        );
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
        if state != copy {
            list.list
                .ResourceBarrier(&[crate::host::transition(texture, copy, state)]);
        }
        read
    };
    list.close();
    host.execute(&[&list]);
    let Some((buffer, footprint)) = read else {
        return;
    };
    let f = footprint.Footprint;
    let mut data = std::ptr::null_mut();
    // SAFETY: the copy finished; the mapping spans the footprint's rows.
    unsafe {
        if buffer.Map(0, None, Some(&mut data)).is_err() {
            return;
        }
        let pitch = f.RowPitch as usize;
        let rows = std::slice::from_raw_parts(data.cast::<u8>(), pitch * f.Height as usize);
        if std::env::var_os("VICTORY_DUMP_RAW").is_some()
            && (d.Width >= 1200 || d.Width <= 160)
            && let Some(folder) = folder()
        {
            let name = format!(
                "raw-{i:03}-{}x{}-f{}-p{pitch}.raw",
                f.Width, f.Height, f.Format.0
            );
            let _ = std::fs::write(std::path::Path::new(folder).join(name), rows);
        }
        if let Some(rgba) = to_rgba(
            f.Format.0 as u32,
            f.Width as usize,
            f.Height as usize,
            pitch,
            rows,
        ) {
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

fn half(h: u16) -> f32 {
    let (sign, exp, man) = (
        (h >> 15) as u32,
        ((h >> 10) & 0x1f) as i32,
        (h & 0x3ff) as f32,
    );
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
    let u32_at =
        |b: &[u8], i: usize| u32::from_le_bytes(b[4 * i..4 * i + 4].try_into().unwrap_or([0; 4]));
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
    let wild = values.iter().filter(|v| v.iter().any(|c| !c.is_finite())).count();
    let mut most = high;
    if !grey && matches!(format, 1..=2 | 9..=11 | 26) {
        // What most pixels reach: a few far brighter ones leave the rest black.
        let mut lit: Vec<f32> = values
            .iter()
            .flatten()
            .copied()
            .filter(|v| v.is_finite() && *v > 0.0)
            .step_by(7)
            .collect();
        if !lit.is_empty() {
            let k = lit.len() * 95 / 100;
            most = *lit.select_nth_unstable_by(k, f32::total_cmp).1;
        }
    }
    tracing::info!(format, low, high, most, wild, w, h, "target values");
    let high = most;
    let range = (high - low).max(1e-20);
    for v in values {
        let c = v.map(|c| (((c - low) / range).clamp(0.0, 1.0) * 255.0) as u8);
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
