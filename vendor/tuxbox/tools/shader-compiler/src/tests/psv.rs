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
