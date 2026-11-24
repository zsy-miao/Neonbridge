//! Shader stage and binding metadata from the DirectX pipeline validation part.
//! Layouts follow [Microsoft's versioned PSV definitions](https://github.com/microsoft/DirectXShaderCompiler/blob/main/include/dxc/DxilContainer/DxilPipelineStateValidation.h).

#[derive(Clone, Copy, Debug)]
pub struct GsInfo {
    pub input_primitive: u32,
    pub output_topology: u32,
    pub max_vertex_count: u16,
}

#[derive(Clone, Copy, Debug)]
pub struct MeshInfo {
    pub vertices: u16,
    pub primitives: u16,
    pub topology: u8,
    pub threads: [u32; 3],
    pub payload: u32,
    pub shared_bytes: u32,
}

#[derive(Clone, Debug)]
pub struct PsvResource {
    pub res_type: u32,
    pub space: u32,
    pub lower: u32,
    pub upper: u32,
}

fn word(bytes: &[u8], offset: usize) -> Option<u32> {
    Some(u32::from_le_bytes(*bytes.get(offset..)?.first_chunk()?))
}

// Later PSV versions append fields to the runtime info. The binding table
// starts at the declared end even when the producer uses a newer version.
fn sections(body: &[u8]) -> Option<(&[u8], &[u8])> {
    let size = word(body, 0)? as usize;
    if size < 24 {
        return None;
    }
    body.get(4..)?.split_at_checked(size)
}

fn thread_group(runtime: &[u8]) -> Option<[u32; 3]> {
    let xyz = [word(runtime, 36)?, word(runtime, 40)?, word(runtime, 44)?];
    (!xyz.contains(&0)).then_some(xyz)
}

pub use tuxbox_dxbc::compute_threads as parse_compute_threads;

pub fn parse_psv0_gs(body: &[u8]) -> Option<GsInfo> {
    let (runtime, _) = sections(body)?;
    if runtime.get(24) != Some(&2) {
        return None;
    }
    Some(GsInfo {
        input_primitive: word(runtime, 0)?,
        output_topology: word(runtime, 4)?,
        max_vertex_count: u16::from_le_bytes(*runtime.get(26..)?.first_chunk()?),
    })
}

pub fn parse_mesh_info(body: &[u8]) -> Option<MeshInfo> {
    let (runtime, _) = sections(body)?;
    if runtime.get(24) != Some(&13) {
        return None;
    }
    Some(MeshInfo {
        vertices: u16::from_le_bytes(*runtime.get(12..)?.first_chunk()?),
        primitives: u16::from_le_bytes(*runtime.get(14..)?.first_chunk()?),
        topology: *runtime.get(27)?,
        threads: thread_group(runtime)?,
        payload: word(runtime, 8)?,
        shared_bytes: word(runtime, 0)?,
    })
}

pub fn parse_psv0(body: &[u8]) -> Result<Vec<PsvResource>, String> {
    let (_, table) = sections(body).ok_or("invalid PSV runtime info length")?;
    let count = word(table, 0).ok_or("missing PSV resource count")? as usize;
    if count == 0 {
        return Ok(Vec::new());
    }
    let stride = word(table, 4)
        .filter(|&n| n >= 16)
        .ok_or("invalid PSV binding stride")? as usize;
    let bytes = count
        .checked_mul(stride)
        .ok_or("PSV binding table size overflow")?;
    let records = table
        .get(8..)
        .and_then(|data| data.get(..bytes))
        .ok_or("truncated PSV binding table")?;
    records
        .chunks_exact(stride)
        .map(|row| {
            let fields = row
                .first_chunk::<16>()
                .ok_or("truncated PSV binding")?
                .as_chunks::<4>()
                .0;
            Ok(PsvResource {
                res_type: u32::from_le_bytes(fields[0]),
                space: u32::from_le_bytes(fields[1]),
                lower: u32::from_le_bytes(fields[2]),
                upper: u32::from_le_bytes(fields[3]),
            })
        })
        .collect()
}

#[cfg(test)]
mod tests {
    use super::*;

    fn body(runtime: &[u8], table: &[u32]) -> Vec<u8> {
        let mut bytes = (runtime.len() as u32).to_le_bytes().to_vec();
        bytes.extend_from_slice(runtime);
        bytes.extend(table.iter().flat_map(|v| v.to_le_bytes()));
        bytes
    }

    #[test]
    fn newer_runtime_and_resource_strides_preserve_binding_ranges() {
        let data = body(&[0; 60], &[2, 20, 2, 7, 4, 9, 123, 1, 12, 0, u32::MAX, 456]);
        let bindings = parse_psv0(&data).unwrap();
        assert_eq!((bindings[0].space, bindings[0].lower), (7, 4));
        assert_eq!((bindings[1].res_type, bindings[1].upper), (1, u32::MAX));
        assert!(parse_psv0(&data[..data.len() - 1]).is_err());
        assert!(parse_psv0(&body(&[0; 24], &[0])).unwrap().is_empty());
        assert!(parse_psv0(&body(&[0; 24], &[u32::MAX, u32::MAX])).is_err());
    }

    #[test]
    fn dimensions_and_geometry_limits_belong_to_the_declared_stage() {
        let mut info = [0u8; 48];
        info[24] = 13;
        info[0..4].copy_from_slice(&512u32.to_le_bytes());
        info[8..12].copy_from_slice(&32u32.to_le_bytes());
        info[12..16].copy_from_slice(&[10, 0, 7, 0]);
        info[27] = 2;
        for (field, n) in info[36..48].chunks_exact_mut(4).zip([8u32, 4, 1]) {
            field.copy_from_slice(&n.to_le_bytes());
        }
        let data = body(&info, &[0]);
        let mesh = parse_mesh_info(&data).unwrap();
        assert_eq!((mesh.vertices, mesh.primitives, mesh.topology), (10, 7, 2));
        assert_eq!(
            (mesh.threads, mesh.payload, mesh.shared_bytes),
            ([8, 4, 1], 32, 512)
        );
        assert!(parse_compute_threads(&data).is_none());
        info[24] = 5;
        assert_eq!(parse_compute_threads(&body(&info, &[0])), Some([8, 4, 1]));
        assert!(parse_compute_threads(&body(&info[..36], &[8, 4, 1])).is_none());
        info[24] = 2;
        info[26..28].copy_from_slice(&72u16.to_le_bytes());
        assert_eq!(
            parse_psv0_gs(&body(&info[..36], &[0]))
                .unwrap()
                .max_vertex_count,
            72
        );
    }
}
