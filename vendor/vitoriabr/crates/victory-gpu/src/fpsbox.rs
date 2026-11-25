//! On-screen performance HUD drawn into the top-left corner of every presented
//! frame (VItoriaBr, FPS, frametime, draws/frame, resolution, VRAM, GPU).
//!
//! Drawn on the CPU with a compact 5×7 bitmap font into a small ring of upload
//! buffers copied onto the swap chain's back buffer right before `Present`.
//! The title's own render target is never touched.
//!
//! Environment variable `VICTORY_FPS_COUNTER`:
//! - unset / `1` / `full`: two-line HUD (`VITORIABR | FPS | MS | DRAWS` + `RES | VRAM | GPU`)
//! - `compact`: FPS only (`10.3 FPS`)
//! - `0`: off

use std::sync::{Mutex, OnceLock};
use std::sync::atomic::{AtomicU32, Ordering};

use windows::Win32::Graphics::Direct3D12::*;
use windows::Win32::Graphics::Dxgi::Common::*;

/// Last measured metrics (updated once per second by the presenter's meter).
static TENTHS: AtomicU32 = AtomicU32::new(u32::MAX);
static FRAME_MS: AtomicU32 = AtomicU32::new(0);
static DRAWS: AtomicU32 = AtomicU32::new(0);

#[derive(Clone, Copy, PartialEq, Eq)]
enum Mode {
    Off,
    Compact,
    Full,
}

fn mode() -> Mode {
    static MODE: OnceLock<Mode> = OnceLock::new();
    *MODE.get_or_init(|| match std::env::var("VICTORY_FPS_COUNTER").ok().as_deref() {
        Some("0") => Mode::Off,
        Some("compact") => Mode::Compact,
        _ => Mode::Full,
    })
}

/// Records the latest frame rate, frametime (ms), and draws/frame.
pub(crate) fn set(fps: f64, ms: f64, draws: f64) {
    if fps.is_finite() && fps >= 0.0 {
        TENTHS.store((fps * 10.0).round().min(99_999.0) as u32, Ordering::Relaxed);
    }
    if ms.is_finite() && ms >= 0.0 {
        FRAME_MS.store(ms.round().min(99_999.0) as u32, Ordering::Relaxed);
    }
    if draws.is_finite() && draws >= 0.0 {
        DRAWS.store(draws.round().min(999_999.0) as u32, Ordering::Relaxed);
    }
}

/// A glyph's seven rows, five dots each (bit 4 is the left one).
fn glyph(c: char) -> [u8; 7] {
    match c.to_ascii_uppercase() {
        '0' => [0b01110, 0b10001, 0b10011, 0b10101, 0b11001, 0b10001, 0b01110],
        '1' => [0b00100, 0b01100, 0b00100, 0b00100, 0b00100, 0b00100, 0b01110],
        '2' => [0b01110, 0b10001, 0b00001, 0b00010, 0b00100, 0b01000, 0b11111],
        '3' => [0b11110, 0b00001, 0b00001, 0b01110, 0b00001, 0b00001, 0b11110],
        '4' => [0b00010, 0b00110, 0b01010, 0b10010, 0b11111, 0b00010, 0b00010],
        '5' => [0b11111, 0b10000, 0b11110, 0b00001, 0b00001, 0b10001, 0b01110],
        '6' => [0b00110, 0b01000, 0b10000, 0b11110, 0b10001, 0b10001, 0b01110],
        '7' => [0b11111, 0b00001, 0b00010, 0b00100, 0b01000, 0b01000, 0b01000],
        '8' => [0b01110, 0b10001, 0b10001, 0b01110, 0b10001, 0b10001, 0b01110],
        '9' => [0b01110, 0b10001, 0b10001, 0b01111, 0b00001, 0b00010, 0b01100],
        'A' => [0b01110, 0b10001, 0b10001, 0b11111, 0b10001, 0b10001, 0b10001],
        'B' => [0b11110, 0b10001, 0b10001, 0b11110, 0b10001, 0b10001, 0b11110],
        'C' => [0b01110, 0b10001, 0b10000, 0b10000, 0b10000, 0b10001, 0b01110],
        'D' => [0b11100, 0b10010, 0b10001, 0b10001, 0b10001, 0b10010, 0b11100],
        'E' => [0b11111, 0b10000, 0b10000, 0b11110, 0b10000, 0b10000, 0b11111],
        'F' => [0b11111, 0b10000, 0b10000, 0b11110, 0b10000, 0b10000, 0b10000],
        'G' => [0b01110, 0b10001, 0b10000, 0b10111, 0b10001, 0b10001, 0b01110],
        'H' => [0b10001, 0b10001, 0b10001, 0b11111, 0b10001, 0b10001, 0b10001],
        'I' => [0b01110, 0b00100, 0b00100, 0b00100, 0b00100, 0b00100, 0b01110],
        'J' => [0b00111, 0b00010, 0b00010, 0b00010, 0b10010, 0b10010, 0b01100],
        'K' => [0b10001, 0b10010, 0b10100, 0b11000, 0b10100, 0b10010, 0b10001],
        'L' => [0b10000, 0b10000, 0b10000, 0b10000, 0b10000, 0b10000, 0b11111],
        'M' => [0b10001, 0b11011, 0b10101, 0b10101, 0b10001, 0b10001, 0b10001],
        'N' => [0b10001, 0b11001, 0b10101, 0b10011, 0b10001, 0b10001, 0b10001],
        'O' => [0b01110, 0b10001, 0b10001, 0b10001, 0b10001, 0b10001, 0b01110],
        'P' => [0b11110, 0b10001, 0b10001, 0b11110, 0b10000, 0b10000, 0b10000],
        'Q' => [0b01110, 0b10001, 0b10001, 0b10001, 0b10101, 0b10010, 0b01101],
        'R' => [0b11110, 0b10001, 0b10001, 0b11110, 0b10100, 0b10010, 0b10001],
        'S' => [0b01111, 0b10000, 0b10000, 0b01110, 0b00001, 0b00001, 0b11110],
        'T' => [0b11111, 0b00100, 0b00100, 0b00100, 0b00100, 0b00100, 0b00100],
        'U' => [0b10001, 0b10001, 0b10001, 0b10001, 0b10001, 0b10001, 0b01110],
        'V' => [0b10001, 0b10001, 0b10001, 0b10001, 0b10001, 0b01010, 0b00100],
        'W' => [0b10001, 0b10001, 0b10001, 0b10101, 0b10101, 0b11011, 0b10001],
        'X' => [0b10001, 0b10001, 0b01010, 0b00100, 0b01010, 0b10001, 0b10001],
        'Y' => [0b10001, 0b10001, 0b01010, 0b00100, 0b00100, 0b00100, 0b00100],
        'Z' => [0b11111, 0b00001, 0b00010, 0b00100, 0b01000, 0b10000, 0b11111],
        '.' => [0, 0, 0, 0, 0, 0b01100, 0b01100],
        ':' => [0, 0b00100, 0b00100, 0, 0b00100, 0b00100, 0],
        '-' => [0, 0, 0, 0b11111, 0, 0, 0],
        '/' => [0b00001, 0b00010, 0b00100, 0b00100, 0b01000, 0b10000, 0],
        '|' => [0b00100, 0b00100, 0b00100, 0b00100, 0b00100, 0b00100, 0b00100],
        _ => [0; 7],
    }
}

/// The text for compact mode (FPS only).
fn text(tenths: u32) -> String {
    format!("{}.{} FPS", tenths / 10, tenths % 10)
}

/// Two-line HUD text for full mode.
fn hud_lines(
    tenths: u32,
    ms: u32,
    draws: u32,
    (width, height): (u32, u32),
    vram_mb: Option<u32>,
    gpu: &str,
) -> Vec<String> {
    let line1 = format!(
        "VITORIABR | {}.{} FPS | {} MS | {} DRAWS",
        tenths / 10,
        tenths % 10,
        ms,
        draws
    );
    let mut line2 = format!("{width}X{height}");
    if let Some(mb) = vram_mb {
        let tenths_gb = (u64::from(mb) * 10 + 512) / 1024;
        line2.push_str(&format!(" | VRAM {}.{} GB", tenths_gb / 10, tenths_gb % 10));
    }
    if !gpu.is_empty() {
        let short: String = gpu.chars().take(20).collect();
        line2.push_str(" | ");
        line2.push_str(short.trim());
    }
    vec![line1, line2]
}

/// Bytes a pixel of a swap chain format this layer presents in, or none.
fn pixel_bytes(format: DXGI_FORMAT) -> Option<usize> {
    match format {
        DXGI_FORMAT_R8G8B8A8_UNORM | DXGI_FORMAT_B8G8R8A8_UNORM | DXGI_FORMAT_R10G10B10A2_UNORM => {
            Some(4)
        }
        DXGI_FORMAT_R16G16B16A16_FLOAT => Some(8),
        _ => None,
    }
}

/// One pixel of `format`: of the text (`lit`) or of the box behind it.
fn pixel(format: DXGI_FORMAT, lit: bool, out: &mut [u8]) {
    // Bright yellow text on a nearly black box.
    let (r, g, b): (f32, f32, f32) = if lit { (1.0, 0.92, 0.16) } else { (0.03, 0.03, 0.03) };
    let byte = |v: f32| (v * 255.0).round() as u8;
    match format {
        DXGI_FORMAT_R8G8B8A8_UNORM => out.copy_from_slice(&[byte(r), byte(g), byte(b), 255]),
        DXGI_FORMAT_B8G8R8A8_UNORM => out.copy_from_slice(&[byte(b), byte(g), byte(r), 255]),
        DXGI_FORMAT_R10G10B10A2_UNORM => {
            let ten = |v: f32| (v * 1023.0).round() as u32;
            out.copy_from_slice(&(ten(r) | ten(g) << 10 | ten(b) << 20 | 3 << 30).to_le_bytes());
        }
        _ => {
            for (i, v) in [r, g, b, 1.0].into_iter().enumerate() {
                out[i * 2..i * 2 + 2].copy_from_slice(&half(v).to_le_bytes());
            }
        }
    }
}

/// A value from 0 to 1 as a 16-bit float.
fn half(v: f32) -> u16 {
    let bits = v.clamp(0.0, 1.0).to_bits();
    if v <= 0.0 {
        return 0;
    }
    let exponent = ((bits >> 23) & 0xFF) as i32 - 127 + 15;
    if exponent <= 0 {
        return 0;
    }
    ((exponent as u32) << 10 | (bits >> 13) & 0x3FF) as u16
}

/// Renders one or more lines of text into a bitmap at `scale` dots per pixel.
fn bitmap_lines(lines: &[String], scale: u32) -> (u32, u32, Vec<bool>) {
    let max_chars = lines.iter().map(|l| l.chars().count()).max().unwrap_or(0) as u32;
    if max_chars == 0 || lines.is_empty() {
        return (0, 0, Vec::new());
    }
    let pad = 2 * scale;
    let line_gap = 3 * scale;
    let line_h = 7 * scale;
    let width = 2 * pad + max_chars * 6 * scale - scale;
    let height = 2 * pad + lines.len() as u32 * line_h + (lines.len() as u32 - 1) * line_gap;
    let mut lit = vec![false; (width * height) as usize];
    for (line_idx, line) in lines.iter().enumerate() {
        let base_y = pad + line_idx as u32 * (line_h + line_gap);
        for (i, c) in line.chars().enumerate() {
            let rows = glyph(c);
            for (row, bits) in rows.into_iter().enumerate() {
                for column in 0..5u32 {
                    if bits >> (4 - column) & 1 == 0 {
                        continue;
                    }
                    let (x0, y0) = (pad + (i as u32 * 6 + column) * scale, base_y + row as u32 * scale);
                    for y in y0..y0 + scale {
                        for x in x0..x0 + scale {
                            lit[(y * width + x) as usize] = true;
                        }
                    }
                }
            }
        }
    }
    (width, height, lit)
}

/// A buffer the card copies the HUD box from, with its footprint.
struct Made {
    buffer: ID3D12Resource,
    width: u32,
    height: u32,
    pitch: u32,
}

// SAFETY: a free-threaded D3D12 object and plain numbers.
unsafe impl Send for Made {}

/// Small ring of HUD buffers so live updates never leak upload memory while
/// keeping in-flight presents safe.
const RING: usize = 4;

struct HudCache {
    last_text: String,
    last_fmt: i32,
    last_scale: u32,
    slot: usize,
    ring: [Option<Made>; RING],
}

static CACHE: Mutex<HudCache> = Mutex::new(HudCache {
    last_text: String::new(),
    last_fmt: 0,
    last_scale: 0,
    slot: 0,
    ring: [const { None }; RING],
});

/// Copies the performance HUD onto `back`, a back buffer of `width` by `height`
/// pixels of `format` that `list` holds as a copy's destination.
pub(crate) fn draw(
    host: &crate::host::Host,
    list: &ID3D12GraphicsCommandList,
    back: &ID3D12Resource,
    (width, height, format): (u32, u32, DXGI_FORMAT),
) {
    let mode = mode();
    if mode == Mode::Off {
        return;
    }
    let tenths = TENTHS.load(Ordering::Relaxed);
    let Some(bytes) = pixel_bytes(format).filter(|_| tenths != u32::MAX) else {
        return;
    };
    static GPU: OnceLock<String> = OnceLock::new();
    let lines = match mode {
        Mode::Off => return,
        Mode::Compact => vec![text(tenths)],
        Mode::Full => {
            let gpu = GPU.get_or_init(|| host.gpu_label());
            let ms = FRAME_MS.load(Ordering::Relaxed);
            let draws = DRAWS.load(Ordering::Relaxed);
            let vram = host.vram_used_mb();
            hud_lines(tenths, ms, draws, (width, height), vram, gpu)
        }
    };
    let joined = lines.join("\n");
    // Compact scale for 2-line HUD so it stays crisp and unobtrusive.
    let scale = match mode {
        Mode::Full => (height / 540).clamp(2, 8),
        _ => (height / 360).clamp(2, 12),
    };
    let mut cache = CACHE.lock().unwrap_or_else(|e| e.into_inner());
    let needs_rebuild = cache.ring[cache.slot].is_none()
        || cache.last_text != joined
        || cache.last_fmt != format.0
        || cache.last_scale != scale;
    if needs_rebuild {
        let (w, h, lit) = bitmap_lines(&lines, scale);
        if w == 0 || h == 0 {
            return;
        }
        let pitch = (w as usize * bytes).next_multiple_of(D3D12_TEXTURE_DATA_PITCH_ALIGNMENT as usize);
        let Some((buffer, data)) = host.shared_buffer((pitch * h as usize) as u64) else {
            return;
        };
        // SAFETY: the buffer's own mapped bytes, `pitch * h` of them.
        let out = unsafe { std::slice::from_raw_parts_mut(data, pitch * h as usize) };
        for y in 0..h as usize {
            for x in 0..w as usize {
                let at = y * pitch + x * bytes;
                pixel(format, lit[y * w as usize + x], &mut out[at..at + bytes]);
            }
        }
        let next = (cache.slot + 1) % RING;
        if let Some(old) = cache.ring[next].take() {
            crate::host::retire(old.buffer);
        }
        cache.ring[next] = Some(Made {
            buffer,
            width: w,
            height: h,
            pitch: pitch as u32,
        });
        cache.slot = next;
        cache.last_text = joined;
        cache.last_fmt = format.0;
        cache.last_scale = scale;
    }
    let Some(m) = cache.ring[cache.slot].as_ref() else {
        return;
    };
    let margin = 4 * scale;
    if m.width + 2 * margin > width || m.height + 2 * margin > height {
        return;
    }
    // SAFETY: an open list; the back buffer is a copy's destination in it and
    // the box lies inside it; our buffer stays alive via ring + retire.
    unsafe {
        let from = D3D12_TEXTURE_COPY_LOCATION {
            pResource: std::mem::transmute_copy(&m.buffer),
            Type: D3D12_TEXTURE_COPY_TYPE_PLACED_FOOTPRINT,
            Anonymous: D3D12_TEXTURE_COPY_LOCATION_0 {
                PlacedFootprint: D3D12_PLACED_SUBRESOURCE_FOOTPRINT {
                    Offset: 0,
                    Footprint: D3D12_SUBRESOURCE_FOOTPRINT {
                        Format: format,
                        Width: m.width,
                        Height: m.height,
                        Depth: 1,
                        RowPitch: m.pitch,
                    },
                },
            },
        };
        let to = D3D12_TEXTURE_COPY_LOCATION {
            pResource: std::mem::transmute_copy(back),
            Type: D3D12_TEXTURE_COPY_TYPE_SUBRESOURCE_INDEX,
            Anonymous: D3D12_TEXTURE_COPY_LOCATION_0 { SubresourceIndex: 0 },
        };
        list.CopyTextureRegion(&to, margin, margin, 0, &from, None);
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn the_text_is_the_rate_to_a_tenth() {
        assert_eq!(text(103), "10.3 FPS");
        assert_eq!(text(7), "0.7 FPS");
        assert_eq!(text(300), "30.0 FPS");
    }

    #[test]
    fn hud_lines_format_metrics_and_gpu() {
        let lines = hud_lines(48, 208, 3916, (1920, 1080), Some(11_674), "RTX 5060 TI");
        assert_eq!(lines[0], "VITORIABR | 4.8 FPS | 208 MS | 3916 DRAWS");
        assert_eq!(lines[1], "1920X1080 | VRAM 11.4 GB | RTX 5060 TI");
    }

    #[test]
    fn the_box_holds_every_glyph_with_a_margin() {
        let lines = vec!["10.3 FPS".to_owned()];
        let (w, h, lit) = bitmap_lines(&lines, 3);
        assert_eq!((w, h), ((8 * 5 + 7 + 4) * 3, (7 + 4) * 3));
        assert_eq!(lit.len(), (w * h) as usize);
        assert!(lit[..(w * 6) as usize].iter().all(|&l| !l), "the top pad");
        assert!(
            (0..h).all(|y| !lit[(y * w) as usize] && !lit[(y * w + w - 1) as usize]),
            "the side pads"
        );
        for c in "0123456789ABCDEFGHIJKLMNOPQRSTUVWXYZ.:-/|".chars() {
            assert!(glyph(c).iter().any(|&r| r != 0), "{c}");
        }
    }

    #[test]
    fn pixels_are_in_the_formats_the_layer_presents_in() {
        let mut four = [0u8; 4];
        pixel(DXGI_FORMAT_R8G8B8A8_UNORM, true, &mut four);
        assert_eq!(four, [255, 235, 41, 255]);
        pixel(DXGI_FORMAT_B8G8R8A8_UNORM, true, &mut four);
        assert_eq!(four, [41, 235, 255, 255]);
        pixel(DXGI_FORMAT_R10G10B10A2_UNORM, true, &mut four);
        let v = u32::from_le_bytes(four);
        assert_eq!((v & 1023, v >> 10 & 1023, v >> 20 & 1023, v >> 30), (1023, 941, 164, 3));
        let mut eight = [0u8; 8];
        pixel(DXGI_FORMAT_R16G16B16A16_FLOAT, true, &mut eight);
        assert_eq!(u16::from_le_bytes([eight[0], eight[1]]), 0x3C00, "1.0 as a half");
        assert_eq!(u16::from_le_bytes([eight[6], eight[7]]), 0x3C00);
        assert_eq!(half(0.5), 0x3800);
        assert_eq!(half(0.0), 0);
        assert_eq!(pixel_bytes(DXGI_FORMAT_R16G16B16A16_FLOAT), Some(8));
        assert_eq!(pixel_bytes(DXGI_FORMAT_D32_FLOAT), None);
    }
}
