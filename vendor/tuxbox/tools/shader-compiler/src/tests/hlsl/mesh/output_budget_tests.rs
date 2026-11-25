use super::mesh_vertex_budget;

#[test]
fn budget_matches_the_measured_dxc_limit() {
    // Twenty vertex elements passed DXC at 96 vertices and failed at 97.
    assert_eq!(mesh_vertex_budget(20, 0, 128), 96);
    // Twenty-two elements pack into twenty rows, so DXC accepts 96 as well.
    // Counting one row per element gives a safe, lower bound.
    assert_eq!(mesh_vertex_budget(22, 0, 128), 87);
    assert!(mesh_vertex_budget(22, 0, 256) <= 96);
}

#[test]
fn small_vertices_keep_the_launch_count() {
    assert_eq!(mesh_vertex_budget(8, 0, 128), 128);
    assert_eq!(mesh_vertex_budget(8, 0, 256), 240);
}

#[test]
fn primitive_attributes_reduce_the_vertex_budget() {
    // 128 primitives with indices plus 2 attributes: 6144 bytes, leaving
    // 26624 bytes of 8-row (128 byte) vertices.
    assert_eq!(mesh_vertex_budget(8, 2, 256), 208);
}

#[test]
fn nothing_fits_when_primitives_use_the_budget() {
    assert_eq!(mesh_vertex_budget(8, 15, 128), 0);
}
