//! A MangoHud-style panel over the presented frame that shows shader
//! translation and pipeline building progress.
//!
//! The text is rasterized with GDI into a small bitmap, written into a
//! per-frame upload buffer and copied onto the back buffer in the same command
//! list as the frame itself. It uses only D3D12 and GDI, so it behaves the same
//! on Windows and under Wine, and it never touches the guest's own image.

mod layout;

use crate::progress::{self, Mode, Snapshot};
use core::ffi::c_void;
use layout::{Corner, PixelFormat};
use std::mem::{ManuallyDrop, size_of};
use std::time::Instant;
use windows::Win32::Foundation::{COLORREF, E_POINTER};
use windows::Win32::Graphics::Direct3D12::*;
use windows::Win32::Graphics::Dxgi::Common::{
    DXGI_FORMAT, DXGI_FORMAT_R8G8B8A8_UNORM, DXGI_FORMAT_R10G10B10A2_UNORM, DXGI_FORMAT_UNKNOWN,
    DXGI_SAMPLE_DESC,
};
use windows::Win32::Graphics::Gdi::*;
use windows::core::{Result, w};

// Colours follow the progress map and MangoHud's dark panel (0xRRGGBB).
const BACKGROUND: u32 = 0x0e0e14;
const BORDER: u32 = 0x3c3c4b;
const TRACK: u32 = 0x272733;
const ACCENT: u32 = 0x6fc7d4;
const TEXT: u32 = 0xd7d7de;
const MUTED: u32 = 0xa0a0b1;
const WARNING: u32 = 0xe2bc8c;
/// Steps in one sweep of the busy bar, and milliseconds per step.
const PERIOD: u32 = 24;
const STEP_MS: u128 = 60;

pub(super) struct Overlay {
    mode: Mode,
    format: PixelFormat,
    dxgi_format: DXGI_FORMAT,
    scale: u32,
    size: (u32, u32),
    origin: Option<(u32, u32)>,
    pixels: Vec<u32>,
    version: u64,
    key: Option<([String; 4], u32)>,
    uploads: [Option<Upload>; 2],
    started: Instant,
}

struct Upload {
    buffer: ID3D12Resource,
    mapped: usize,
    pitch: u32,
    version: u64,
}

/// Where to copy the panel on this frame.
pub(super) struct Placement {
    buffer: ID3D12Resource,
    footprint: D3D12_PLACED_SUBRESOURCE_FOOTPRINT,
    x: u32,
    y: u32,
}

impl Overlay {
    pub(super) fn new(format: DXGI_FORMAT, width: u32, height: u32) -> Self {
        let pixel_format = match format {
            DXGI_FORMAT_R10G10B10A2_UNORM => PixelFormat::Rgb10A2,
            _ => PixelFormat::Rgba8,
        };
        let supported = matches!(format, DXGI_FORMAT_R8G8B8A8_UNORM | DXGI_FORMAT_R10G10B10A2_UNORM);
        let scale = layout::scale_for(height);
        let size = layout::panel_size(scale);
        let corner = Corner::parse(std::env::var("TUXBOX_PROGRESS_OVERLAY_POS").ok().as_deref());
        let origin = layout::origin(corner, size, (width, height), scale);
        let mode = if supported && origin.is_some() { Mode::from_env() } else { Mode::Off };
        Self {
            mode,
            format: pixel_format,
            dxgi_format: format,
            scale,
            size,
            origin,
            pixels: Vec::new(),
            version: 0,
            key: None,
            uploads: [None, None],
            started: Instant::now(),
        }
    }

    /// Draw the panel into frame `index`'s upload buffer when it should be on
    /// screen. Call after waiting for that frame's previous use to retire.
    pub(super) fn prepare(&mut self, device: &ID3D12Device, index: usize) -> Option<Placement> {
        if self.mode == Mode::Off {
            return None;
        }
        let (x, y) = self.origin?;
        let snapshot = progress::snapshot();
        if !progress::visible(self.mode, &snapshot) {
            return None;
        }
        let phase = if snapshot.working() { ((self.started.elapsed().as_millis() / STEP_MS) % u128::from(PERIOD)) as u32 } else { 0 };
        let text = progress::lines(&snapshot);
        let key = (text.clone(), phase);
        if self.key.as_ref() != Some(&key) {
            // SAFETY: GDI objects are created, used and released inside the call.
            match unsafe { rasterize(self.size, self.scale, &text, &snapshot, phase) } {
                Some(pixels) => {
                    self.pixels = pixels;
                    self.version += 1;
                    self.key = Some(key);
                }
                None => return None,
            }
        }
        let pitch = layout::row_pitch(self.size.0);
        let slot = self.uploads.get_mut(index)?;
        if slot.is_none() {
            match create_upload(device, pitch, self.size.1) {
                Ok(upload) => *slot = Some(upload),
                Err(error) => {
                    crate::diagnostics::log(&format!("progress overlay disabled: upload buffer {:?}", error.code()));
                    self.mode = Mode::Off;
                    return None;
                }
            }
        }
        let upload = slot.as_mut()?;
        if upload.version != self.version {
            // SAFETY: the buffer is mapped for its lifetime and holds
            // `pitch * height` bytes. The caller waited for the frame that
            // last read it, so the GPU is not using it.
            unsafe { write_pixels(upload, &self.pixels, self.size, self.format) };
            upload.version = self.version;
        }
        Some(Placement {
            buffer: upload.buffer.clone(),
            footprint: D3D12_PLACED_SUBRESOURCE_FOOTPRINT {
                Offset: 0,
                Footprint: D3D12_SUBRESOURCE_FOOTPRINT {
                    Format: self.dxgi_format,
                    Width: self.size.0,
                    Height: self.size.1,
                    Depth: 1,
                    RowPitch: upload.pitch,
                },
            },
            x,
            y,
        })
    }
}

/// Record the panel copy. The target must be in the COPY_DEST state.
pub(super) fn record(list: &ID3D12GraphicsCommandList, target: &ID3D12Resource, placement: &Placement) {
    let mut source = D3D12_TEXTURE_COPY_LOCATION {
        pResource: ManuallyDrop::new(Some(placement.buffer.clone())),
        Type: D3D12_TEXTURE_COPY_TYPE_PLACED_FOOTPRINT,
        Anonymous: D3D12_TEXTURE_COPY_LOCATION_0 { PlacedFootprint: placement.footprint },
    };
    let mut destination = D3D12_TEXTURE_COPY_LOCATION {
        pResource: ManuallyDrop::new(Some(target.clone())),
        Type: D3D12_TEXTURE_COPY_TYPE_SUBRESOURCE_INDEX,
        Anonymous: D3D12_TEXTURE_COPY_LOCATION_0 { SubresourceIndex: 0 },
    };
    // SAFETY: the list is open, both resources belong to its device, and the
    // copy descriptors are consumed during the call. The cloned references
    // inside the ManuallyDrop fields are released right after.
    unsafe {
        list.CopyTextureRegion(&destination, placement.x, placement.y, 0, &source, None);
        ManuallyDrop::drop(&mut source.pResource);
        ManuallyDrop::drop(&mut destination.pResource);
    }
}

fn create_upload(device: &ID3D12Device, pitch: u32, height: u32) -> Result<Upload> {
    let heap = D3D12_HEAP_PROPERTIES { Type: D3D12_HEAP_TYPE_UPLOAD, ..Default::default() };
    let desc = D3D12_RESOURCE_DESC {
        Dimension: D3D12_RESOURCE_DIMENSION_BUFFER,
        Width: u64::from(pitch) * u64::from(height),
        Height: 1,
        DepthOrArraySize: 1,
        MipLevels: 1,
        Format: DXGI_FORMAT_UNKNOWN,
        SampleDesc: DXGI_SAMPLE_DESC { Count: 1, Quality: 0 },
        Layout: D3D12_TEXTURE_LAYOUT_ROW_MAJOR,
        ..Default::default()
    };
    let mut buffer: Option<ID3D12Resource> = None;
    let mut mapped: *mut c_void = core::ptr::null_mut();
    // SAFETY: the descriptors outlive the calls; Map on an upload buffer
    // returns a CPU pointer that stays valid until the resource is released.
    unsafe {
        device.CreateCommittedResource(
            &heap,
            D3D12_HEAP_FLAG_NONE,
            &desc,
            D3D12_RESOURCE_STATE_GENERIC_READ,
            None,
            &mut buffer,
        )?;
        let buffer = buffer.ok_or(E_POINTER)?;
        buffer.Map(0, None, Some(&mut mapped))?;
        if mapped.is_null() {
            return Err(E_POINTER.into());
        }
        Ok(Upload { buffer, mapped: mapped as usize, pitch, version: 0 })
    }
}

unsafe fn write_pixels(upload: &Upload, pixels: &[u32], size: (u32, u32), format: PixelFormat) {
    let (width, height) = (size.0 as usize, size.1 as usize);
    for row in 0..height {
        let line = (upload.mapped + row * upload.pitch as usize) as *mut u32;
        for column in 0..width {
            let rgb = pixels.get(row * width + column).copied().unwrap_or(0);
            let packed = layout::pack(format, [(rgb >> 16) as u8, (rgb >> 8) as u8, rgb as u8]);
            // SAFETY: row and column are inside the pitch-by-height buffer.
            unsafe { line.add(column).write(packed) };
        }
    }
}

fn colorref(rgb: u32) -> COLORREF {
    // COLORREF is 0x00BBGGRR.
    COLORREF((rgb & 0xff) << 16 | (rgb & 0xff00) | (rgb >> 16 & 0xff))
}

/// Render the panel to 0x00RRGGBB pixels, top row first.
unsafe fn rasterize(size: (u32, u32), scale: u32, text: &[String; 4], snapshot: &Snapshot, phase: u32) -> Option<Vec<u32>> {
    let (width, height) = size;
    let scale = scale as i32;
    // SAFETY: every handle below is created here, checked, and released
    // before returning. The DIB section memory lives until its bitmap is
    // deleted, and is copied out first.
    unsafe {
        let dc = CreateCompatibleDC(None);
        if dc.is_invalid() {
            return None;
        }
        let info = BITMAPINFO {
            bmiHeader: BITMAPINFOHEADER {
                biSize: size_of::<BITMAPINFOHEADER>() as u32,
                biWidth: width as i32,
                biHeight: -(height as i32),
                biPlanes: 1,
                biBitCount: 32,
                biCompression: BI_RGB.0,
                ..Default::default()
            },
            ..Default::default()
        };
        let mut bits: *mut c_void = core::ptr::null_mut();
        let bitmap = match CreateDIBSection(Some(dc), &info, DIB_RGB_COLORS, &mut bits, None, 0) {
            Ok(bitmap) if !bits.is_null() => bitmap,
            _ => {
                let _ = DeleteDC(dc);
                return None;
            }
        };
        let previous = SelectObject(dc, bitmap.into());
        let pixels = core::slice::from_raw_parts_mut(bits.cast::<u32>(), (width * height) as usize);
        let mut fill = |x0: u32, y0: u32, w: u32, h: u32, colour: u32| {
            for y in y0..(y0 + h).min(height) {
                for x in x0..(x0 + w).min(width) {
                    pixels[(y * width + x) as usize] = colour;
                }
            }
        };
        fill(0, 0, width, height, BORDER);
        let border = scale as u32;
        fill(border, border, width - 2 * border, height - 2 * border, BACKGROUND);
        // Progress bar: a block sweeps along the track while work is in flight.
        let pad = 12 * scale as u32;
        let track_width = width - 2 * pad;
        let bar_y = height - 18 * scale as u32;
        let bar_height = 6 * scale as u32;
        fill(pad, bar_y, track_width, bar_height, TRACK);
        if snapshot.working() {
            let (start, end) = layout::marching_block(track_width, phase, PERIOD);
            fill(pad + start, bar_y, end - start, bar_height, ACCENT);
        } else {
            fill(pad, bar_y, track_width, bar_height, MUTED);
        }

        let font = CreateFontW(
            -(14 * scale),
            0,
            0,
            0,
            FW_NORMAL.0 as i32,
            0,
            0,
            0,
            DEFAULT_CHARSET,
            OUT_DEFAULT_PRECIS,
            CLIP_DEFAULT_PRECIS,
            ANTIALIASED_QUALITY,
            (FIXED_PITCH.0 | FF_MODERN.0) as u32,
            w!("Consolas"),
        );
        let old_font = SelectObject(dc, font.into());
        SetBkMode(dc, TRANSPARENT);
        let problems = if snapshot.shaders.timed_out > 0 { WARNING } else { MUTED };
        let colours = [ACCENT, TEXT, problems, MUTED];
        for (index, (line, colour)) in text.iter().zip(colours).enumerate() {
            SetTextColor(dc, colorref(colour));
            let wide: Vec<u16> = line.encode_utf16().collect();
            let _ = TextOutW(dc, pad as i32, 10 * scale + index as i32 * 18 * scale, &wide);
        }
        let _ = GdiFlush();
        let out: Vec<u32> = pixels.iter().map(|p| p & 0x00ff_ffff).collect();
        SelectObject(dc, old_font);
        let _ = DeleteObject(font.into());
        SelectObject(dc, previous);
        let _ = DeleteObject(bitmap.into());
        let _ = DeleteDC(dc);
        Some(out)
    }
}
