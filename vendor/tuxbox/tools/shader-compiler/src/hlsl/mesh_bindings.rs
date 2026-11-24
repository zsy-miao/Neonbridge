//! Mesh resource bindings and any corresponding desktop root changes.
use crate::ShaderExtract;
pub(super) use tuxbox_dxbc::mesh_table::MeshLayout as MeshBindings;

pub(super) fn read(ex: &ShaderExtract) -> Option<MeshBindings> {
    let resources: Vec<_> = ex
        .resources
        .iter()
        .map(|r| [r.res_type, r.space, r.lower, r.upper])
        .collect();
    MeshBindings::read(&ex.fssm, &resources, ex.root_sig.as_ref()?)
}
