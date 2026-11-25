//! Offline graph-to-directory control for user-supplied serialized BLAS bytes.
//! This does not publish resources or integrate with the runtime provider.
#[path = "../../../runtime/d3d12/src/device/blas/sidecar_directory.rs"]
mod directory;
#[path = "../../../runtime/d3d12/src/commands/serialized_blas.rs"]
mod serialized_blas;
#[path = "validate_traversal.rs"]
mod validate_traversal;

use directory::{Candidate, InstanceRange, encode};
use std::{collections::BTreeMap, env, fs, path::Path};

fn word(data: &[u8], at: usize) -> Result<u32, String> {
    let bytes = data
        .get(at..at.checked_add(4).ok_or("offset overflow")?)
        .ok_or("short word")?;
    Ok(u32::from_le_bytes(bytes.try_into().unwrap()))
}

fn run(serialized: &[u8], output: &Path) -> Result<(), String> {
    validate_traversal::validate(serialized).map_err(|e| format!("graph validation: {e:?}"))?;
    let triangles = serialized_blas::decode(serialized).map_err(|e| format!("decoder: {e:?}"))?;
    let mut expected: BTreeMap<_, _> = triangles
        .into_iter()
        .map(|t| ((t.geometry, t.primitive), t.vertices))
        .collect();
    let body = serialized.get(64..).ok_or("missing body")?;
    let mut pending = vec![word(body, 48)?];
    let mut candidates = Vec::new();
    let mut records = Vec::new();
    while let Some(pointer) = pending.pop() {
        let at = ((pointer >> 3) as usize)
            .checked_mul(64)
            .ok_or("node overflow")?;
        match pointer & 7 {
            4 | 5 => {
                for i in 0..4 {
                    let child = word(body, at + i * 4)?;
                    if child != u32::MAX {
                        pending.push(child);
                    }
                }
            }
            0 => {
                let descriptor = word(body, at + 48)?;
                for side in 0..=u32::from(descriptor & 0x80 != 0) {
                    let primitive = word(body, at + 52 + side as usize * 4)?;
                    let geometry = descriptor >> 8;
                    let vertices = expected
                        .remove(&(geometry, primitive))
                        .ok_or("missing or repeated decoded primitive")?;
                    let leaf_offset = u32::try_from(at).map_err(|_| "leaf offset overflow")?;
                    candidates.push(Candidate { leaf_offset, side });
                    for w in [geometry, primitive, leaf_offset, side] {
                        records.extend_from_slice(&w.to_le_bytes());
                    }
                    for vertex in vertices {
                        for value in vertex {
                            records.extend_from_slice(&value.to_bits().to_le_bytes());
                        }
                    }
                }
            }
            _ => return Err("unsupported node type".into()),
        }
    }
    if !expected.is_empty() || candidates.is_empty() {
        return Err("unmapped decoded primitives".into());
    }
    let count = u32::try_from(candidates.len()).map_err(|_| "too many candidates")?;
    let directory = encode(body, &[InstanceRange { first: 0, count }], &candidates)
        .map_err(|e| format!("directory: {e:?}"))?;
    fs::write(output.join("sphere-sidecar-body.bin"), body).map_err(|e| e.to_string())?;
    fs::write(output.join("sphere-sidecar-directory.bin"), directory).map_err(|e| e.to_string())?;
    fs::write(output.join("sphere-sidecar-expected.bin"), records).map_err(|e| e.to_string())?;
    println!("candidates={count}");
    Ok(())
}

fn main() {
    let mut args = env::args_os().skip(1);
    let Some(input) = args.next() else {
        eprintln!("usage: sphere_sidecar INPUT OUTPUT_DIR");
        std::process::exit(2)
    };
    let Some(output) = args.next() else {
        eprintln!("missing output directory");
        std::process::exit(2)
    };
    if args.next().is_some() {
        eprintln!("unexpected argument");
        std::process::exit(2);
    }
    let result = fs::read(input)
        .map_err(|e| e.to_string())
        .and_then(|bytes| run(&bytes, Path::new(&output)));
    if let Err(error) = result {
        eprintln!("{error}");
        std::process::exit(1);
    }
}
