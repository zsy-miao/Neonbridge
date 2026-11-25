//! Check traversal of the original runtime-style BOX16/BOX32 and leaf records
//! against Frosty's independently decoded indexed sphere mesh.
#[path = "../../../runtime/d3d12/src/commands/serialized_blas.rs"]
mod serialized_blas;
mod shadow_instance;

use std::{env, fs, io, io::Write};

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

fn sub(a: [f32; 3], b: [f32; 3]) -> [f32; 3] {
    std::array::from_fn(|i| a[i] - b[i])
}

fn dot(a: [f32; 3], b: [f32; 3]) -> f32 {
    a.into_iter().zip(b).map(|(x, y)| x * y).sum()
}

fn cross(a: [f32; 3], b: [f32; 3]) -> [f32; 3] {
    [
        a[1] * b[2] - a[2] * b[1],
        a[2] * b[0] - a[0] * b[2],
        a[0] * b[1] - a[1] * b[0],
    ]
}

fn triangle(origin: [f32; 3], direction: [f32; 3], vertices: [[f32; 3]; 3]) -> Option<f32> {
    let a = sub(vertices[1], vertices[0]);
    let b = sub(vertices[2], vertices[0]);
    let p = cross(direction, b);
    let determinant = dot(a, p);
    if determinant.abs() < 1e-9 {
        return None;
    }
    let inverse = 1.0 / determinant;
    let distance = sub(origin, vertices[0]);
    let u = dot(distance, p) * inverse;
    if !(0.0..=1.0).contains(&u) {
        return None;
    }
    let q = cross(distance, a);
    let v = dot(direction, q) * inverse;
    if v < 0.0 || u + v > 1.0 {
        return None;
    }
    let t = dot(b, q) * inverse;
    (t >= 0.0).then_some(t)
}

fn hit_box(origin: [f32; 3], direction: [f32; 3], bounds: [f32; 6]) -> bool {
    let mut near = 0.0f32;
    let mut far = f32::INFINITY;
    for i in 0..3 {
        if direction[i] == 0.0 {
            if origin[i] < bounds[i] || origin[i] > bounds[i + 3] {
                return false;
            }
        } else {
            let a = (bounds[i] - origin[i]) / direction[i];
            let b = (bounds[i + 3] - origin[i]) / direction[i];
            near = near.max(a.min(b));
            far = far.min(a.max(b));
        }
    }
    near <= far
}

fn trace(
    body: &[u8],
    triangles: &[serialized_blas::Triangle],
    origin: [f32; 3],
    direction: [f32; 3],
) -> Result<(f32, u32, usize), String> {
    let mut pending = vec![word(body, 48)];
    let mut best = f32::INFINITY;
    let mut primitive = u32::MAX;
    let mut visited = 0;
    while let Some(pointer) = pending.pop() {
        let offset = (pointer as usize >> 3) * 64;
        visited += 1;
        match pointer & 7 {
            4 | 5 => {
                let packed = pointer & 7 == 4;
                for child in 0..4 {
                    let next = word(body, offset + child * 4);
                    if next == u32::MAX {
                        continue;
                    }
                    let base = offset + 16 + child * if packed { 12 } else { 24 };
                    let bounds: [f32; 6] = std::array::from_fn(|i| {
                        if packed {
                            half(u16::from_le_bytes(
                                body[base + i * 2..base + i * 2 + 2].try_into().unwrap(),
                            ))
                        } else {
                            f32::from_bits(word(body, base + i * 4))
                        }
                    });
                    if hit_box(origin, direction, bounds) {
                        pending.push(next);
                    }
                }
            }
            0 => {
                let descriptor = word(body, offset + 48);
                let geometry = descriptor >> 8;
                let count = if descriptor & 0x80 != 0 { 2 } else { 1 };
                for side in 0..count {
                    let id = word(body, offset + 52 + side * 4);
                    let item = triangles
                        .iter()
                        .find(|t| t.geometry == geometry && t.primitive == id)
                        .ok_or_else(|| format!("missing geometry {geometry} primitive {id}"))?;
                    if let Some(t) = triangle(origin, direction, item.vertices)
                        && t < best
                    {
                        best = t;
                        primitive = id;
                    }
                }
            }
            other => return Err(format!("unsupported node type {other}")),
        }
    }
    Ok((best, primitive, visited))
}

// Construct the two-instance test scene from Xbox-layout records. This
// deliberately handles only the uncompressed sphere fixture, not arbitrary
// runtime TLAS input.
fn sphere_scene(body: &[u8], shadow_tlas: bool) -> Result<Vec<u8>, String> {
    use shadow_instance::{Blas, Bounds, Instance};
    let bounds = Bounds {
        min: std::array::from_fn(|i| f32::from_bits(word(body, 16 + i * 4))),
        max: std::array::from_fn(|i| f32::from_bits(word(body, 28 + i * 4))),
    };
    let blas = Blas {
        bounds,
        root: word(body, 48),
        compression: word(body, 60),
    };
    let mut output = vec![0u8; if shadow_tlas { 576 } else { 448 }];
    let store = |out: &mut [u8], offset: usize, value: u32| {
        out[offset..offset + 4].copy_from_slice(&value.to_le_bytes());
    };
    // Header, a single BOX32 root, and two 128-byte INSTANCE nodes.
    let root = (1 << 3) | 5;
    store(&mut output, 48, root);
    for i in 0..4 {
        store(&mut output, 64 + 4 * i, u32::MAX);
    }
    for index in 0..if shadow_tlas { 3 } else { 2 } {
        let offset = 192 + index * 128;
        let pointer = ((offset / 64) as u32) << 3 | 6;
        let transform = if shadow_tlas && index == 1 {
            [
                [0.0, -2.0, 0.5, 512.0],
                [0.5, 0.0, 0.25, 0.0],
                [0.0, 0.0, 1.5, 0.0],
            ]
        } else {
            [
                [1.0, 0.0, 0.0, 512.0 * index as f32],
                [0.0, 1.0, 0.0, 0.0],
                [0.0, 0.0, 1.0, 0.0],
            ]
        };
        let mask = if shadow_tlas && index == 2 { 0 } else { 0xff };
        let input = Instance {
            transform,
            id_and_mask: (mask << 24) | (7 + index as u32),
            contribution_and_flags: 0,
            blas_address: 0x10000,
        };
        let Some(record) = shadow_instance::build(input, blas, index as u32, root)
            .map_err(|error| format!("instance {index}: {error:?}"))?
        else {
            continue;
        };
        store(&mut output, 64 + index * 4, pointer);
        for axis in 0..3 {
            store(
                &mut output,
                80 + index * 24 + axis * 4,
                record.bounds.min[axis].to_bits(),
            );
            store(
                &mut output,
                92 + index * 24 + axis * 4,
                record.bounds.max[axis].to_bits(),
            );
        }
        for (word_index, value) in record.words.into_iter().enumerate() {
            store(&mut output, offset + word_index * 4, value);
        }
    }
    Ok(output)
}

fn trace_scene(
    scene: &[u8],
    body: &[u8],
    triangles: &[serialized_blas::Triangle],
    origin: [f32; 3],
    direction: [f32; 3],
    maximum: f32,
) -> Result<Option<(u32, u32, f32)>, String> {
    let mut best: Option<(u32, u32, f32)> = None;
    let root = ((word(scene, 48) >> 3) as usize) * 64;
    for child in 0..4 {
        let pointer = word(scene, root + child * 4);
        if pointer == u32::MAX {
            continue;
        }
        if pointer & 7 != 6 {
            return Err("unexpected synthetic TLAS node".into());
        }
        let bounds =
            std::array::from_fn(|i| f32::from_bits(word(scene, root + 16 + child * 24 + i * 4)));
        if !hit_box(origin, direction, bounds) {
            continue;
        }
        let node = (pointer as usize >> 3) * 64;
        if word(scene, node + 48) >> 24 == 0 {
            continue;
        }
        let inverse = |row: usize, vector: [f32; 3], point: bool| {
            let translation = if point {
                f32::from_bits(word(scene, node + 64 + row * 4))
            } else {
                0.0
            };
            translation
                + (0..3)
                    .map(|column| {
                        f32::from_bits(word(scene, node + 76 + (row * 3 + column) * 4))
                            * vector[column]
                    })
                    .sum::<f32>()
        };
        let local_origin = std::array::from_fn(|row| inverse(row, origin, true));
        let local_direction = std::array::from_fn(|row| inverse(row, direction, false));
        let (distance, primitive, _) = trace(body, triangles, local_origin, local_direction)?;
        if primitive != u32::MAX
            && distance <= maximum
            && best.is_none_or(|(_, _, current)| distance < current)
        {
            best = Some((word(scene, node + 48) & 0x00ff_ffff, primitive, distance));
        }
    }
    Ok(best)
}

fn main() -> Result<(), Box<dyn std::error::Error>> {
    let mut args = env::args_os().skip(1);
    let source = fs::read(args.next().ok_or("missing serialized sphere")?)?;
    let mesh = fs::read(args.next().ok_or("missing independent sphere vertices")?)?;
    let triangles = serialized_blas::decode(&source)
        .map_err(|e| io::Error::new(io::ErrorKind::InvalidData, format!("{e:?}")))?;
    if mesh.len() != 528 * 36 || triangles.len() != 528 {
        return Err("unexpected sphere triangle count".into());
    }
    let body = &source[64..];
    let mut total_nodes = 0;
    for (id, bytes) in mesh.chunks_exact(36).enumerate() {
        let vertices: [[f32; 3]; 3] = std::array::from_fn(|v| {
            std::array::from_fn(|axis| f32::from_bits(word(bytes, (v * 3 + axis) * 4)))
        });
        let center: [f32; 3] =
            std::array::from_fn(|axis| vertices.iter().map(|v| v[axis]).sum::<f32>() / 3.0);
        let radius = dot(center, center).sqrt();
        let normal = center.map(|x| x / radius);
        let distance = 1.0 + (id % 13) as f32 * 0.25;
        let origin = std::array::from_fn(|axis| center[axis] + normal[axis] * distance);
        let (hit, primitive, nodes) = trace(body, &triangles, origin, normal.map(|x| -x))
            .map_err(|e| io::Error::new(io::ErrorKind::InvalidData, e))?;
        if primitive != id as u32 || (hit - distance).abs() > 0.0002 {
            return Err(format!(
                "ray {id}: primitive {primitive}, distance {hit}, expected {distance}"
            )
            .into());
        }
        total_nodes += nodes;
    }
    for origin in [
        [0.0, 200.0, -180.0],
        [256.0, 200.0, -180.0],
        [512.0, 200.0, -180.0],
    ] {
        let (distance, primitive, nodes) = trace(body, &triangles, origin, [0.0, 0.0, 1.0])
            .map_err(|e| io::Error::new(io::ErrorKind::InvalidData, e))?;
        if primitive != u32::MAX || !distance.is_infinite() {
            return Err(format!(
                "expected a miss from {origin:?}, got primitive {primitive} at {distance}"
            )
            .into());
        }
        total_nodes += nodes;
    }
    let rays_path = args.next();
    if rays_path.as_deref() == Some(std::ffi::OsStr::new("--export-shadow-tlas")) {
        let output = args.next().ok_or("missing synthetic TLAS output path")?;
        if args.next().is_some() {
            return Err("unexpected synthetic TLAS argument".into());
        }
        let scene =
            sphere_scene(body, true).map_err(|e| io::Error::new(io::ErrorKind::InvalidData, e))?;
        fs::write(output, scene)?;
        return Ok(());
    }
    let hits_path = args.next();
    let option = args.next();
    let shadow_tlas = option.as_ref().is_some_and(|arg| arg == "--shadow-tlas");
    if rays_path.is_some() != hits_path.is_some()
        || (option.is_some() && (!shadow_tlas || rays_path.is_none()))
        || args.next().is_some()
    {
        return Err("supply both the ray input and hit output paths".into());
    }
    if let (Some(rays), Some(hits)) = (rays_path, hits_path) {
        let scene = sphere_scene(body, shadow_tlas)
            .map_err(|e| io::Error::new(io::ErrorKind::InvalidData, e))?;
        let rays = fs::read(rays)?;
        if !rays.len().is_multiple_of(36) {
            return Err("sphere ray buffer is not a sequence of 36-byte records".into());
        }
        let mut output = io::BufWriter::new(fs::File::create(hits)?);
        for ray in rays.chunks_exact(36) {
            let value = |index: usize| f32::from_bits(word(ray, index * 4));
            let origin = [value(0), value(1), value(2)];
            let direction = [value(4), value(5), value(6)];
            let flags = word(ray, 32);
            if value(3) != 0.0 || !matches!(flags, 0 | 0x40 | 0x80) {
                return Err("unsupported fixture ray flags or minimum distance".into());
            }
            let hit = if flags == 0x80 {
                None // Both sphere instances use non-opaque geometry.
            } else {
                trace_scene(&scene, body, &triangles, origin, direction, value(7))
                    .map_err(|e| io::Error::new(io::ErrorKind::InvalidData, e))?
            };
            let (instance, geometry, primitive, distance) = match hit {
                Some((instance, primitive, distance)) => (instance, 0u32, primitive, distance),
                None => (u32::MAX, u32::MAX, u32::MAX, 0.0),
            };
            for value in [instance, geometry, primitive, distance.to_bits()] {
                output.write_all(&value.to_le_bytes())?;
            }
        }
        output.flush()?;
        println!("{} synthetic TLAS rays written", rays.len() / 36);
    }
    println!(
        "528 sphere rays matched primitive and distance; 3 miss rays matched; {total_nodes} nodes visited"
    );
    Ok(())
}
