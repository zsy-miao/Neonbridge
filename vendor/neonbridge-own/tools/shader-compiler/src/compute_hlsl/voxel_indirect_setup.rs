//! Initialize the three-word Lumen visibility-shading dispatch arguments.
//!
//! The title allocates one 12-byte argument record and binds it as R32_UINT.
//! The native typed stores produce its three dimensions: the allocator value,
//! then two unit dimensions. The R32 view exposes only the X component of a
//! formatted multi-component store; byte-offset stores beyond the view do not
//! write the guard region.
use crate::ShaderExtract;
use crate::rootsig::RootParamKind;

pub(super) fn translate(shader: &ShaderExtract) -> Option<String> {
    if shader.entry.as_deref() != Some("SetupVisBufferShadingCS")
        || shader.psv_threads != Some([1, 1, 1])
        || shader.isa.len() != 156
        || !shader
            .resources
            .iter()
            .map(|r| (r.res_type, r.space, r.lower, r.upper))
            .eq([(5, 0, 0, 0), (6, 0, 0, 0)])
    {
        return None;
    }
    let hash = shader.isa.iter().fold(0xcbf29ce484222325u64, |h, &v| {
        (h ^ u64::from(v)).wrapping_mul(0x100000001b3)
    });
    if hash != 0x25dee4f48fb6170a {
        return None;
    }
    let root = shader.root_sig.as_ref()?;
    for (index, range_type) in [(0, 0), (3, 1)] {
        let parameter = root.params.get(index)?;
        let RootParamKind::Table(ranges) = &parameter.kind else { return None; };
        let range = ranges.first()?;
        if parameter.visibility != 0 || range.range_type != range_type || range.space != 0
            || range.base_register != 0 || range.num == 0
            || !matches!(range.offset_in_table, 0 | u32::MAX) {
            return None;
        }
    }
    Some(SOURCE.into())
}

const SOURCE: &str = r#"
Buffer<uint> dimensions : register(t0);
RWBuffer<uint> arguments : register(u0);
[numthreads(1, 1, 1)]
void main() {
    arguments[0] = dimensions[0];
    arguments[1] = 1u;
    arguments[2] = 1u;
}
"#;
