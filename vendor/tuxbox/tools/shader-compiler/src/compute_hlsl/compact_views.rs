//! Compact the valid per-mip views of a virtual shadow map. A 64-lane Xbox
//! wave reserves space for both its nonempty views and all their selected mips.
use crate::ShaderExtract;

pub(super) fn translate(shader: &ShaderExtract) -> Option<String> {
    if shader.entry.as_deref() != Some("CompactViewsVSM_CS")
        || shader.psv_threads != Some([64, 1, 1])
        || shader.isa.len() != 1868
        || shader.isa.iter().fold(0xcbf29ce484222325u64, |h, &v| {
            (h ^ u64::from(v)).wrapping_mul(0x100000001b3)
        }) != 0x027347a1ed8fb2df
        || !shader.resources.iter().map(|r| (r.res_type, r.space, r.lower, r.upper))
            .eq([(2, 0, 0, 0), (5, 0, 0, 0), (5, 0, 1, 1),
                 (8, 0, 0, 0), (8, 0, 1, 1), (8, 0, 2, 2)])
    {
        return None;
    }
    Some(SOURCE.to_string())
}

const SOURCE: &str = r#"
struct View { uint4 words[50]; };
cbuffer Parameters : register(b0) { uint4 params[3]; };
StructuredBuffer<uint4> rects : register(t0);
StructuredBuffer<View> views : register(t1);
RWStructuredBuffer<View> selectedViews : register(u0);
RWStructuredBuffer<uint2> viewRanges : register(u1);
RWStructuredBuffer<uint> counters : register(u2);
groupshared uint2 laneWeights[64], blockOffsets[8], groupBase;

bool validMip(uint view) {
    uint2 rectId = views[view].words[48].xy;
    uint4 bounds = rects[8u * rectId.x + rectId.y];
    return all(bounds.zw >= bounds.xy);
}

[numthreads(64,1,1)]
void main(uint lane : SV_GroupIndex, uint3 tid : SV_DispatchThreadID) {
    uint viewCount = params[2].y;
    uint mips = tid.x < viewCount ? views[tid.x].words[48].z : 0u;
    uint valid = 0u;
    [loop] for (uint mip = 0; mip < mips; ++mip)
        valid += uint(validMip(tid.x + mip * viewCount));
    uint2 weight = uint2(valid, valid != 0u);
    laneWeights[lane] = weight;
    GroupMemoryBarrierWithGroupSync();

    uint2 prefix = 0u;
    [unroll] for (uint k = 0; k < 8u; ++k)
        if (k < (lane & 7u)) prefix += laneWeights[(lane & ~7u) + k];
    if ((lane & 7u) == 7u) blockOffsets[lane >> 3] = prefix + weight;
    GroupMemoryBarrierWithGroupSync();

    if (lane == 0u) {
        uint2 total = 0u, first = 0u;
        [unroll] for (uint k = 0; k < 8u; ++k) {
            uint2 size = blockOffsets[k];
            blockOffsets[k] = total;
            total += size;
        }
        if (total.y != 0u) {
            InterlockedAdd(counters[1], total.x, first.x);
            InterlockedAdd(counters[0], total.y, first.y);
        }
        groupBase = first;
    }
    GroupMemoryBarrierWithGroupSync();

    if (valid != 0u) {
        uint2 first = groupBase + blockOffsets[lane >> 3] + prefix;
        viewRanges[first.y] = uint2(first.x, valid);
        [loop] for (uint mip = 0; mip < mips; ++mip) {
            uint index = tid.x + mip * viewCount;
            if (validMip(index)) {
                [unroll] for (uint k = 0; k < 50u; ++k)
                    selectedViews[first.x].words[k] = views[index].words[k];
                ++first.x;
            }
        }
    }
}
"#;
