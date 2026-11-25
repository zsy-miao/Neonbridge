//! Host-side control for comparing decoded vertices against the original indexed mesh.
#[path = "../../../runtime/d3d12/src/commands/serialized_blas.rs"]
mod serialized_blas;

use std::io::{self, Read, Write};

fn main() -> io::Result<()> {
    let with_ids = match std::env::args().nth(1).as_deref() {
        None => false,
        Some("--with-ids") => true,
        _ => {
            return Err(io::Error::new(
                io::ErrorKind::InvalidInput,
                "expected --with-ids",
            ));
        }
    };
    let mut bytes = Vec::new();
    io::stdin().read_to_end(&mut bytes)?;
    let triangles = serialized_blas::decode_for_build(&bytes).map_err(|e| {
        io::Error::new(
            io::ErrorKind::InvalidData,
            format!("Xbox triangle BLAS: {e:?}"),
        )
    })?;
    let mut out = io::BufWriter::new(io::stdout().lock());
    // Enumerate exactly as the host DXR builder does. Inactive padding cannot
    // intersect, but it still consumes geometry and primitive indices.
    for (geometry, group) in triangles
        .chunk_by(|a, b| a.geometry == b.geometry)
        .enumerate()
    {
        for (primitive, t) in group.iter().enumerate() {
            if t.vertices[0][0].is_nan() {
                continue;
            }
            if with_ids {
                out.write_all(&(geometry as u32).to_le_bytes())?;
                out.write_all(&(primitive as u32).to_le_bytes())?;
            }
            for v in t.vertices.into_iter().flatten() {
                out.write_all(&v.to_le_bytes())?;
            }
        }
    }
    out.flush()
}
