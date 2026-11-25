//! (Neptune) F12 screenshots: the overlay thread calls [`request`]; the next present records a
//! readback of the presented texture ([`read_back`]), and [`finish`] converts it to 8 bits and
//! hands it to a thread that writes `%USERPROFILE%\Pictures\Neptune\neptune-<date>_<time>.png`.
//! The overlay is a window of its own, so the worker draws the same panel ([`crate::overlay::render`])
//! onto the picture, from the stats current at capture time.

use std::sync::atomic::{AtomicBool, Ordering};

use windows::Win32::Graphics::Direct3D12::*;

static WANTED: AtomicBool = AtomicBool::new(false);

pub(crate) fn request() {
    WANTED.store(true, Ordering::Relaxed);
}

/// Whether a screenshot was asked for (and clears the ask).
pub(crate) fn take() -> bool {
    WANTED.swap(false, Ordering::Relaxed)
}

pub(crate) struct Shot {
    buffer: ID3D12Resource,
    footprint: D3D12_PLACED_SUBRESOURCE_FOOTPRINT,
}

/// Records a copy of `texture` (in the copy-source state) into a readback buffer.
pub(crate) fn read_back(device: &ID3D12Device, list: &ID3D12GraphicsCommandList, texture: &ID3D12Resource) -> Option<Shot> {
    let mut footprint = D3D12_PLACED_SUBRESOURCE_FOOTPRINT::default();
    let mut total = 0;
    let heap = D3D12_HEAP_PROPERTIES { Type: D3D12_HEAP_TYPE_READBACK, ..Default::default() };
    let mut buffer: Option<ID3D12Resource> = None;
    // SAFETY: a live texture and an open list; one subresource.
    unsafe {
        let desc = texture.GetDesc();
        device.GetCopyableFootprints(&desc, 0, 1, 0, Some(&mut footprint), None, None, Some(&mut total));
        let desc = D3D12_RESOURCE_DESC {
            Dimension: D3D12_RESOURCE_DIMENSION_BUFFER,
            Width: total,
            Height: 1,
            DepthOrArraySize: 1,
            MipLevels: 1,
            SampleDesc: windows::Win32::Graphics::Dxgi::Common::DXGI_SAMPLE_DESC { Count: 1, Quality: 0 },
            Layout: D3D12_TEXTURE_LAYOUT_ROW_MAJOR,
            ..Default::default()
        };
        device
            .CreateCommittedResource(&heap, D3D12_HEAP_FLAG_NONE, &desc, D3D12_RESOURCE_STATE_COPY_DEST, None, &mut buffer)
            .ok()?;
        let buffer = buffer?;
        let to = D3D12_TEXTURE_COPY_LOCATION {
            pResource: std::mem::transmute_copy(&buffer),
            Type: D3D12_TEXTURE_COPY_TYPE_PLACED_FOOTPRINT,
            Anonymous: D3D12_TEXTURE_COPY_LOCATION_0 { PlacedFootprint: footprint },
        };
        let from = D3D12_TEXTURE_COPY_LOCATION {
            pResource: std::mem::transmute_copy(texture),
            Type: D3D12_TEXTURE_COPY_TYPE_SUBRESOURCE_INDEX,
            Anonymous: D3D12_TEXTURE_COPY_LOCATION_0 { SubresourceIndex: 0 },
        };
        list.CopyTextureRegion(&to, 0, 0, 0, &from, None);
        Some(Shot { buffer, footprint })
    }
}

/// Converts the finished readback to RGB and saves it on a thread of its own (the present thread
/// only pays for the copy out of the mapping).
pub(crate) fn finish(shot: Shot) {
    let f = shot.footprint.Footprint;
    let (w, h, pitch) = (f.Width as usize, f.Height as usize, f.RowPitch as usize);
    let mut data = std::ptr::null_mut();
    // SAFETY: the copy finished (`Host::execute` waits); the mapping spans the footprint's rows.
    let rgb = unsafe {
        if shot.buffer.Map(0, None, Some(&mut data)).is_err() {
            return;
        }
        let rows = std::slice::from_raw_parts(data.cast::<u8>(), pitch * h);
        let rgb = to_rgb(f.Format.0 as u32, w, h, pitch, rows);
        shot.buffer.Unmap(0, None);
        rgb
    };
    let Some(rgb) = rgb else {
        tracing::warn!(format = f.Format.0, "screenshot not saved: format not handled");
        return;
    };
    let stats = crate::overlay::snapshot();
    let _ = std::thread::Builder::new().name("victory-screenshot".into()).spawn(move || {
        let mut rgb = rgb;
        if let Some(panel) = stats.and_then(|s| crate::overlay::render(&s, h)) {
            let m = crate::overlay::margin(h);
            crate::pixels::blend_onto_rgb(&mut rgb, w, h, &panel, m, m);
        }
        let png = encode_png(w as u32, h as u32, &rgb);
        match save(&png) {
            Ok(path) => tracing::info!(path = %path.display(), "screenshot saved"),
            Err(e) => tracing::warn!(error = %e, "screenshot not saved"),
        }
    });
}

fn save(png: &[u8]) -> std::io::Result<std::path::PathBuf> {
    use windows_sys::Win32::System::SystemInformation::GetLocalTime;
    let home = std::env::var_os("USERPROFILE").ok_or_else(|| std::io::Error::other("no USERPROFILE"))?;
    let folder = std::path::Path::new(&home).join("Pictures").join("Neptune");
    std::fs::create_dir_all(&folder)?;
    // SAFETY: a plain struct filled by the call.
    let t = unsafe {
        let mut t = std::mem::zeroed();
        GetLocalTime(&mut t);
        t
    };
    let stem = format!("neptune-{:04}-{:02}-{:02}_{:02}-{:02}-{:02}", t.wYear, t.wMonth, t.wDay, t.wHour, t.wMinute, t.wSecond);
    let mut path = folder.join(format!("{stem}.png"));
    for n in 2.. {
        if !path.exists() {
            break;
        }
        path = folder.join(format!("{stem}-{n}.png"));
    }
    std::fs::write(&path, png)?;
    Ok(path)
}

/// 8-bit RGB rows of a readback in DXGI format `format`; `None` for a format not handled.
/// Float formats are clamped to 0..1 (no tone mapping).
fn to_rgb(format: u32, w: usize, h: usize, pitch: usize, rows: &[u8]) -> Option<Vec<u8>> {
    let mut out = Vec::with_capacity(w * h * 3);
    let half = |b: &[u8]| {
        let v = f32_from_half(u16::from_le_bytes([b[0], b[1]]));
        (if v.is_nan() { 0.0 } else { v.clamp(0.0, 1.0) } * 255.0 + 0.5) as u8
    };
    for y in 0..h {
        let row = rows.get(y * pitch..)?;
        for x in 0..w {
            match format {
                // R8G8B8A8 typeless, unorm, srgb
                27..=29 => out.extend_from_slice(row.get(x * 4..x * 4 + 3)?),
                // B8G8R8A8 / B8G8R8X8 typeless, unorm, srgb
                87..=88 | 90..=93 => {
                    let p = row.get(x * 4..x * 4 + 3)?;
                    out.extend([p[2], p[1], p[0]]);
                }
                // R10G10B10A2 typeless, unorm
                23 | 24 => {
                    let v = u32::from_le_bytes(row.get(x * 4..x * 4 + 4)?.try_into().ok()?);
                    out.extend([0, 10, 20].map(|s| ((v >> s & 0x3ff) * 255 + 511) / 1023).map(|c| c as u8));
                }
                // R16G16B16A16 typeless, float
                9 | 10 => {
                    let p = row.get(x * 8..x * 8 + 6)?;
                    out.extend([half(&p[0..2]), half(&p[2..4]), half(&p[4..6])]);
                }
                _ => return None,
            }
        }
    }
    Some(out)
}

fn f32_from_half(h: u16) -> f32 {
    let (sign, exp, man) = (u32::from(h >> 15), u32::from(h >> 10 & 0x1f), u32::from(h & 0x3ff));
    let magnitude = match exp {
        0 => man as f32 * 2f32.powi(-24),
        31 => if man == 0 { f32::INFINITY } else { f32::NAN },
        _ => (1.0 + man as f32 / 1024.0) * 2f32.powi(exp as i32 - 15),
    };
    if sign == 1 { -magnitude } else { magnitude }
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

fn chunk(png: &mut Vec<u8>, kind: &[u8; 4], data: &[u8]) {
    png.extend((data.len() as u32).to_be_bytes());
    let start = png.len();
    png.extend(kind);
    png.extend(data);
    let crc = crc32(&png[start..]);
    png.extend(crc.to_be_bytes());
}

/// A PNG of 8-bit RGB `rgb` (`w * h * 3` bytes): per-row filters, deflated by zlib-rs at level 6.
fn encode_png(w: u32, h: u32, rgb: &[u8]) -> Vec<u8> {
    let raw = crate::pixels::filter_image(rgb, w as usize * 3, 3);
    let mut png = b"\x89PNG\r\n\x1a\n".to_vec();
    let mut header = Vec::new();
    header.extend(w.to_be_bytes());
    header.extend(h.to_be_bytes());
    header.extend([8, 2, 0, 0, 0]);
    chunk(&mut png, b"IHDR", &header);
    let mut packed = vec![0u8; zlib_rs::compress_bound(raw.len())];
    let (idat, rc) = zlib_rs::compress_slice(&mut packed, &raw, zlib_rs::DeflateConfig::new(6));
    debug_assert_eq!(rc, zlib_rs::ReturnCode::Ok);
    chunk(&mut png, b"IDAT", idat);
    chunk(&mut png, b"IEND", &[]);
    png
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn crc_of_the_check_string() {
        assert_eq!(crc32(b"123456789"), 0xcbf4_3926);
    }

    #[test]
    fn png_structure() {
        let png = encode_png(2, 2, &[7; 12]);
        assert_eq!(&png[..8], b"\x89PNG\r\n\x1a\n");
        assert_eq!(&png[8..16], b"\0\0\0\x0dIHDR");
        assert_eq!(&png[16..24], b"\0\0\0\x02\0\0\0\x02");
        assert_eq!(&png[png.len() - 12..], b"\0\0\0\0IEND\xae\x42\x60\x82");
        // IHDR's crc covers its type and data.
        assert_eq!(u32::from_be_bytes(png[29..33].try_into().unwrap()), crc32(&png[12..29]));
    }

    #[test]
    fn pixel_formats() {
        // B8G8R8A8: swapped to RGB.
        assert_eq!(to_rgb(87, 1, 1, 4, &[1, 2, 3, 255]).unwrap(), [3, 2, 1]);
        // R10G10B10A2: full red, no green, half blue.
        let v: u32 = 0x3ff | 512 << 20;
        assert_eq!(to_rgb(24, 1, 1, 4, &v.to_le_bytes()).unwrap(), [255, 0, 128]);
        // R16G16B16A16_FLOAT: 1.0, 2.0 (clamped), -1.0 (clamped).
        let px: Vec<u8> = [0x3c00u16, 0x4000, 0xbc00, 0x3c00].iter().flat_map(|v| v.to_le_bytes()).collect();
        assert_eq!(to_rgb(10, 1, 1, 8, &px).unwrap(), [255, 255, 0]);
        assert!(to_rgb(2, 1, 1, 16, &[0; 16]).is_none());
    }
}
