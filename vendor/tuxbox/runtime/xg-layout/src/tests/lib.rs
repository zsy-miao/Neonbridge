//! Host-side invariants for the layout math. Known answers come from the
//! hardware constants the code relies on (4 KiB / 64 KiB tiles, 256-byte
//! linear pitch), not from snapshots of this implementation.
use super::*;
use std::collections::HashSet;

fn desc(format: u32, width: u64, height: u32, slices: u16, mips: u16, layout: u32) -> Desc {
    Desc {
        dimension: 3, alignment: 0, width, height, slices, mips, format,
        samples: 1, quality: 0, layout, flags: 0,
    }
}

// One format per element size: R8, RG8, RGBA8, RGBA16, RGBA32, BC1, BC3.
const FORMATS: [(u32, u32); 7] = [(61, 1), (49, 2), (28, 4), (10, 8), (2, 16), (71, 8), (77, 16)];
const SHAPES: [(u64, u32); 6] = [(1, 1), (7, 3), (64, 64), (256, 256), (300, 17), (1024, 512)];

fn full_chain(w: u64, h: u32) -> u16 {
    (1 + (w as u32).max(h).ilog2()) as u16
}

fn planes() -> Vec<(Desc, Plane)> {
    let mut out = Vec::new();
    for &(format, _) in &FORMATS {
        for &(w, h) in &SHAPES {
            for layout in [LINEAR, TILE_4K] {
                for mips in [1, full_chain(w, h)] {
                    let d = desc(format, w, h, 3, mips, layout);
                    out.push((d, Plane::compute(&d).expect("supported descriptor")));
                }
            }
        }
    }
    out
}

#[test]
fn element_bytes_match_format_table() {
    for &(format, bytes) in &FORMATS {
        let p = Plane::compute(&desc(format, 64, 64, 1, 1, TILE_4K)).unwrap();
        assert_eq!(p.element_bytes(), bytes, "format {format}");
        let block = if format >= 70 { 4 } else { 1 };
        assert_eq!(p.texels_per_element_axis(), block, "format {format}");
    }
}

#[test]
fn single_mip_known_sizes() {
    // RGBA8 256x256: 1024-byte rows, a whole number of 32x32 4 KiB tiles.
    let p = Plane::compute(&desc(28, 256, 256, 1, 1, TILE_4K)).unwrap();
    assert_eq!(p.size, 256 * 256 * 4);
    // Linear rows pad to 256 bytes: 7 RGBA8 texels -> 64 texels.
    let p = Plane::compute(&desc(28, 7, 3, 2, 1, LINEAR)).unwrap();
    assert_eq!(p.size, 256 * 3 * 2);
    // A 1x1 tiled R8 still occupies one full 64x64 4 KiB tile.
    let p = Plane::compute(&desc(61, 1, 1, 1, 1, TILE_4K)).unwrap();
    assert_eq!(p.size, 4096);
    // BC1 1024x512 is 256x128 blocks of 8 bytes.
    let p = Plane::compute(&desc(71, 1024, 512, 1, 1, TILE_4K)).unwrap();
    assert_eq!(p.size, 256 * 128 * 8);
}

#[test]
fn mip_counts_and_rejection() {
    for (d, p) in planes() {
        assert_eq!(p.mip_count(), u32::from(d.mips));
        assert!(p.unpacked_mip_count() <= p.mip_count());
        assert_eq!(p.array_size(), u32::from(d.slices));
        assert!(p.mip_offset(p.mip_count()).is_none());
        assert!(p.mip_extent(p.mip_count()).is_none());
    }
    // One level more than a full chain, zero levels, and zero width are invalid.
    assert!(Plane::compute(&desc(28, 256, 256, 1, 10, TILE_4K)).is_err());
    assert!(Plane::compute(&desc(28, 256, 256, 1, 0, TILE_4K)).is_err());
    assert!(Plane::compute(&desc(28, 0, 256, 1, 1, TILE_4K)).is_err());
    assert!(Plane::compute(&desc(28, 256, 256, 1, 9, TILE_4K)).is_ok());
}

#[test]
fn mip_offsets_ordered_disjoint_and_in_slice() {
    for (d, p) in planes() {
        let ctx = format!("fmt {} {}x{} mips {} layout {}", d.format, d.width, d.height, d.mips, d.layout);
        if d.mips == 1 {
            assert_eq!(p.mip_offset(0), Some(0), "{ctx}");
        }
        assert_eq!(p.size, p.slice_bytes * u64::from(d.slices), "{ctx}");
        let unpacked = p.unpacked_mip_count();
        let mut prev: Option<u64> = None;
        for level in 0..p.mip_count() {
            let r = p.mip_range_in_slice(0, level).unwrap();
            assert_eq!(Some(r.start), p.mip_offset(level), "{ctx}");
            assert!(r.end <= p.slice_bytes, "{ctx} level {level}");
            let align = u64::from(p.mips[level as usize].alignment);
            assert_eq!(r.start % align, 0, "{ctx} level {level}");
            // Unpacked levels are stored smallest-first, so offsets fall
            // strictly as the level rises, and never overlap.
            if level < unpacked {
                if let Some(prev) = prev {
                    assert!(r.end <= prev, "{ctx} level {level}");
                }
                prev = Some(r.start);
            }
            // Slice n+1 starts exactly one slice stride later.
            let r1 = p.mip_range_in_slice(1, level).unwrap();
            assert_eq!(r1.start - r.start, p.slice_bytes, "{ctx}");
        }
        assert!(p.mip_range_in_slice(d.slices.into(), 0).is_none());
        // Packed tail levels share one 4 KiB tile below the unpacked levels.
        if unpacked < p.mip_count() {
            let lowest = (0..unpacked).filter_map(|l| p.mip_offset(l)).min().unwrap_or(p.slice_bytes);
            assert_eq!(lowest, 4096, "{ctx}");
            for level in unpacked..p.mip_count() {
                assert!(p.mip_range_in_slice(0, level).unwrap().end <= 4096, "{ctx}");
            }
        }
    }
}

#[test]
fn mip_extents_halve_and_floor() {
    for (d, p) in planes() {
        let block = p.texels_per_element_axis();
        for level in 0..p.mip_count() {
            let (w, h, depth, linear) = p.mip_extent(level).unwrap();
            let tw = ((d.width as u32) >> level).max(1);
            let th = (d.height >> level).max(1);
            assert_eq!((w, h, depth), (tw.div_ceil(block), th.div_ceil(block), u32::from(d.slices)));
            assert_eq!(linear, d.layout == LINEAR);
        }
    }
}

#[test]
fn element_offsets_distinct_aligned_and_in_mip() {
    for (d, p) in planes() {
        let eb = u64::from(p.element_bytes());
        for slice in 0..u32::from(d.slices) {
            for level in 0..p.mip_count() {
                let r = p.mip_range_in_slice(slice, level).unwrap();
                let (w, h, _, _) = p.mip_extent(level).unwrap();
                let mut seen = HashSet::new();
                for y in 0..h.min(70) {
                    for x in 0..u64::from(w.min(70)) {
                        let o = p.element_offset_in_slice(slice, level, x, y).unwrap();
                        assert!(r.contains(&o) && o + eb <= r.end, "fmt {} level {level} ({x},{y}) {o:#x} !in {r:?}", d.format);
                        assert_eq!(o % eb, 0);
                        assert!(seen.insert(o), "fmt {} level {level} ({x},{y}) aliases {o:#x}", d.format);
                    }
                }
                assert!(p.element_offset_in_slice(slice, level, u64::from(w), 0).is_none());
                assert!(p.element_offset_in_slice(slice, level, 0, h).is_none());
            }
        }
        assert!(p.element_offset_in_slice(d.slices.into(), 0, 0, 0).is_none());
    }
}

#[test]
fn tiled_4k_swizzle_is_a_bijection_on_one_tile() {
    // Every element of a 4 KiB tile maps to its own aligned slot in that tile.
    for &(format, bytes) in &FORMATS {
        let p = Plane::compute(&desc(format, 1024, 1024, 1, 1, TILE_4K)).unwrap();
        let elems = 4096 / bytes;
        let (gw, gh) = p.standard_tile_grid(0, 1, 1).unwrap();
        let tw = (1..=elems).find(|tw| elems % tw == 0 && {
            let o = p.element_offset(0, u64::from(*tw), 0).unwrap();
            o >= 4096
        }).unwrap();
        let th = elems / tw;
        assert_eq!(gw % tw, 0);
        assert_eq!(gh % th, 0);
        let mut slots = HashSet::new();
        for y in 0..th {
            for x in 0..tw {
                let o = p.element_offset(0, u64::from(x), y).unwrap();
                assert!(o < 4096 && o % u64::from(bytes) == 0, "format {format}");
                slots.insert(o);
            }
        }
        assert_eq!(slots.len() as u32, elems, "format {format}");
    }
}

#[test]
fn linear_round_trip() {
    for (d, p) in planes() {
        // The decoder first requires the offset to lie in a slice-0 mip
        // range and then walks levels from offset 0, so only single-level
        // slice 0 is a clean inverse. The native differential covers one
        // slice only, so the other cases are left unasserted.
        if d.layout != LINEAR || d.mips != 1 {
            assert!(d.layout == LINEAR || p.linear_coordinate(0).is_none());
            continue;
        }
        let (w, h, _, _) = p.mip_extent(0).unwrap();
        for slice in [0] {
            for y in [0, h / 2, h - 1] {
                for x in [0, u64::from(w / 2), u64::from(w - 1)] {
                    let o = p.element_offset_in_slice(slice, 0, x, y).unwrap();
                    assert_eq!(o, u64::from(slice) * p.slice_bytes
                        + u64::from(y) * u64::from(p.mips[0].pitch_bytes) + x * u64::from(p.element_bytes()));
                    assert_eq!(p.linear_coordinate(o), Some((0, x, y, slice)));
                }
            }
        }
        // Row padding beyond the visible width decodes to nothing.
        if u64::from(w) * u64::from(p.element_bytes()) < u64::from(p.mips[0].pitch_bytes) {
            assert_eq!(p.linear_coordinate(u64::from(w * p.element_bytes())), None);
        }
        assert_eq!(p.linear_coordinate(p.size), None);
    }
}

fn depth(format: u32, w: u64, h: u32, slices: u16, samples: u32, flags: u32) -> DepthStencil {
    let d = Desc { samples, flags, ..desc(format, w, h, slices, 1, 0) };
    DepthStencil::compute(&d).expect("supported depth descriptor")
}

#[test]
fn depth_htile_and_plane_sizes() {
    // D32S8 (19) with HTILE: one 64 KiB HTILE tile per 1024x1024 block,
    // then depth and stencil planes padded to 64 KiB Z tiles.
    let ds = depth(19, 1920, 1080, 2, 1, 0x0100_0002);
    assert_eq!(ds.htile_bytes(), 2 * 2 * 65536 * 2);
    assert_eq!(ds.depth_index(), 1);
    assert_eq!(ds.base(0), Some(0));
    assert_eq!(ds.base(1), Some(ds.htile_bytes()));
    let depth_bytes = 1920u64.next_multiple_of(128) * 1080u64.next_multiple_of(128) * 4 * 2;
    assert_eq!(ds.base(2), Some(ds.htile_bytes() + depth_bytes));
    let stencil_bytes = 1920u64.next_multiple_of(256) * 1080u64.next_multiple_of(256) * 2;
    assert_eq!(ds.size(), ds.htile_bytes() + depth_bytes + stencil_bytes);
    assert_eq!(ds.base(3), None);
    // Without HTILE the depth plane moves to index 0 and nothing precedes it.
    let ds = depth(40, 100, 100, 1, 1, 0x8130_0002);
    assert_eq!((ds.htile_bytes(), ds.depth_index(), ds.base(1)), (0, 0, None));
    assert_eq!(ds.size(), 128 * 128 * 4);
    let mut layout: Layout = unsafe { core::mem::zeroed() };
    for ds in [depth(19, 640, 480, 3, 4, 0x0100_0002), depth(53, 33, 9, 1, 8, 0x8130_0002)] {
        ds.write_planes(&mut layout);
        assert_eq!(layout.bytes, ds.size());
        let total: u64 = layout.planes[..layout.plane_count as usize].iter().map(|p| p.size).sum();
        assert_eq!(total, ds.size());
        for p in &layout.planes[..layout.plane_count as usize] {
            assert_eq!(p.size % 65536, 0);
        }
    }
}

#[test]
fn depth_swizzle_is_a_bijection_on_one_tile() {
    // (format, flags, depth bytes, stencil plane)
    for &(format, flags, dbytes, stencil) in
        &[(19, 0x0100_0002, 4, true), (40, 0x8130_0002, 4, false), (53, 0x8130_0002, 2, false)]
    {
        let htile = flags != 0x8130_0002;
        for samples in [1u32, 2, 4, 8] {
            let ds = depth(format, 2048, 2048, 4, samples, flags);
            let first = u32::from(htile);
            let mut planes = vec![(first, dbytes)];
            if stencil {
                planes.push((first + 1, 1));
            }
            if htile {
                planes.push((0, 0));
            }
            for (index, bytes) in planes {
                let ctx = format!("fmt {format} x{samples} plane {index}");
                // The tile footprint is where the 64 KiB tile index first steps.
                let tw = (1..=2048u32).find(|&x| ds.offset(index, x.into(), 0, 0).unwrap() >= 65536).unwrap();
                let th = (1..=2048u32).find(|&y| ds.offset(index, 0, y, 0).unwrap() >= 65536).unwrap();
                if bytes == 0 {
                    assert_eq!((tw, th), (1024, 1024), "{ctx}");
                } else {
                    // A data tile packs exactly 64 KiB of samples.
                    assert_eq!(u64::from(tw * th * bytes * samples), 65536, "{ctx}");
                }
                // HTILE keeps one 4-byte word per 8x8 pixel block.
                let (step, unit) = if bytes == 0 { (8, 4) } else { (1, bytes * samples) };
                let mut seen = HashSet::new();
                for slice in [0, 3] {
                    for y in (0..th).step_by(step) {
                        for x in (0..tw).step_by(step) {
                            let o = ds.offset(index, x.into(), y, slice).unwrap();
                            assert_eq!(o % u64::from(unit), 0, "{ctx}");
                            assert!(seen.insert(o), "{ctx} ({x},{y}) slice {slice}");
                            if bytes == 0 {
                                assert_eq!(ds.offset(index, (x + 7).into(), y + 7, slice), Some(o));
                            }
                        }
                    }
                }
                assert!(ds.offset(index, 2048, 0, 0).is_none());
                assert!(ds.offset(index, 0, 0, 4).is_none());
            }
        }
    }
}

#[test]
fn htile_rejects_nonzero_samples() {
    let ds = depth(19, 64, 64, 1, 4, 0x0100_0002);
    assert_eq!(ds.samples(), 4);
    assert!(ds.query_sample(0, 1).is_none());
    let ds = depth(19, 64, 64, 1, 4, 0x8130_0002);
    assert_eq!(ds.query_sample(0, 3), Some(12));
    assert_eq!(ds.query_sample(1, 3), Some(3));
    assert!(ds.query_sample(0, 4).is_none());
}

#[test]
fn msaa_color_samples_distinct() {
    for samples in [2u32, 4, 8] {
        let d = Desc { samples, flags: 0x100001, ..desc(28, 256, 256, 2, 1, 0) };
        let mut p = Plane::compute_64k_color(&d).unwrap();
        assert_eq!(p.color_samples(), samples);
        let (tw, th) = p.multisample_tile().unwrap();
        // One 64 KiB tile holds every sample of a tw x th RGBA8 block.
        assert_eq!(u64::from(tw * th) * 4 * u64::from(samples), 65536);
        assert_eq!(p.size, p.slice_bytes * 2);
        for v4 in [false, true] {
            if v4 {
                p.select_version_four();
            }
            let mut seen = HashSet::new();
            for layer in 0..2 {
                // The hardware swizzle uses the transposed tile shape.
                for y in 0..tw {
                    for x in 0..th {
                        for s in 0..samples {
                            let o = p.color_sample_offset(layer, x.into(), y, s).unwrap();
                            assert_eq!(o % 4, 0);
                            assert!(o < u64::from(layer + 1) * p.slice_bytes);
                            assert!(seen.insert(o), "x{samples} v4={v4} ({x},{y}) s{s}");
                        }
                    }
                }
            }
            assert_eq!(seen.len() as u64, 2 * 65536 / 4);
            assert!(p.color_sample_offset(0, 0, 0, samples).is_none());
            assert!(p.color_sample_offset(2, 0, 0, 0).is_none());
        }
    }
}

#[test]
fn sparse_64k_bc7_mip_chain_has_a_64k_size() {
    // Scarlett asks for this before the tile-heap assert's successor: a 2D
    // BC7 array, ten mips, layout 266 (0x10a).
    let d = Desc { flags: 0x300000, ..desc(98, 512, 512, 32, 10, 266) };
    let plane = Plane::compute_sparse_64k(&d).expect("layout 266 mip chain");
    assert_ne!(plane.size, 0);
    assert_ne!(plane.size, u64::MAX);
    assert_eq!(plane.alignment, 65536);
    assert_eq!(plane.size % 65536, 0);
}

#[test]
fn single_sample_color_has_no_msaa_tile() {
    let p = Plane::compute(&desc(28, 64, 64, 1, 1, TILE_4K)).unwrap();
    assert_eq!(p.color_samples(), 1);
    assert!(p.multisample_tile().is_none());
}
