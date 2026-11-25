//! Lumen update-grid tile marking and allocation for the verified program.
//! Frosty's named resources use float4 bounds, R32_UINT counter and tile
//! views, and an R8_UINT three-dimensional mask. The program is one wave64;
//! workgroup barriers make the shared result valid on wave32 hosts too.
use crate::rootsig::RootParamKind;
use crate::ShaderExtract;

pub(super) fn translate(shader: &ShaderExtract) -> Option<String> {
    if shader.entry.as_deref() != Some("BuildUpdateGridTilesCS")
        || shader.psv_threads != Some([64, 1, 1])
        || shader.isa.len() != 704
        || shader.isa.iter().fold(0xcbf29ce484222325u64, |h, &v| {
            (h ^ u64::from(v)).wrapping_mul(0x100000001b3)
        }) != 0xd7b9dcad45589d8d
        || !shader.resources.iter().map(|r| (r.res_type, r.space, r.lower, r.upper))
            .eq([(2, 0, 0, 0), (3, 0, 0, 0), (6, 0, 0, 0), (8, 0, 1, 1), (6, 0, 2, 2)])
    {
        return None;
    }
    let root = shader.root_sig.as_ref()?;
    for (index, range_type, count) in [(0, 0, 1), (1, 2, 1), (3, 1, 3)] {
        let parameter = root.params.get(index)?;
        let RootParamKind::Table(ranges) = &parameter.kind else { return None; };
        let range = ranges.first()?;
        if parameter.visibility != 0 || range.range_type != range_type || range.space != 0
            || range.base_register != 0 || range.num < count
            || !matches!(range.offset_in_table, 0 | u32::MAX) {
            return None;
        }
    }
    Some(SOURCE.into())
}

const SOURCE: &str = r#"
cbuffer Params : register(b0) { uint4 cb[8]; }
Buffer<float4> bounds : register(t0);
RWBuffer<uint> counter : register(u0);
RWBuffer<uint> tiles : register(u1);
RWTexture3D<uint> mask : register(u2);
groupshared uint hit;
[numthreads(64,1,1)]
void main(uint lane : SV_GroupIndex, uint3 group : SV_GroupID) {
    if (lane == 0) hit = 0;
    GroupMemoryBarrierWithGroupSync();
    float3 pos = mad(asfloat(cb[5].xyz), float3(group), asfloat(cb[6].xyz));
    for (uint i = lane; i < cb[3].x; i += 64) {
        precise float3 dist = max(0.0f, (-bounds[i*2+1].xyz - asfloat(cb[7].xyz)) + abs(pos - bounds[i*2].xyz));
        precise float square = dist.y * dist.y;
        square = mad(dist.x, dist.x, square);
        square = mad(dist.z, dist.z, square);
        if (0.0f >= square) { InterlockedOr(hit, 1); break; }
    }
    GroupMemoryBarrierWithGroupSync();
    if (lane == 0 && all(group < cb[4].xyz)) {
        if (hit != 0) { uint prev; InterlockedAdd(counter[0], 1, prev); tiles[prev] = group.x | (group.y << 8) | (group.z << 16); }
        mask[group] = hit;
    }
}
"#;
