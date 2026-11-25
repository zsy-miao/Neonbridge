//! Decode the observed Scarlett serialized triangle BLAS into original primitive order.
//! The console traversal nodes cannot be passed to a desktop GPU as a native AS.

#[derive(Debug, PartialEq)]
pub struct Triangle {
    pub geometry: u32,
    pub primitive: u32,
    pub vertices: [[f32; 3]; 3],
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum DecodeError {
    InvalidHeader(&'static str),
    UnsupportedLayout(&'static str, u32, u32),
    InvalidHierarchy(&'static str, u32),
    InvalidTriangle(&'static str, u32),
}

const BLOCK: usize = 64;

// All callers pass a checked 64-byte header or node, with a constant field index.
fn word(block: &[u8], i: usize) -> u32 {
    u32::from_le_bytes(block[4 * i..4 * i + 4].try_into().unwrap())
}

fn wide(block: &[u8], i: usize) -> u64 {
    u64::from_le_bytes(block[8 * i..8 * i + 8].try_into().unwrap())
}

/// Accepts a serialized AS span beginning at its 32-byte driver identifier. A caller
/// reading a suballocation must restrict it to the length in header bytes 32..40.
/// Returns indexed triangles grouped by geometry. Material opacity and instance flags
/// are separate inputs to a host rebuild.
pub fn decode(serialized: &[u8]) -> Result<Vec<Triangle>, DecodeError> {
    use DecodeError::*;
    let (header, body) = serialized
        .split_at_checked(BLOCK)
        .ok_or(InvalidHeader("missing serialized header"))?;
    if wide(header, 4) != serialized.len() as u64 || wide(header, 5) != body.len() as u64 {
        return Err(InvalidHeader("serialized and payload lengths"));
    }
    if header[..32] != [0; 32] || wide(header, 6) != 0 {
        return Err(UnsupportedLayout(
            "driver identifier or pointer count",
            word(header, 0),
            word(header, 12),
        ));
    }
    let shape = body
        .first_chunk::<BLOCK>()
        .ok_or(InvalidHeader("missing BVH header"))?;
    if !body.len().is_multiple_of(BLOCK)
        || u64::from(word(shape, 1)) + 1 != (body.len() / BLOCK) as u64
    {
        return Err(InvalidHeader("BVH block count"));
    }
    if word(shape, 0) != 0x8000_0004
        || word(shape, 2) != 0xff1
        || wide(shape, 5) != 0
        || word(shape, 14) != 64
        || word(shape, 15) != 0x007f_7f7f
    {
        return Err(UnsupportedLayout(
            "BVH header format",
            word(shape, 0),
            word(shape, 2),
        ));
    }
    let min = [4, 5, 6].map(|i| f32::from_bits(word(shape, i)));
    let max = [7, 8, 9].map(|i| f32::from_bits(word(shape, i)));
    if !(0..3).all(|i| min[i].is_finite() && max[i].is_finite() && min[i] <= max[i]) {
        return Err(InvalidHeader("BVH bounds"));
    }
    let leaf_offset = word(shape, 13) as usize;
    let leaf_count = word(shape, 3) as usize;
    if !leaf_offset.is_multiple_of(BLOCK)
        || leaf_offset < BLOCK
        || leaf_offset > body.len()
        || leaf_count == 0
        || leaf_count > (body.len() - leaf_offset) / BLOCK
    {
        return Err(InvalidHeader("BVH leaf table range"));
    }
    let leaf_start = leaf_offset / BLOCK;
    let leaf_end = leaf_start + leaf_count;
    let mut visited = vec![false; body.len() / BLOCK];
    let mut pending = vec![word(shape, 12)];
    let mut leaves_seen = 0;
    let mut triangles = Vec::with_capacity(leaf_count);
    while let Some(pointer) = pending.pop() {
        let kind = pointer & 7;
        let index = (pointer >> 3) as usize;
        let blocks = if kind == 5 { 2 } else { 1 };
        let Some(used) = visited.get_mut(index..index + blocks) else {
            return Err(InvalidHierarchy("node pointer outside BVH", pointer));
        };
        if used.iter().any(|&v| v) {
            // A spatial split may reference the same terminal leaf from two
            // different child bounds. Rebuild its original triangles only once.
            if kind == 0 && (leaf_start..leaf_end).contains(&index) {
                continue;
            }
            return Err(InvalidHierarchy("repeated internal node", pointer));
        }
        used.fill(true);
        let node = &body[index * BLOCK..(index + 1) * BLOCK];
        match kind {
            0 if (leaf_start..leaf_end).contains(&index) => {
                decode_leaf(node, min, max, &mut triangles)?;
                leaves_seen += 1;
            }
            4 | 5 if index >= leaf_end => {
                let old = pending.len();
                pending.extend((0..4).map(|i| word(node, i)).filter(|&p| p != u32::MAX));
                if pending.len() == old {
                    return Err(InvalidHierarchy("empty internal node", pointer));
                }
            }
            0 | 4 | 5 => return Err(InvalidHierarchy("node outside type region", pointer)),
            _ => return Err(UnsupportedLayout("node type", kind, pointer)),
        }
    }
    if leaves_seen != leaf_count {
        return Err(InvalidHierarchy("unreachable leaves", leaves_seen as u32));
    }
    triangles.sort_unstable_by_key(|t| (t.geometry, t.primitive));
    // Other spatial splits store the original primitive in distinct leaves.
    // A duplicate ID with different coordinates or winding remains invalid.
    triangles.dedup();
    if triangles
        .windows(2)
        .any(|t| (t[0].geometry, t[0].primitive) == (t[1].geometry, t[1].primitive))
    {
        return Err(InvalidTriangle("duplicate primitive ID", 0));
    }
    if let Some(t) = triangles.iter().find(|t| t.primitive >= 1 << 29) {
        return Err(InvalidTriangle("primitive outside DXR limits", t.primitive));
    }
    Ok(triangles)
}

/// Desktop DXR assigns IDs by array index. The Xbox builder can omit a whole
/// geometry or individual inactive triangles from its cooked tree. NaN-X
/// placeholders consume the original index without producing intersections.
/// See the DXR functional spec, “Inactive primitives and instances”.
pub fn decode_for_build(serialized: &[u8]) -> Result<Vec<Triangle>, DecodeError> {
    let triangles = decode(serialized)?;
    let mut slots = 0u64;
    let mut next_geometry = 0u32;
    for group in triangles.chunk_by(|a, b| a.geometry == b.geometry) {
        let last = group.last().unwrap();
        slots += u64::from(last.geometry - next_geometry) + u64::from(last.primitive) + 1;
        next_geometry = last.geometry + 1;
    }
    if slots == triangles.len() as u64 {
        return Ok(triangles);
    }
    // Limit expansion from corrupt IDs before allocating or sending a GPU job.
    if slots > (1 << 29) || slots > (triangles.len() as u64 * 16).max(65_536) {
        return Err(DecodeError::InvalidTriangle(
            "sparse ID expansion",
            u32::MAX,
        ));
    }
    const INACTIVE: [[f32; 3]; 3] = [[f32::NAN, 0., 0.]; 3];
    let mut expanded = Vec::new();
    expanded
        .try_reserve_exact(slots as usize)
        .map_err(|_| DecodeError::InvalidHeader("BLAS allocation size"))?;
    let mut next = (0, 0);
    for t in triangles {
        while next.0 < t.geometry {
            if next.1 == 0 {
                expanded.push(Triangle {
                    geometry: next.0,
                    primitive: 0,
                    vertices: INACTIVE,
                });
            }
            next = (next.0 + 1, 0);
        }
        while next.1 < t.primitive {
            expanded.push(Triangle {
                geometry: next.0,
                primitive: next.1,
                vertices: INACTIVE,
            });
            next.1 += 1;
        }
        next.1 += 1;
        expanded.push(t);
    }
    Ok(expanded)
}

fn decode_leaf(
    node: &[u8],
    min: [f32; 3],
    max: [f32; 3],
    out: &mut Vec<Triangle>,
) -> Result<(), DecodeError> {
    let descriptor = word(node, 12);
    let paired = descriptor & 0x80 != 0;
    let second_id = word(node, 14);
    let selectors = word(node, 15);
    if descriptor & 0x7f != 0 || selectors >> 16 != 0 {
        return Err(DecodeError::UnsupportedLayout(
            "quad flags",
            descriptor,
            selectors,
        ));
    }
    if paired != (second_id != u32::MAX) || paired != (selectors >> 8 != 0) {
        return Err(DecodeError::InvalidTriangle(
            "quad pair descriptor",
            descriptor,
        ));
    }
    // The first triangle uses corners 0,1,2. Its neighbour shares the edge 1,2 with
    // reversed winding. Each selector rotates the vertices back to original index order.
    for side in 0..=usize::from(paired) {
        let rotation = match (selectors >> (8 * side)) & 0xff {
            9 => 0,
            2 => 1,
            4 => 2,
            _ => {
                return Err(DecodeError::UnsupportedLayout(
                    "quad rotation",
                    descriptor,
                    selectors,
                ));
            }
        };
        let mut corners = [[0, 1, 2], [1, 3, 2]][side];
        corners.rotate_left(rotation);
        let vertices = corners
            .map(|corner| [0, 1, 2].map(|axis| f32::from_bits(word(node, 3 * corner + axis))));
        if !vertices
            .iter()
            .all(|v| (0..3).all(|a| min[a] <= v[a] && v[a] <= max[a]))
        {
            return Err(DecodeError::InvalidTriangle(
                "vertex outside BVH bounds",
                word(node, 13 + side),
            ));
        }
        out.push(Triangle {
            geometry: descriptor >> 8,
            primitive: word(node, 13 + side),
            vertices,
        });
    }
    Ok(())
}

#[cfg(test)]
#[path = "../tests/commands/serialized_blas.rs"]
mod tests;
