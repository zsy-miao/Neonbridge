//! Conservative, bounded validation of a serialized uncompressed Xbox BLAS.
//!
//! This is an offline control, not a production traversal gate. A leaf must be
//! contained in the intersection of every box on its path. Shared leaves and
//! spatially clipped triangles are intentionally unsupported: valid trees that
//! need split coverage analysis are rejected rather than certified here.
#[path = "../../../runtime/d3d12/src/commands/serialized_blas.rs"]
mod serialized_blas;

use std::{env, fs::File, io::Read};
const BLOCK: usize = 64;
const LIMIT: usize = 64 * 1024 * 1024;

#[derive(Debug, PartialEq)]
pub(crate) enum Error {
    Length,
    Decode,
    Bounds,
    ClippedLeaf,
    SharedLeaf,
    Pointer,
}
#[derive(Debug)]
pub(crate) struct Summary {
    box16: usize,
    box32: usize,
    leaves: usize,
    triangles: usize,
}

fn word(bytes: &[u8], offset: usize) -> u32 {
    u32::from_le_bytes(bytes[offset..offset + 4].try_into().unwrap())
}
fn half(bits: u16) -> f32 {
    let sign = u32::from(bits & 0x8000) << 16;
    let exponent = (bits >> 10) & 31;
    let fraction = u32::from(bits & 1023);
    if exponent == 0 {
        let value = fraction as f32 * 2f32.powi(-24);
        return if sign != 0 { -value } else { value };
    }
    let value = if exponent == 31 {
        sign | 0x7f80_0000 | (fraction << 13)
    } else {
        sign | ((u32::from(exponent) + 112) << 23) | (fraction << 13)
    };
    f32::from_bits(value)
}
fn valid(b: [f32; 6]) -> bool {
    b.iter().all(|v| v.is_finite()) && (0..3).all(|i| b[i] <= b[i + 3])
}
pub(crate) fn validate(bytes: &[u8]) -> Result<Summary, Error> {
    if bytes.len() < 2 * BLOCK || bytes.len() > LIMIT {
        return Err(Error::Length);
    }
    // Reuse the actual decoder's header, graph-region, pointer, cycle, leaf
    // descriptor and geometry checks. Do not use its reconstructed host nodes.
    let triangles = serialized_blas::decode(bytes).map_err(|_| Error::Decode)?;
    let body = &bytes[BLOCK..];
    let root_bounds = std::array::from_fn(|i| f32::from_bits(word(body, 16 + i * 4)));
    if !valid(root_bounds) {
        return Err(Error::Bounds);
    }
    let mut pending = vec![(word(body, 48), root_bounds)];
    let mut seen = vec![false; body.len() / BLOCK];
    let mut summary = Summary {
        box16: 0,
        box32: 0,
        leaves: 0,
        triangles: triangles.len(),
    };
    while let Some((pointer, path)) = pending.pop() {
        let kind = pointer & 7;
        let block = (pointer >> 3) as usize;
        let blocks = if kind == 5 { 2 } else { 1 };
        if block.checked_add(blocks).is_none_or(|end| end > seen.len()) {
            return Err(Error::Pointer);
        }
        if seen[block..block + blocks].iter().any(|v| *v) {
            return Err(if kind == 0 {
                Error::SharedLeaf
            } else {
                Error::Pointer
            });
        }
        seen[block..block + blocks].fill(true);
        let offset = block * BLOCK;
        match kind {
            4 | 5 => {
                if kind == 4 {
                    summary.box16 += 1;
                } else {
                    summary.box32 += 1;
                }
                let stride = if kind == 4 { 12 } else { 24 };
                let mut count = 0;
                for i in 0..4 {
                    let child = word(body, offset + i * 4);
                    if child == u32::MAX {
                        continue;
                    }
                    count += 1;
                    let at = offset + 16 + i * stride;
                    let bounds: [f32; 6] = std::array::from_fn(|j| {
                        if kind == 4 {
                            half(u16::from_le_bytes(
                                body[at + j * 2..at + j * 2 + 2].try_into().unwrap(),
                            ))
                        } else {
                            f32::from_bits(word(body, at + j * 4))
                        }
                    });
                    if !valid(bounds) {
                        return Err(Error::Bounds);
                    }
                    let merged = std::array::from_fn(|j| {
                        if j < 3 {
                            path[j].max(bounds[j])
                        } else {
                            path[j].min(bounds[j])
                        }
                    });
                    if !valid(merged) {
                        return Err(Error::Bounds);
                    }
                    pending.push((child, merged));
                }
                if count == 0 {
                    return Err(Error::Pointer);
                }
            }
            0 => {
                let count = if word(body, offset + 48) & 0x80 != 0 {
                    4
                } else {
                    3
                };
                for vertex in 0..count {
                    for axis in 0..3 {
                        let v = f32::from_bits(word(body, offset + vertex * 12 + axis * 4));
                        if !v.is_finite() || v < path[axis] || v > path[axis + 3] {
                            return Err(Error::ClippedLeaf);
                        }
                    }
                }
                summary.leaves += 1;
            }
            _ => return Err(Error::Pointer),
        }
    }
    Ok(summary)
}

fn main() {
    let Some(path) = env::args_os().nth(1) else {
        eprintln!("usage: validate_traversal SERIALIZED");
        std::process::exit(2);
    };
    let result = (|| -> Result<Summary, String> {
        let file = File::open(path).map_err(|e| e.to_string())?;
        let mut bytes = Vec::new();
        file.take((LIMIT + 1) as u64)
            .read_to_end(&mut bytes)
            .map_err(|e| e.to_string())?;
        validate(&bytes).map_err(|e| format!("{e:?}"))
    })();
    match result {
        Ok(s) => println!(
            "box16={} box32={} leaves={} triangles={}",
            s.box16, s.box32, s.leaves, s.triangles
        ),
        Err(e) => {
            eprintln!("serialized traversal validation failed: {e}");
            std::process::exit(1);
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    fn store(data: &mut [u8], offset: usize, value: u32) {
        data[offset..offset + 4].copy_from_slice(&value.to_le_bytes());
    }
    fn fixture(box32: bool) -> Vec<u8> {
        let size = if box32 { 320 } else { 256 };
        let mut d = vec![0; size];
        for (offset, value) in [
            (32, size as u32),
            (40, (size - 64) as u32),
            (64, 0x80000004),
            (68, (size / 64 - 2) as u32),
            (72, 0xff1),
            (76, 1),
            (92, 1f32.to_bits()),
            (96, 1f32.to_bits()),
            (112, if box32 { 21 } else { 20 }),
            (116, 64),
            (120, 64),
            (124, 0x7f7f7f),
            (176, 128),
            (180, 1),
            (184, 0),
            (188, 0x0902),
            (192, 8),
            (196, u32::MAX),
            (200, u32::MAX),
            (204, u32::MAX),
        ] {
            store(&mut d, offset, value);
        }
        for offset in [140, 156, 164, 168] {
            store(&mut d, offset, 1f32.to_bits());
        }
        if box32 {
            for at in [220, 224] {
                store(&mut d, at, 1f32.to_bits());
            }
        } else {
            for at in [214, 216] {
                d[at..at + 2].copy_from_slice(&0x3c00u16.to_le_bytes());
            }
        }
        d
    }
    #[test]
    fn accepts_both_box_encodings() {
        for packed in [false, true] {
            let s = validate(&fixture(packed)).unwrap();
            assert_eq!((s.leaves, s.triangles), (1, 2));
        }
    }
    #[test]
    fn rejects_clipped_nan_infinite_and_inverted_bounds() {
        let mut d = fixture(false);
        d[214..216].copy_from_slice(&0x3800u16.to_le_bytes());
        assert_eq!(validate(&d).unwrap_err(), Error::ClippedLeaf);
        for bad in [0x7c00u16, 0x7e00, 0xbc00] {
            let mut d = fixture(false);
            d[214..216].copy_from_slice(&bad.to_le_bytes());
            assert_eq!(validate(&d).unwrap_err(), Error::Bounds);
        }
        let mut d = fixture(true);
        store(&mut d, 220, f32::NAN.to_bits());
        assert_eq!(validate(&d).unwrap_err(), Error::Bounds);
    }
    #[test]
    fn rejects_out_of_range_cycle_and_truncation() {
        let mut d = fixture(false);
        store(&mut d, 192, 0xfffffff8);
        assert!(validate(&d).is_err());
        let mut d = fixture(false);
        store(&mut d, 192, 20);
        assert!(validate(&d).is_err());
        let mut d = fixture(true);
        d.pop();
        assert!(validate(&d).is_err());
    }
    #[test]
    fn rejects_shared_leaf_instead_of_assuming_split_coverage() {
        let mut d = fixture(false);
        store(&mut d, 196, 8);
        for (offset, value) in [
            (220, 0u16),
            (222, 0),
            (224, 0),
            (226, 0x3c00),
            (228, 0x3c00),
            (230, 0),
        ] {
            d[offset..offset + 2].copy_from_slice(&value.to_le_bytes());
        }
        assert_eq!(validate(&d).unwrap_err(), Error::SharedLeaf);
    }
    #[test]
    fn all_ancestor_bounds_must_contain_descendants() {
        let mut d = fixture(false);
        d.resize(320, 0);
        store(&mut d, 32, 320);
        store(&mut d, 40, 256);
        store(&mut d, 68, 3);
        store(&mut d, 112, 28); // root at body block 3, BOX16
        store(&mut d, 256, 20); // existing BOX16 is its only child
        for at in [260, 264, 268] {
            store(&mut d, at, u32::MAX);
        }
        for at in [278, 280] {
            d[at..at + 2].copy_from_slice(&0x3c00u16.to_le_bytes());
        }
        assert!(validate(&d).is_ok());
        d[278..280].copy_from_slice(&0x3800u16.to_le_bytes());
        assert_eq!(validate(&d).unwrap_err(), Error::ClippedLeaf);
    }

    #[test]
    fn ignores_inactive_bounds_but_checks_box32_inversion() {
        let mut d = fixture(false);
        d[220..222].copy_from_slice(&0x7e00u16.to_le_bytes());
        assert!(validate(&d).is_ok()); // second child is the invalid sentinel
        let mut d = fixture(true);
        store(&mut d, 220, (-1f32).to_bits());
        assert_eq!(validate(&d).unwrap_err(), Error::Bounds);
    }
}
