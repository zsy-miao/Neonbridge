//! The console GPU's ray-tracing node test, `image_bvh_intersect_ray` and
//! `image_bvh64_intersect_ray`, on the CPU: the node formats, and what the
//! instruction returns for one ray and one node. The emitter lowers the
//! instruction to the same arithmetic in the same order
//! (`emit/intersect.rs`), so its code can be compared against this dword
//! for dword.
//!
//! The instruction tests one ray against one 64-byte node of a bounding
//! volume hierarchy and returns four dwords. Traversal (the stack, instance
//! transforms, culling, the hit distance `numerator / denominator`) is the
//! program's own code.
//!
//! # Node pointers
//!
//! A node pointer's low three bits are the node's kind, the rest its byte
//! offset in 8-byte units: the node is at `(pointer >> 3) << 6` past the
//! base address of the descriptor (40 bits in dword 0 and the low byte of
//! dword 1, in 256-byte units). The 32-bit form takes a 32-bit pointer;
//! the 64-bit form a 64-bit one, with which a program uses one descriptor
//! with base 0 for every structure and adds `address >> 3` itself.
//!
//! | kind | node |
//! |---|---|
//! | 0-3 | triangle 0-3 of a triangle node |
//! | 4 | box node, 16-bit floats (64 bytes) |
//! | 5 | box node, 32-bit floats (128 bytes) |
//! | 6 | instance: not read by the instruction |
//! | 7 | procedural primitive: not read by the instruction |
//!
//! # Box nodes
//!
//! Four child pointers at bytes 0-15 (`0xffff_ffff`: none), then four
//! boxes, each minimum x y z, maximum x y z: 16-bit floats from byte 16
//! (12 bytes a box), or 32-bit floats from byte 16 (24 bytes a box; bytes
//! 112-127 are not read). The result is the pointers of the children whose
//! box the ray enters before its extent, nearest entry first when the
//! descriptor enables box sorting (bit 63), and `0xffff_ffff` for the rest.
//!
//! # Triangle nodes
//!
//! Five vertices of three 32-bit floats at bytes 0-59 and a dword at byte
//! 60 with one byte per triangle: bits 1:0 name the triangle's vertex that
//! barycentric I weighs, bits 3:2 the one J weighs (the builder stores an
//! input triangle's second vertex at the former, its third at the latter
//! and its first at the remaining one, so I and J are the weights of the
//! input triangle's second and third vertices). Triangle `k` of the node
//! uses the node's vertices [`TRIANGLE_VERTICES`]`[k]`. A program that
//! keeps fewer triangles in a node uses the bytes of the unused vertices
//! for its own data.
//!
//! With the descriptor's triangle return mode set (bit 120) the result is
//! the hit distance's numerator, its denominator, and the numerators of I
//! and J over the same denominator, as floats. The denominator is positive
//! for a triangle whose vertices run clockwise seen from the ray's origin
//! in a left-handed coordinate system. A miss returns +infinity, 1.0, 0, 0.
//! The ray's extent is not tested for triangles.
//!
//! # Where this comes from
//!
//! The node formats, the descriptor bits and the tests are those of the
//! public software emulation of this instruction in the open-source Vulkan
//! driver for this GPU family (box test and sorting network as there; the
//! triangle test is the watertight one it uses, Woop, Benthin and Wald,
//! JCGT 2013), and agree with what the title's own structure
//! builder writes and its trace programs read. Not known from either: how
//! the console rounds, which of two boxes entered at the same distance it
//! returns first, and what it returns for a ray exactly through an edge
//! (here: a hit in both triangles of the edge) or for kinds 6 and 7 (here:
//! a triangle miss; the title's programs do not read it).

/// The result dword for a box the ray misses, or a child that is not there.
pub const NONE: u32 = 0xffff_ffff;

/// The node's vertices that make triangle 0-3, by the triangle's own
/// vertex numbers 0-2.
pub const TRIANGLE_VERTICES: [[u32; 3]; 4] = [[0, 1, 2], [1, 3, 2], [2, 3, 4], [2, 4, 0]];

/// Descriptor dword 1: boxes are returned nearest first.
pub const BOX_SORT: u32 = 1 << 31;
/// Descriptor dword 3: triangles return the numerators of I and J (clear:
/// a triangle id and a hit status, which nothing here produces).
pub const RETURN_BARYCENTRICS: u32 = 1 << 24;
/// Descriptor dword 3, bits 31:28: the descriptor's type, a ray-tracing
/// structure.
pub const DESCRIPTOR_TYPE: u32 = 8;

/// The ray as the instruction takes it.
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct Ray {
    pub extent: f32,
    pub origin: [f32; 3],
    pub direction: [f32; 3],
    /// What the program computed as `1 / direction`.
    pub inverse: [f32; 3],
}

impl Ray {
    /// A ray with the inverse direction a program computes by division.
    pub fn new(origin: [f32; 3], direction: [f32; 3], extent: f32) -> Self {
        Self {
            extent,
            origin,
            direction,
            inverse: direction.map(|d| 1.0 / d),
        }
    }
}

/// The kind of the node a pointer names.
pub fn kind(pointer: u64) -> u32 {
    (pointer & 7) as u32
}

/// The byte address of the node `pointer` names, for a descriptor whose
/// first two dwords are `word0` and `word1`.
pub fn node_address(word0: u32, word1: u32, pointer: u64) -> u64 {
    let base = (u64::from(word1 & 0xff) << 32 | u64::from(word0)) << 8;
    base.wrapping_add((pointer >> 3) << 6)
}

/// What the instruction returns for the node at `words` (its first 28
/// dwords; a 64-byte node's dwords 16-27 are not read) named by a pointer
/// of this `kind`, with box sorting as `sort` says and the triangle return
/// mode set.
pub fn intersect(words: &[u32; 28], kind: u32, ray: &Ray, sort: bool) -> [u32; 4] {
    match kind & 7 {
        4 => boxes(&children(words), &boxes16(words), ray, sort),
        5 => boxes(&children(words), &boxes32(words), ray, sort),
        k @ 0..=3 => triangle(&triangle_vertices(words, k), ray).map(f32::to_bits),
        _ => MISS.map(f32::to_bits),
    }
}

/// A box node's four child pointers.
pub fn children(words: &[u32; 28]) -> [u32; 4] {
    [words[0], words[1], words[2], words[3]]
}

/// The four boxes of a 16-bit box node: minimum x y z, maximum x y z.
pub fn boxes16(words: &[u32; 28]) -> [[f32; 6]; 4] {
    std::array::from_fn(|b| {
        std::array::from_fn(|k| {
            let half = 6 * b + k;
            half_to_f32((words[4 + half / 2] >> (16 * (half % 2))) as u16)
        })
    })
}

/// The four boxes of a 32-bit box node.
pub fn boxes32(words: &[u32; 28]) -> [[f32; 6]; 4] {
    std::array::from_fn(|b| std::array::from_fn(|k| f32::from_bits(words[4 + 6 * b + k])))
}

/// [`TRIANGLE_VERTICES`] of triangle `k` in four bits each, vertex 0 lowest
/// (as the public descriptions of the format give it: 0x210, 0x231, 0x432,
/// 0x042).
pub fn vertex_map(k: u32) -> u32 {
    let v = TRIANGLE_VERTICES[(k & 3) as usize];
    v[0] | v[1] << 4 | v[2] << 8
}

/// The vertices of triangle `k` (0-3) of a triangle node, the first being
/// the one neither barycentric weighs, then I's, then J's.
pub fn triangle_vertices(words: &[u32; 28], k: u32) -> [[f32; 3]; 3] {
    let id = words[15] >> (8 * k);
    let (i, j) = (id & 3, id >> 2 & 3);
    // An id naming one vertex twice, or a vertex 3, is malformed; it reads
    // what this arithmetic lands on (a vertex of the node), as the emitted
    // code does.
    let first = 3u32.wrapping_sub(i).wrapping_sub(j) & 3;
    [first, i, j].map(|n| {
        let vertex = (vertex_map(k) >> (4 * n) & 0xf) as usize;
        std::array::from_fn(|c| f32::from_bits(words[3 * vertex + c]))
    })
}

/// The box test: child pointers of the boxes the ray enters, `NONE` for
/// the others; sorted by entry distance when `sort`, in child order
/// otherwise.
pub fn boxes(children: &[u32; 4], boxes: &[[f32; 6]; 4], ray: &Ray, sort: bool) -> [u32; 4] {
    // An infinite inverse (a direction component of 0) would turn an
    // origin exactly on a box's face into a NaN. (Maximum then minimum, as
    // the emitted code: a NaN becomes the lower bound, where `clamp` would
    // keep it.)
    #[allow(clippy::manual_clamp)]
    let inverse = ray.inverse.map(|x| x.max(-f32::MAX).min(f32::MAX));
    let mut ids = [NONE; 4];
    let mut distances = [f32::INFINITY; 4];
    for (n, b) in boxes.iter().enumerate() {
        let (mut near, mut far) = ([0f32; 3], [0f32; 3]);
        for a in 0..3 {
            let low = (b[a] - ray.origin[a]) * inverse[a];
            let high = (b[3 + a] - ray.origin[a]) * inverse[a];
            near[a] = low.min(high);
            far[a] = low.max(high);
        }
        let enter = near[0].max(near[1]).max(near[2]);
        let leave = far[0].min(far[1]).min(far[2]);
        // A box whose minimum x is NaN is switched off.
        if !b[0].is_nan() && leave >= enter.max(0.0) && enter < ray.extent {
            ids[n] = children[n];
            distances[n] = enter;
        }
    }
    if sort {
        for (a, b) in [(0, 1), (2, 3), (0, 2), (1, 3), (1, 2)] {
            if distances[b] < distances[a] {
                distances.swap(a, b);
                ids.swap(a, b);
            }
        }
    }
    ids
}

/// What a triangle the ray misses returns.
pub const MISS: [f32; 4] = [f32::INFINITY, 1.0, 0.0, 0.0];

/// The triangle test: the hit distance's numerator, the denominator, and
/// the numerators of the weights of vertices 1 and 2; [`MISS`] otherwise.
/// Watertight: the vertices are sheared so the ray runs along its largest
/// axis, and the signs of the three edge functions decide, so a ray
/// between two triangles that share an edge cannot miss both. Any NaN
/// misses, and so does a triangle seen edge-on.
pub fn triangle(p: &[[f32; 3]; 3], ray: &Ray) -> [f32; 4] {
    let d = ray.direction;
    let size = d.map(f32::abs);
    // The largest axis, and the other two in an order that keeps the
    // triangle's winding.
    let kz = if size[0] >= size[1] {
        if size[0] >= size[2] { 0 } else { 2 }
    } else if size[1] >= size[2] {
        1
    } else {
        2
    };
    let (mut kx, mut ky) = ((kz + 1) % 3, (kz + 2) % 3);
    if d[kz] < 0.0 {
        std::mem::swap(&mut kx, &mut ky);
    }
    let sz = ray.inverse[kz];
    let (sx, sy) = (d[kx] * sz, d[ky] * sz);
    let sheared = |v: &[f32; 3]| {
        let r = [
            v[0] - ray.origin[0],
            v[1] - ray.origin[1],
            v[2] - ray.origin[2],
        ];
        (r[kx] - sx * r[kz], r[ky] - sy * r[kz], sz * r[kz])
    };
    let (ax, ay, az) = sheared(&p[0]);
    let (bx, by, bz) = sheared(&p[1]);
    let (cx, cy, cz) = sheared(&p[2]);
    let u = cx * by - cy * bx;
    let v = ax * cy - ay * cx;
    let w = bx * ay - by * ax;
    let behind = u < 0.0 || v < 0.0 || w < 0.0;
    let before = u > 0.0 || v > 0.0 || w > 0.0;
    let det = u + (v + w);
    let t = (u * az + v * bz) + w * cz;
    let ahead = det > 0.0 && t >= 0.0 || det < 0.0 && t <= 0.0;
    if !(behind && before) && ahead {
        [t, det, v, w]
    } else {
        MISS
    }
}

/// A 16-bit float as a 32-bit one (exact).
pub fn half_to_f32(h: u16) -> f32 {
    let sign = u32::from(h >> 15) << 31;
    let (exponent, mantissa) = (u32::from(h >> 10 & 0x1f), u32::from(h & 0x3ff));
    let bits = match exponent {
        0 if mantissa == 0 => sign,
        0 => {
            // Subnormal: normalise.
            let shift = mantissa.leading_zeros() - 21;
            sign | (113 - shift) << 23 | (mantissa << shift & 0x3ff) << 13
        }
        0x1f => sign | 0xff << 23 | mantissa << 13,
        e => sign | (e + 112) << 23 | mantissa << 13,
    };
    f32::from_bits(bits)
}

#[cfg(test)]
pub(crate) mod tests {
    use super::*;

    /// The smallest 16-bit float not below `x` (`up`) or not above it.
    pub(crate) fn f32_to_half(x: f32, up: bool) -> u16 {
        let mut best: Option<u16> = None;
        for h in 0..=0xffffu16 {
            let f = half_to_f32(h);
            if f.is_nan() || if up { f < x } else { f > x } {
                continue;
            }
            best = match best {
                Some(b) if (half_to_f32(b) - x).abs() <= (f - x).abs() => Some(b),
                _ => Some(h),
            };
        }
        best.expect("a half")
    }

    /// A 32-bit box node.
    pub(crate) fn box32(children: [u32; 4], boxes: [[f32; 6]; 4]) -> [u32; 28] {
        let mut w = [0u32; 28];
        w[..4].copy_from_slice(&children);
        for (b, bx) in boxes.iter().enumerate() {
            for (k, x) in bx.iter().enumerate() {
                w[4 + 6 * b + k] = x.to_bits();
            }
        }
        w
    }

    /// A 16-bit box node holding `boxes` rounded outward.
    pub(crate) fn box16(children: [u32; 4], boxes: [[f32; 6]; 4]) -> [u32; 28] {
        let mut w = [0u32; 28];
        w[..4].copy_from_slice(&children);
        for (b, bx) in boxes.iter().enumerate() {
            for (k, x) in bx.iter().enumerate() {
                let half = 6 * b + k;
                let h = if x.is_nan() {
                    0x7e00
                } else {
                    f32_to_half(*x, k >= 3)
                };
                w[4 + half / 2] |= u32::from(h) << (16 * (half % 2));
            }
        }
        w
    }

    /// A triangle node: up to five vertices, and the dword at byte 60.
    pub(crate) fn triangles(vertices: &[[f32; 3]], id: u32) -> [u32; 28] {
        let mut w = [0u32; 28];
        for (n, v) in vertices.iter().enumerate() {
            for (c, x) in v.iter().enumerate() {
                w[3 * n + c] = x.to_bits();
            }
        }
        w[15] = id;
        w
    }

    const UNIT: [f32; 6] = [-1.0, -1.0, -1.0, 1.0, 1.0, 1.0];

    fn moved(b: [f32; 6], by: [f32; 3]) -> [f32; 6] {
        std::array::from_fn(|k| b[k] + by[k % 3])
    }

    const OFF: [f32; 6] = [f32::NAN; 6];

    #[test]
    fn node_pointers_name_64_byte_nodes_past_the_descriptors_base() {
        // The 64-bit form as the title uses it: base 0, the structure's
        // address folded into the pointer.
        let structure = 0x12_3456_7800u64;
        let pointer = (structure >> 8 << 5) + (0x40 >> 3 | 5);
        assert_eq!(kind(pointer), 5);
        assert_eq!(node_address(0, BOX_SORT, pointer), structure + 0x40);
        // The 32-bit form: the base in the descriptor.
        let word0 = (structure >> 8) as u32;
        let word1 = (structure >> 40) as u32 | BOX_SORT;
        assert_eq!(
            node_address(word0, word1, 0x1000 >> 3 | 4),
            structure + 0x1000
        );
        // A base past 4 GiB of 256-byte units.
        assert_eq!(node_address(0, 0x8000_0001, 8), (1 << 40) + 64);
    }

    #[test]
    fn halves_convert_exactly() {
        assert_eq!(half_to_f32(0x3c00), 1.0);
        assert_eq!(half_to_f32(0xc000), -2.0);
        assert_eq!(half_to_f32(0x7c00), f32::INFINITY);
        assert_eq!(half_to_f32(0x0001), 2f32.powi(-24));
        assert_eq!(half_to_f32(0x03ff), 1023.0 * 2f32.powi(-24));
        assert_eq!(half_to_f32(0x7bff), 65504.0);
        assert!(half_to_f32(0x7e00).is_nan());
        assert_eq!(half_to_f32(0x8000).to_bits(), 0x8000_0000);
        // Outward rounding of the test helper.
        assert!(half_to_f32(f32_to_half(0.1, true)) >= 0.1);
        assert!(half_to_f32(f32_to_half(0.1, false)) <= 0.1);
    }

    #[test]
    fn boxes_are_returned_nearest_first_and_misses_last() {
        let children = [0x105, 0x20c, 0x304, 0x400];
        // Along +x from the origin: boxes at x = 30, 10, (off axis), 20.
        let node = box32(
            children,
            [
                moved(UNIT, [30.0, 0.0, 0.0]),
                moved(UNIT, [10.0, 0.0, 0.0]),
                moved(UNIT, [10.0, 5.0, 0.0]),
                moved(UNIT, [20.0, 0.0, 0.0]),
            ],
        );
        let ray = Ray::new([0.0; 3], [1.0, 0.0, 0.0], 1000.0);
        assert_eq!(intersect(&node, 5, &ray, true), [0x20c, 0x400, 0x105, NONE]);
        // Without sorting: child order, the miss in its place.
        assert_eq!(
            intersect(&node, 5, &ray, false),
            [0x105, 0x20c, NONE, 0x400]
        );
        // The extent cuts the far ones off (entered at 9, 19, 29).
        let short = Ray {
            extent: 19.5,
            ..ray
        };
        assert_eq!(
            intersect(&node, 5, &short, true),
            [0x20c, 0x400, NONE, NONE]
        );
        // An entry exactly at the extent is no hit.
        let exact = Ray {
            extent: 19.0,
            ..ray
        };
        assert_eq!(intersect(&node, 5, &exact, true), [0x20c, NONE, NONE, NONE]);
        // Backwards: nothing.
        let back = Ray::new([0.0; 3], [-1.0, 0.0, 0.0], 1000.0);
        assert_eq!(intersect(&node, 5, &back, true), [NONE; 4]);
    }

    #[test]
    fn a_ray_starting_inside_a_box_enters_it_at_its_origin() {
        let node = box32(
            [8, 16, NONE, NONE],
            [UNIT, moved(UNIT, [3.0, 0.0, 0.0]), OFF, OFF],
        );
        let ray = Ray::new([0.0; 3], [1.0, 0.0, 0.0], 100.0);
        // Box 0 is entered at -1 (behind the origin), box 1 at 2.
        assert_eq!(intersect(&node, 5, &ray, true), [8, 16, NONE, NONE]);
        // Even with a tiny extent the box around the origin is a hit.
        let tiny = Ray {
            extent: 1e-6,
            ..ray
        };
        assert_eq!(intersect(&node, 5, &tiny, true), [8, NONE, NONE, NONE]);
    }

    #[test]
    fn axis_aligned_rays_and_origins_on_a_face_make_no_nan() {
        let node = box32([8, 16, 24, 32], [UNIT, OFF, OFF, OFF]);
        // Direction components of 0: the inverse is infinite.
        let ray = Ray::new([-5.0, -1.0, 0.0], [1.0, 0.0, 0.0], 100.0);
        assert!(ray.inverse[1].is_infinite());
        // The origin lies in the plane of the box's -y face: (-1 - -1) x inf
        // would be a NaN; with the inverse held finite it is 0, and the ray
        // running along the face is inside.
        assert_eq!(intersect(&node, 5, &ray, true), [8, NONE, NONE, NONE]);
        // Along the +y face it is outside: the face's plane is left at 0
        // and the box is entered at 4 (as the public emulation has it).
        let top = Ray::new([-5.0, 1.0, 0.0], [1.0, 0.0, 0.0], 100.0);
        assert_eq!(intersect(&node, 5, &top, true), [NONE; 4]);
        let outside = Ray::new([-5.0, 1.5, 0.0], [1.0, 0.0, 0.0], 100.0);
        assert_eq!(intersect(&node, 5, &outside, true), [NONE; 4]);
        // -0 gives -infinity.
        let minus = Ray::new([-5.0, 0.5, 0.0], [1.0, -0.0, 0.0], 100.0);
        assert_eq!(minus.inverse[1], f32::NEG_INFINITY);
        assert_eq!(intersect(&node, 5, &minus, true), [8, NONE, NONE, NONE]);
    }

    #[test]
    fn a_box_switched_off_and_a_missing_child_return_none() {
        let node = box32(
            [8, NONE, 24, 32],
            [UNIT, UNIT, [f32::NAN, -1.0, -1.0, 1.0, 1.0, 1.0], OFF],
        );
        let ray = Ray::new([-5.0, 0.0, 0.0], [1.0, 0.0, 0.0], 100.0);
        // Child 1 is hit but is not there: its pointer is what "none" is.
        assert_eq!(intersect(&node, 5, &ray, true), [8, NONE, NONE, NONE]);
    }

    #[test]
    fn equal_distances_keep_child_order() {
        let node = box32([8, 16, 24, 32], [UNIT; 4]);
        let ray = Ray::new([-5.0, 0.0, 0.0], [1.0, 0.0, 0.0], 100.0);
        assert_eq!(intersect(&node, 5, &ray, true), [8, 16, 24, 32]);
    }

    #[test]
    fn half_float_boxes_test_like_their_widened_values() {
        let boxes = [
            moved(UNIT, [30.0, 0.1, 0.0]),
            moved(UNIT, [10.0, 0.0, 0.3]),
            moved(UNIT, [10.0, 5.0, 0.0]),
            moved(UNIT, [20.7, 0.0, 0.0]),
        ];
        let children = [0x105, 0x20c, 0x304, 0x400];
        let node = box16(children, boxes);
        let wide = boxes16(&node);
        for (w, b) in wide.iter().zip(&boxes) {
            for k in 0..3 {
                assert!(w[k] <= b[k] && w[3 + k] >= b[3 + k], "rounded outward");
            }
        }
        let ray = Ray::new([0.0; 3], [1.0, 0.0, 0.0], 1000.0);
        assert_eq!(intersect(&node, 4, &ray, true), [0x20c, 0x400, 0x105, NONE]);
        assert_eq!(
            intersect(&node, 4, &ray, true),
            intersect(&box32(children, wide), 5, &ray, true)
        );
    }

    /// The hit a result describes: distance, I, J, front face.
    fn hit(r: [u32; 4]) -> Option<(f32, f32, f32, bool)> {
        let [t, det, i, j] = r.map(f32::from_bits);
        (t != f32::INFINITY).then(|| (t / det, i / det, j / det, det > 0.0))
    }

    /// Clockwise seen from the origin looking down +z in a left-handed
    /// system (x right, y up): the front face.
    const TRIANGLE: [[f32; 3]; 3] = [[0.0, 2.0, 5.0], [2.0, -2.0, 5.0], [-2.0, -2.0, 5.0]];

    #[test]
    fn a_triangle_returns_distance_and_weights_over_one_denominator() {
        let node = triangles(&TRIANGLE, 9);
        let ray = Ray::new([0.0; 3], [0.0, 0.0, 1.0], 100.0);
        let (t, i, j, front) = hit(intersect(&node, 0, &ray, true)).expect("a hit");
        assert_eq!(t, 5.0);
        // (0, 0) = 0.5 v0 + 0.25 v1 + 0.25 v2.
        assert_eq!((i, j), (0.25, 0.25));
        assert!(front);
        // Near vertex 1: I near 1.
        let ray = Ray::new([1.75, -1.75, 0.0], [0.0, 0.0, 1.0], 100.0);
        let (_, i, j, _) = hit(intersect(&node, 0, &ray, true)).expect("a hit");
        assert!(i > 0.85 && j < 0.1, "{i} {j}");
        // From behind: the same weights, the back face.
        let ray = Ray::new([0.0, 0.0, 10.0], [0.0, 0.0, -1.0], 100.0);
        let (t, i, j, front) = hit(intersect(&node, 0, &ray, true)).expect("a hit");
        assert_eq!((t, i, j, front), (5.0, 0.25, 0.25, false));
        // An unnormalised direction: the distance is in its units.
        let ray = Ray::new([0.0; 3], [0.0, 0.0, 4.0], 100.0);
        assert_eq!(hit(intersect(&node, 0, &ray, true)).unwrap().0, 1.25);
    }

    #[test]
    fn a_triangle_is_missed_beside_behind_and_edge_on() {
        let node = triangles(&TRIANGLE, 9);
        let miss = MISS.map(f32::to_bits);
        // Beside it.
        let ray = Ray::new([3.0, 0.0, 0.0], [0.0, 0.0, 1.0], 100.0);
        assert_eq!(intersect(&node, 0, &ray, true), miss);
        // It lies behind the origin.
        let ray = Ray::new([0.0, 0.0, 6.0], [0.0, 0.0, 1.0], 100.0);
        assert_eq!(intersect(&node, 0, &ray, true), miss);
        // In its plane.
        let ray = Ray::new([-5.0, 0.0, 5.0], [1.0, 0.0, 0.0], 100.0);
        assert_eq!(intersect(&node, 0, &ray, true), miss);
        // A triangle with no area.
        let flat = triangles(&[[0.0, 0.0, 5.0], [1.0, 1.0, 5.0], [2.0, 2.0, 5.0]], 9);
        let ray = Ray::new([1.0, 1.0, 0.0], [0.0, 0.0, 1.0], 100.0);
        assert_eq!(intersect(&flat, 0, &ray, true), miss);
        // A vertex that is not a number, a direction of zero.
        let nan = triangles(&[[f32::NAN, 2.0, 5.0], TRIANGLE[1], TRIANGLE[2]], 9);
        let ray = Ray::new([0.0; 3], [0.0, 0.0, 1.0], 100.0);
        assert_eq!(intersect(&nan, 0, &ray, true), miss);
        let still = Ray::new([0.0; 3], [0.0; 3], 100.0);
        assert_eq!(intersect(&node, 0, &still, true), miss);
        // The extent is the program's to test: a far triangle is a hit.
        let short = Ray::new([0.0; 3], [0.0, 0.0, 1.0], 1.0);
        assert!(hit(intersect(&node, 0, &short, true)).is_some());
    }

    #[test]
    fn a_ray_through_a_shared_edge_misses_neither_triangle() {
        // Two triangles of a quad sharing the diagonal, wound alike.
        let quad = [
            [-1.0, -1.0, 4.0],
            [-1.0, 1.0, 4.0],
            [1.0, 1.0, 4.0],
            [1.0, -1.0, 4.0],
        ];
        let first = triangles(&[quad[0], quad[1], quad[2]], 9);
        let second = triangles(&[quad[0], quad[2], quad[3]], 9);
        // A fan of rays crossing the diagonal, some exactly on it.
        for n in -40..=40 {
            let x = n as f32 / 64.0;
            for (ox, oy) in [(x, x), (x, x + 1.0 / 1024.0), (x + 1.0 / 1024.0, x)] {
                for d in [[0.0, 0.0, 1.0], [0.125, -0.0625, 1.0], [0.25, 0.5, 2.0]] {
                    // Aim at (ox, oy, 4).
                    let s = 4.0 / d[2];
                    let ray = Ray::new([ox - d[0] * s, oy - d[1] * s, 0.0], d, 100.0);
                    let a = hit(intersect(&first, 0, &ray, true));
                    let b = hit(intersect(&second, 0, &ray, true));
                    assert!(
                        a.is_some() || b.is_some(),
                        "a hole at {ox} {oy} along {d:?}"
                    );
                    for h in [a, b].into_iter().flatten() {
                        assert!((h.0 - s).abs() < 1e-5 && h.3, "{h:?}");
                    }
                }
            }
        }
    }

    #[test]
    fn the_id_dword_says_which_vertices_the_weights_belong_to() {
        // What the title's builder does with an input triangle (a, b, c)
        // and an id byte: a at the vertex neither field names, b at I's,
        // c at J's.
        let [a, b, c] = TRIANGLE;
        let ray = Ray::new([0.5, -0.25, 0.0], [0.0, 0.0, 1.0], 100.0);
        let plain = intersect(&triangles(&[a, b, c], 9), 0, &ray, true);
        for (i, j) in [(1u32, 2u32), (2, 0), (0, 1), (2, 1), (0, 2), (1, 0)] {
            let mut stored = [[0f32; 3]; 3];
            stored[(3 - i - j) as usize] = a;
            stored[i as usize] = b;
            stored[j as usize] = c;
            let node = triangles(&stored, i | j << 2);
            assert_eq!(intersect(&node, 0, &ray, true), plain, "I {i} J {j}");
        }
        // The higher bits of the byte (opaque and the like) change nothing.
        let node = triangles(&[a, b, c], 9 | 0x80);
        assert_eq!(intersect(&node, 0, &ray, true), plain);
    }

    #[test]
    fn triangles_one_to_three_take_their_vertices_and_their_id_byte() {
        let [a, b, c] = TRIANGLE;
        let far = [50.0, 50.0, 50.0];
        let ray = Ray::new([0.5, -0.25, 0.0], [0.0, 0.0, 1.0], 100.0);
        let plain = intersect(&triangles(&[a, b, c], 9), 0, &ray, true);
        for k in 1..4u32 {
            // Triangle k's vertices 0, 1, 2 are the node's `TRIANGLE_VERTICES[k]`.
            let mut vertices = [far; 5];
            let at = TRIANGLE_VERTICES[k as usize];
            vertices[at[0] as usize] = a;
            vertices[at[1] as usize] = b;
            vertices[at[2] as usize] = c;
            // Triangle 0's byte would be malformed here; only byte k counts.
            let node = triangles(&vertices, 9 << (8 * k));
            assert_eq!(intersect(&node, k, &ray, true), plain, "triangle {k}");
        }
        // A pair as the title's builder stores it: the second triangle's
        // own vertex at node vertex 3, named by I.
        let d = [2.0, 2.0, 5.0];
        // Input triangles (a, b, c) and (a, d, b): they share a and b.
        let node = triangles(&[a, b, c, d], 9 | (1 | 2 << 2) << 8);
        // Triangle 1 = node vertices (1, 3, 2) = (b, d, c) as vertices 0, 1, 2.
        let got = triangle_vertices(&node, 1);
        assert_eq!(got, [b, d, c]);
    }

    #[test]
    fn instances_and_procedural_nodes_return_a_triangle_miss() {
        let node = triangles(&TRIANGLE, 9);
        let ray = Ray::new([0.0; 3], [0.0, 0.0, 1.0], 100.0);
        for k in [6, 7] {
            assert_eq!(intersect(&node, k, &ray, true), MISS.map(f32::to_bits));
        }
    }

    #[test]
    fn the_largest_axis_and_its_sign_do_not_change_the_answer() {
        // The same triangle and ray turned so the ray's largest axis is each
        // of the three, forwards and backwards. Both turns are rotations,
        // so the facing stays.
        let turn = |v: [f32; 3], axis: usize, flip: bool| {
            let mut r = v;
            for _ in 0..axis {
                r = [r[2], r[0], r[1]];
            }
            if flip {
                // Half a turn about the axis after the largest one's next.
                let largest = (2 + axis) % 3;
                r[largest] = -r[largest];
                r[(largest + 1) % 3] = -r[(largest + 1) % 3];
            }
            r
        };
        let ray = Ray::new([0.25, -0.5, 0.0], [0.0625, 0.125, 1.0], 100.0);
        let base = hit(triangle(&TRIANGLE, &ray).map(f32::to_bits)).expect("a hit");
        for axis in 0..3 {
            for flip in [false, true] {
                let p = TRIANGLE.map(|v| turn(v, axis, flip));
                let r = Ray::new(
                    turn(ray.origin, axis, flip),
                    turn(ray.direction, axis, flip),
                    100.0,
                );
                let h = hit(triangle(&p, &r).map(f32::to_bits)).expect("a hit");
                assert!((h.0 - base.0).abs() < 1e-5, "{axis} {flip}: {h:?} {base:?}");
                assert!((h.1 - base.1).abs() < 1e-5 && (h.2 - base.2).abs() < 1e-5);
                assert_eq!(h.3, base.3, "facing, axis {axis} flip {flip}");
            }
        }
    }
}
