//! Console primitive types the host lacks. A rectangle list gives three
//! corners per rectangle, the second the one between the other two, and
//! the hardware adds the fourth at `v0 + v2 - v1` (Grand Theft Auto VI's
//! full-screen rectangles come as top left, bottom left, bottom right); on
//! the host each rectangle becomes two triangles, with the fourth vertex
//! made from the other three on the CPU.
//!
//! shortcut: the fourth vertex is computed from vertex inputs, not from
//! vertex shader outputs as the hardware does; that is the same for vertex
//! shaders linear in their inputs (blits, 2D quads). Inputs in formats
//! [`components`] does not know keep the first vertex's value, and so do
//! components past the end of the vertex (an element that reaches into the
//! next one, which the layout allows).

/// Expands `rects` rectangles of `stride`-byte vertices from `src` into six
/// vertices each in `dst`: `(v0, v1, v2)` and `(v0, v2, v3)`, so both
/// triangles turn the same way. `elements` are (byte offset, format).
pub(crate) fn expand_rects(src: &[u8], dst: &mut [u8], stride: usize, elements: &[(u32, u32)]) {
    if stride == 12
        && matches!(
            elements,
            [(0, 16), (8, 28 | 29 | 87 | 91)] | [(8, 28 | 29 | 87 | 91), (0, 16)]
        )
    {
        expand_position_color(src, dst, elements);
    } else {
        expand_rects_generic(src, dst, stride, elements);
    }
}

/// Packed float2 positions and four UNORM8 channels. This layout covers the
/// entire vertex, so the fourth vertex needs no initial copy or format lookup.
fn expand_position_color(src: &[u8], dst: &mut [u8], elements: &[(u32, u32)]) {
    let (rects, _) = src.as_chunks::<36>();
    let (triangles, _) = dst.as_chunks_mut::<72>();
    for (rect, out) in rects.iter().zip(triangles) {
        let (vertices, _) = rect.as_chunks::<12>();
        let [first, middle, last] = *vertices else {
            unreachable!("three vertices per rectangle");
        };
        let mut position = [0.0f32; 2];
        let mut finite = true;
        for (axis, result) in position.iter_mut().enumerate() {
            let read = |vertex: &[u8; 12]| f32::from_le_bytes(vertex.as_chunks::<4>().0[axis]);
            let (a, b, c) = (read(&first), read(&middle), read(&last));
            finite &= a.is_finite() & b.is_finite() & c.is_finite();
            *result = (a + c) - b;
        }
        if !finite {
            // Use the generic calculation for NaNs and infinities, including
            // the choice of NaN payload when more than one operand is a NaN.
            expand_rects_generic(rect, out, 12, elements);
            continue;
        }

        out[..36].copy_from_slice(rect);
        out[36..48].copy_from_slice(&first);
        out[48..60].copy_from_slice(&last);
        out[60..64].copy_from_slice(&position[0].to_le_bytes());
        out[64..68].copy_from_slice(&position[1].to_le_bytes());
        // Normalization and denormalization cancel. The f32 error is far
        // below half a channel step, so the generic rounding gives this same
        // integer except where both paths already clamp to 0 or 255.
        for channel in 8..12 {
            let sum = i16::from(first[channel]) + i16::from(last[channel]);
            out[60 + channel] = (sum - i16::from(middle[channel])).clamp(0, 255) as u8;
        }
    }
}

fn expand_rects_generic(src: &[u8], dst: &mut [u8], stride: usize, elements: &[(u32, u32)]) {
    for (rect, out) in src
        .chunks_exact(3 * stride)
        .zip(dst.chunks_exact_mut(6 * stride))
    {
        let v = |i: usize| &rect[i * stride..(i + 1) * stride];
        for (slot, i) in [0, 1, 2, 0, 2].into_iter().enumerate() {
            out[slot * stride..(slot + 1) * stride].copy_from_slice(v(i));
        }
        let fourth = &mut out[5 * stride..];
        fourth.copy_from_slice(v(0));
        for &(offset, format) in elements {
            let at = offset as usize;
            let Some((n, kind)) = components(format) else {
                continue;
            };
            // Only the components inside the vertex: the rest belong to the
            // next one.
            let inside = stride.saturating_sub(at) / kind.bytes();
            for c in 0..n.min(inside) {
                let get = |i: usize| kind.read(&v(i)[at..], c);
                let value = get(0) + get(2) - get(1);
                kind.write(&mut fourth[at..], c, value);
            }
        }
    }
}

#[derive(Clone, Copy)]
enum Kind {
    F32,
    F16,
    Unorm8,
    Unorm16,
}

/// Component count and kind of the vertex formats rectangles use.
fn components(format: u32) -> Option<(usize, Kind)> {
    Some(match format {
        2 => (4, Kind::F32),
        6 => (3, Kind::F32),
        16 => (2, Kind::F32),
        41 => (1, Kind::F32),
        10 => (4, Kind::F16),
        34 => (2, Kind::F16),
        54 => (1, Kind::F16),
        11 => (4, Kind::Unorm16),
        35 => (2, Kind::Unorm16),
        28 | 29 | 87 | 91 => (4, Kind::Unorm8),
        _ => return None,
    })
}

impl Kind {
    fn bytes(self) -> usize {
        match self {
            Kind::F32 => 4,
            Kind::F16 | Kind::Unorm16 => 2,
            Kind::Unorm8 => 1,
        }
    }

    fn read(self, at: &[u8], c: usize) -> f32 {
        let b = &at[c * self.bytes()..];
        match self {
            Kind::F32 => f32::from_le_bytes([b[0], b[1], b[2], b[3]]),
            Kind::F16 => half_to_f32(u16::from_le_bytes([b[0], b[1]])),
            Kind::Unorm16 => f32::from(u16::from_le_bytes([b[0], b[1]])) / 65535.0,
            Kind::Unorm8 => f32::from(b[0]) / 255.0,
        }
    }

    fn write(self, at: &mut [u8], c: usize, v: f32) {
        let n = self.bytes();
        let b = &mut at[c * n..(c + 1) * n];
        match self {
            Kind::F32 => b.copy_from_slice(&v.to_le_bytes()),
            Kind::F16 => b.copy_from_slice(&f32_to_half(v).to_le_bytes()),
            Kind::Unorm16 => {
                b.copy_from_slice(&((v.clamp(0.0, 1.0) * 65535.0).round() as u16).to_le_bytes())
            }
            Kind::Unorm8 => b[0] = (v.clamp(0.0, 1.0) * 255.0).round() as u8,
        }
    }
}

fn half_to_f32(h: u16) -> f32 {
    let sign = u32::from(h >> 15) << 31;
    let exp = u32::from((h >> 10) & 0x1f);
    let man = u32::from(h & 0x3ff);
    let bits = match (exp, man) {
        (0, 0) => sign,
        // Subnormal: scale the mantissa.
        (0, m) => {
            let v = m as f32 * 2f32.powi(-24);
            return if sign != 0 { -v } else { v };
        }
        (31, m) => sign | 0x7f80_0000 | m << 13,
        (e, m) => sign | (e + 112) << 23 | m << 13,
    };
    f32::from_bits(bits)
}

fn f32_to_half(v: f32) -> u16 {
    let bits = v.to_bits();
    let sign = ((bits >> 16) & 0x8000) as u16;
    let exp = ((bits >> 23) & 0xff) as i32 - 127 + 15;
    let man = bits & 0x7f_ffff;
    if exp >= 31 {
        return sign | 0x7c00; // too large: infinity
    }
    if exp <= 0 {
        if exp < -10 {
            return sign; // below half the smallest subnormal: zero
        }
        // Half subnormals are `m * 2^-24`. An f32 in this range is still
        // normal (exponent field >= 1), so its significand has the implicit
        // leading 1; shift it into the half mantissa, rounding to even.
        let sig = (1u32 << 23) | man;
        let shift = (14 - exp) as u32;
        let halfway = 1u32 << (shift - 1);
        let q = sig >> shift;
        let r = sig & ((1u32 << shift) - 1);
        let m = q + u32::from(r > halfway || (r == halfway && (q & 1) != 0));
        return sign | m as u16;
    }
    // Rounded to nearest: a carry out of the mantissa is added into the
    // exponent (`2 - 2^-12` is 2.0); at most it reaches infinity, 0x7c00.
    let half = (exp as u32) << 10;
    sign | (half + ((man + 0x1000) >> 13)) as u16
}

#[cfg(test)]
mod tests {
    use super::*;

    /// A rectangle of float2 positions and RGBA8 colors, given as top
    /// left, bottom left, bottom right: the fourth corner completes the
    /// parallelogram at the top right.
    #[test]
    fn rectangles_get_their_fourth_corner() {
        let vertex = |x: f32, y: f32, r: u8| {
            let mut v = Vec::new();
            v.extend(x.to_le_bytes());
            v.extend(y.to_le_bytes());
            v.extend([r, 0, 0, 255]);
            v
        };
        let src: Vec<u8> = [
            vertex(-1.0, 1.0, 0),
            vertex(-1.0, -1.0, 100),
            vertex(1.0, -1.0, 150),
        ]
        .concat();
        let mut dst = vec![0u8; 6 * 12];
        expand_rects(&src, &mut dst, 12, &[(0, 16), (8, 28)]);
        assert_eq!(dst[..36], src[..]);
        assert_eq!(dst[36..48], src[..12]);
        assert_eq!(dst[48..60], src[24..36]);
        assert_eq!(dst[60..], vertex(1.0, 1.0, 50)[..]);
        assert_eq!(f32_to_half(1.5), 0x3e00);
        assert_eq!(half_to_f32(0x3e00), 1.5);
        assert_eq!(half_to_f32(0xc000), -2.0);
    }

    /// A rounding that carries out of the mantissa raises the exponent
    /// (the carry used to be or-ed into an odd exponent, halving the value).
    #[test]
    fn halves_round_up_into_the_next_power_of_two() {
        assert_eq!(f32_to_half(2.0 - 2f32.powi(-12)), 0x4000);
        assert_eq!(f32_to_half(0.5 - 2f32.powi(-13)), 0x3800);
        assert_eq!(f32_to_half(65520.0), 0x7c00, "past the largest half");
        // An R16_FLOAT element at 1.0, 2^-12 and 1.0: the fourth corner is
        // 2 - 2^-12, which rounds to 2.0.
        let src: Vec<u8> = [0x3c00_u16, 0x0c00, 0x3c00]
            .iter()
            .flat_map(|h| h.to_le_bytes())
            .collect();
        let mut dst = vec![0u8; 6 * 2];
        expand_rects(&src, &mut dst, 2, &[(0, 54)]);
        assert_eq!(dst[10..], 0x4000_u16.to_le_bytes());
    }

    /// An element that reaches past the stride (float4 at 4 in 8-byte
    /// vertices, RGBA8 at the stride itself): the components inside get
    /// the fourth corner, the rest keep the first vertex's bytes. The old
    /// code indexed past the vertex and panicked.
    #[test]
    fn elements_past_the_stride_are_left_alone() {
        let vertex = |x: f32, y: f32| [x.to_le_bytes(), y.to_le_bytes()].concat();
        let src = [vertex(0.0, 2.0), vertex(1.0, 3.0), vertex(5.0, 7.0)].concat();
        let mut dst = vec![0u8; 6 * 8];
        expand_rects(&src, &mut dst, 8, &[(0, 16), (4, 2), (8, 28)]);
        assert_eq!(dst[..40], [&src[..], &src[..8], &src[16..]].concat()[..]);
        assert_eq!(dst[40..], vertex(4.0, 6.0)[..]);
    }

    #[test]
    fn packed_position_color_matches_generic_expansion() {
        let mut src = packed_rectangles(4096);
        // The first rectangle exercises signed zeros, a subnormal and
        // overflow with finite operands. The second needs the nonfinite path.
        for (vertex, position) in src.as_chunks_mut::<12>().0.iter_mut().zip([
            [-0.0, f32::MAX],
            [0.0, f32::from_bits(1)],
            [-0.0, f32::MAX],
            [f32::from_bits(0x7fc0_0123), f32::INFINITY],
            [1.0, f32::NEG_INFINITY],
            [f32::from_bits(0xffc0_0456), f32::NEG_INFINITY],
        ]) {
            vertex[..4].copy_from_slice(&position[0].to_le_bytes());
            vertex[4..8].copy_from_slice(&position[1].to_le_bytes());
        }
        // Underflow, exact zero, exact 255 and overflow in the same color.
        src[8..12].copy_from_slice(&[0, 0, 255, 255]);
        src[20..24].copy_from_slice(&[1, 255, 1, 0]);
        src[32..36].copy_from_slice(&[0, 255, 1, 1]);

        src.extend_from_slice(&[0xaa; 35]);
        for format in [28, 29, 87, 91] {
            let layout = [(0, 16), (8, format)];
            let reversed = [(8, format), (0, 16)];
            for elements in [layout, reversed] {
                for dst_len in [71, 4095 * 72 + 71, 4097 * 72 + 71] {
                    let mut expected = vec![0xcc; dst_len];
                    let mut actual = expected.clone();
                    expand_rects_generic(&src, &mut expected, 12, &elements);
                    expand_rects(&src, &mut actual, 12, &elements);
                    assert_eq!(actual, expected);
                }
            }
        }
    }

    #[test]
    #[ignore = "release microbenchmark"]
    fn benchmark_position_color_expansion() {
        use std::hint::black_box;
        use std::time::{Duration, Instant};

        let elements = [(0, 16), (8, 28)];
        for count in [1, 64, 1024] {
            let src = packed_rectangles(count);
            let mut before = vec![0; count * 72];
            let mut after = before.clone();
            let iterations = 200_000 / count;
            expand_rects_generic(&src, &mut before, 12, &elements);
            expand_rects(&src, &mut after, 12, &elements);
            assert_eq!(before, after);

            type Expand = fn(&[u8], &mut [u8], usize, &[(u32, u32)]);
            let measure = |expand: Expand, dst: &mut [u8]| {
                let start = Instant::now();
                for _ in 0..iterations {
                    expand(
                        black_box(&src),
                        black_box(dst),
                        black_box(12),
                        black_box(&elements),
                    );
                    black_box(&*dst);
                }
                start.elapsed()
            };
            let mut baseline = [Duration::ZERO; 9];
            let mut optimized = baseline;
            for sample in 0..baseline.len() {
                if sample % 2 == 0 {
                    baseline[sample] = measure(expand_rects_generic, &mut before);
                    optimized[sample] = measure(expand_rects, &mut after);
                } else {
                    optimized[sample] = measure(expand_rects, &mut after);
                    baseline[sample] = measure(expand_rects_generic, &mut before);
                }
            }
            assert_eq!(before, after);
            baseline.sort_unstable();
            optimized.sort_unstable();
            let ns_per_rect =
                |elapsed: Duration| elapsed.as_secs_f64() * 1e9 / (iterations * count) as f64;
            let (base, opt) = (ns_per_rect(baseline[4]), ns_per_rect(optimized[4]));
            println!(
                "{count:4} rectangles: {base:.2} -> {opt:.2} ns/rect ({:.2}x)",
                base / opt
            );
        }
    }

    fn packed_rectangles(count: usize) -> Vec<u8> {
        let mut state = 42u32;
        let mut next = || {
            state ^= state << 13;
            state ^= state >> 17;
            state ^= state << 5;
            state
        };
        let mut src = Vec::with_capacity(count * 36);
        for _ in 0..3 * count {
            for _ in 0..2 {
                src.extend_from_slice(&((next() as i32) as f32 / 65_536.0).to_le_bytes());
            }
            src.extend_from_slice(&next().to_le_bytes());
        }
        src
    }

    /// `half_to_f32` reads subnormals, so an f16 rectangle output must be
    /// able to write them back instead of flushing every one to zero.
    #[test]
    fn half_subnormals_convert_in_both_directions() {
        assert_eq!(f32_to_half(2f32.powi(-24)), 0x0001);
        assert_eq!(f32_to_half(-2f32.powi(-24)), 0x8001);
        assert_eq!(f32_to_half(2f32.powi(-23)), 0x0002);
        assert_eq!(f32_to_half(1023.0 * 2f32.powi(-24)), 0x03ff);
        // Ties go to an even mantissa: 0/1 and 1/2.
        assert_eq!(f32_to_half(2f32.powi(-25)), 0x0000);
        assert_eq!(f32_to_half(3.0 * 2f32.powi(-25)), 0x0002);
        // Below half the smallest subnormal, and normal values, are unchanged.
        assert_eq!(f32_to_half(2f32.powi(-26)), 0x0000);
        assert_eq!(f32_to_half(1.5), 0x3e00);
        assert_eq!(half_to_f32(0x0001), 2f32.powi(-24));
    }
}
