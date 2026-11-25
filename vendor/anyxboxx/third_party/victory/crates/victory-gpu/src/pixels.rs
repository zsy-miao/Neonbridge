//! (Neptune) Pixel maths with no OS calls: the overlay's canvas (premultiplied RGBA), compositing
//! it onto a screenshot, PNG row filters, and hardware-name shortening.

/// A premultiplied RGBA bitmap, top-down.
pub(crate) struct Bitmap {
    pub w: usize,
    pub h: usize,
    pub px: Vec<u8>,
}

impl Bitmap {
    pub fn new(w: usize, h: usize) -> Self {
        Bitmap { w, h, px: vec![0; w * h * 4] }
    }

    /// Lays `rgb` (straight colour) over pixel (x, y) with opacity `a` (0..=255).
    pub fn over(&mut self, x: usize, y: usize, rgb: [u8; 3], a: u32) {
        if x >= self.w || y >= self.h || a == 0 {
            return;
        }
        let p = &mut self.px[(y * self.w + x) * 4..][..4];
        let keep = 255 - a;
        for i in 0..3 {
            p[i] = ((u32::from(rgb[i]) * a + u32::from(p[i]) * keep + 127) / 255) as u8;
        }
        p[3] = (a + (u32::from(p[3]) * keep + 127) / 255) as u8;
    }

    /// A rounded rectangle with anti-aliased edges, `rgb` at opacity `a`.
    pub fn fill_round(&mut self, (x0, y0, w, h): (usize, usize, usize, usize), radius: f32, rgb: [u8; 3], a: u32) {
        for y in 0..h {
            for x in 0..w {
                let c = round_coverage(x as f32 + 0.5, y as f32 + 0.5, w as f32, h as f32, radius);
                self.over(x0 + x, y0 + y, rgb, (c * a as f32 + 0.5) as u32);
            }
        }
    }
}

/// Coverage (0..1) of pixel centre (x, y) by a `w` x `h` rectangle with corner radius `r`.
pub(crate) fn round_coverage(x: f32, y: f32, w: f32, h: f32, r: f32) -> f32 {
    let (qx, qy) = ((x - w / 2.0).abs() - (w / 2.0 - r), (y - h / 2.0).abs() - (h / 2.0 - r));
    let outside = (qx.max(0.0).powi(2) + qy.max(0.0).powi(2)).sqrt();
    let d = outside + qx.max(qy).min(0.0) - r;
    (0.5 - d).clamp(0.0, 1.0)
}

/// Blends the premultiplied `bm` onto 8-bit RGB rows `rgb` (`w` x `h`) with its corner at (x0, y0).
pub(crate) fn blend_onto_rgb(rgb: &mut [u8], w: usize, h: usize, bm: &Bitmap, x0: usize, y0: usize) {
    for y in 0..bm.h.min(h.saturating_sub(y0)) {
        for x in 0..bm.w.min(w.saturating_sub(x0)) {
            let s = &bm.px[(y * bm.w + x) * 4..][..4];
            let d = &mut rgb[((y0 + y) * w + x0 + x) * 3..][..3];
            for i in 0..3 {
                d[i] = (u32::from(s[i]) + (u32::from(d[i]) * (255 - u32::from(s[3])) + 127) / 255).min(255) as u8;
            }
        }
    }
}

/// PNG filter `kind` (0 none, 1 sub, 2 up, 3 average, 4 paeth) of `row` given the row above.
pub(crate) fn filter_row(kind: u8, row: &[u8], prev: &[u8], bpp: usize, out: &mut Vec<u8>) {
    out.clear();
    out.push(kind);
    for i in 0..row.len() {
        let a = if i >= bpp { row[i - bpp] } else { 0 };
        let b = prev[i];
        let c = if i >= bpp { prev[i - bpp] } else { 0 };
        let pred = match kind {
            1 => a,
            2 => b,
            3 => ((u16::from(a) + u16::from(b)) / 2) as u8,
            4 => paeth(a, b, c),
            _ => 0,
        };
        out.push(row[i].wrapping_sub(pred));
    }
}

fn paeth(a: u8, b: u8, c: u8) -> u8 {
    let (ia, ib, ic) = (i16::from(a), i16::from(b), i16::from(c));
    let p = ia + ib - ic;
    let (pa, pb, pc) = ((p - ia).abs(), (p - ib).abs(), (p - ic).abs());
    if pa <= pb && pa <= pc { a } else if pb <= pc { b } else { c }
}

/// The filtered scanlines of 8-bit pixels, each row filtered the way that leaves the smallest sum
/// of absolute residuals (the usual heuristic).
pub(crate) fn filter_image(px: &[u8], stride: usize, bpp: usize) -> Vec<u8> {
    let zero = vec![0u8; stride];
    let mut raw = Vec::with_capacity((stride + 1) * px.len().div_ceil(stride.max(1)));
    let (mut best, mut cand) = (Vec::new(), Vec::new());
    let mut prev: &[u8] = &zero;
    for row in px.chunks(stride) {
        let mut best_cost = u64::MAX;
        for kind in [0, 1, 2, 4, 3] {
            filter_row(kind, row, prev, bpp, &mut cand);
            let cost: u64 = cand[1..].iter().map(|&v| u64::from((v as i8).unsigned_abs())).sum();
            if cost < best_cost {
                best_cost = cost;
                std::mem::swap(&mut best, &mut cand);
            }
        }
        raw.extend_from_slice(&best);
        prev = row;
    }
    raw
}

/// "AMD Ryzen 7 9800X3D 8-Core Processor" -> "Ryzen 7 9800X3D".
pub(crate) fn short_cpu(name: &str) -> String {
    let cleaned = name.replace("(R)", "").replace("(TM)", "").replace("(tm)", "");
    let cleaned = cleaned.split('@').next().unwrap_or("").trim();
    let words: Vec<&str> = cleaned
        .split_whitespace()
        .filter(|w| !["AMD", "Intel", "CPU", "Processor"].contains(w) && !w.ends_with("-Core"))
        .collect();
    if words.is_empty() { name.trim().to_string() } else { words.join(" ") }
}

/// "NVIDIA GeForce RTX 5070 Ti" -> "RTX 5070 Ti".
pub(crate) fn short_gpu(name: &str) -> String {
    let words: Vec<&str> = name
        .split_whitespace()
        .filter(|w| !["NVIDIA", "GeForce", "AMD", "(TM)", "(R)"].contains(w))
        .collect();
    if words.is_empty() { name.trim().to_string() } else { words.join(" ") }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn unfilter(kind: u8, row: &[u8], prev: &[u8], bpp: usize) -> Vec<u8> {
        let mut out: Vec<u8> = Vec::new();
        for i in 0..row.len() {
            let a = if i >= bpp { out[i - bpp] } else { 0 };
            let c = if i >= bpp { prev[i - bpp] } else { 0 };
            let pred = match kind {
                1 => a,
                2 => prev[i],
                3 => ((u16::from(a) + u16::from(prev[i])) / 2) as u8,
                4 => paeth(a, prev[i], c),
                _ => 0,
            };
            out.push(row[i].wrapping_add(pred));
        }
        out
    }

    #[test]
    fn every_filter_round_trips() {
        let prev: Vec<u8> = (0..30).map(|i| (i * 37 % 251) as u8).collect();
        let row: Vec<u8> = (0..30).map(|i| (i * 91 % 253) as u8).collect();
        let mut out = Vec::new();
        for kind in 0..=4 {
            filter_row(kind, &row, &prev, 3, &mut out);
            assert_eq!(out[0], kind);
            assert_eq!(unfilter(kind, &out[1..], &prev, 3), row, "filter {kind}");
        }
    }

    #[test]
    fn smooth_rows_pick_a_predicting_filter() {
        let px: Vec<u8> = (0..60u32).map(|i| (i / 3) as u8).collect(); // 2 rows of 10 px, a ramp
        let raw = filter_image(&px, 30, 3);
        assert_eq!(raw.len(), 62);
        assert_ne!(raw[0], 0);
    }

    #[test]
    fn over_and_blend() {
        let mut bm = Bitmap::new(2, 1);
        bm.over(0, 0, [255, 255, 255], 255);
        bm.over(1, 0, [0, 0, 0], 128);
        assert_eq!(&bm.px[..4], [255, 255, 255, 255]);
        assert_eq!(&bm.px[4..], [0, 0, 0, 128]);
        let mut rgb = [100u8; 6];
        blend_onto_rgb(&mut rgb, 2, 1, &bm, 0, 0);
        assert_eq!(&rgb[..3], [255, 255, 255]);
        assert_eq!(&rgb[3..], [50, 50, 50]);
        // Out-of-range placement is clipped, not a panic.
        blend_onto_rgb(&mut rgb, 2, 1, &bm, 1, 0);
        blend_onto_rgb(&mut rgb, 2, 1, &bm, 5, 5);
    }

    #[test]
    fn rounded_corners() {
        assert_eq!(round_coverage(50.0, 20.0, 100.0, 40.0, 8.0), 1.0);
        assert_eq!(round_coverage(0.5, 0.5, 100.0, 40.0, 8.0), 0.0);
        assert_eq!(round_coverage(50.0, 0.5, 100.0, 40.0, 8.0), 1.0);
    }

    #[test]
    fn names_are_shortened() {
        assert_eq!(short_cpu("AMD Ryzen 7 9800X3D 8-Core Processor"), "Ryzen 7 9800X3D");
        assert_eq!(short_cpu("Intel(R) Core(TM) i9-14900K @ 3.20GHz"), "Core i9-14900K");
        assert_eq!(short_gpu("NVIDIA GeForce RTX 5070 Ti"), "RTX 5070 Ti");
        assert_eq!(short_gpu("AMD Radeon RX 7900 XTX"), "Radeon RX 7900 XTX");
    }
}
