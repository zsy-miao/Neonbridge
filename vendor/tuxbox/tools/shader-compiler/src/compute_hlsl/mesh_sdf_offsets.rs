//! Build compacted mesh-distance-field object offsets and indirect arguments.
use crate::ShaderExtract;

pub(super) fn translate(shader: &ShaderExtract) -> Option<String> {
    if shader.entry.as_deref() != Some("ComputeCulledMeshSDFObjectsStartOffsetCS")
        || shader.psv_threads != Some([64, 1, 1]) || shader.isa.len() != 528
        || shader.isa.iter().fold(0xcbf29ce484222325u64, |h, &v| {
            (h ^ u64::from(v)).wrapping_mul(0x100000001b3)
        }) != 0xc2c8e68508393e5d
        || !shader.resources.iter().map(|r| (r.res_type, r.space, r.lower, r.upper))
            .eq([(2, 0, 0, 0), (3, 0, 0, 0), (3, 0, 1, 1),
                 (6, 0, 0, 0), (6, 0, 1, 1), (6, 0, 2, 2)])
    { return None; }
    Some(SOURCE.to_string())
}

const SOURCE: &str = r#"
cbuffer Parameters : register(b0) { uint4 params[3]; };
Buffer<uint> dispatchCount : register(t0);
Buffer<uint> objectCounts : register(t1);
RWBuffer<uint> offsetsOut : register(u0);
RWBuffer<uint> totalOut : register(u1);
RWBuffer<uint> dispatchOut : register(u2);
groupshared uint values[64], blocks[8], base;

[numthreads(64,1,1)]
void main(uint lane : SV_GroupIndex, uint3 tid : SV_DispatchThreadID) {
    if (tid.x == 0u) {
        dispatchOut[0] = (dispatchCount[0] + 63u) >> 6u;
        dispatchOut[1] = 1u;
        dispatchOut[2] = 1u;
    }
    uint value = tid.x < params[2].z ? objectCounts[tid.x] : 0u;
    values[lane] = value;
    GroupMemoryBarrierWithGroupSync();
    uint prefix = 0;
    [unroll] for (uint k = 0; k < 8; ++k)
        if (k < (lane & 7u)) prefix += values[(lane & ~7u) + k];
    if ((lane & 7u) == 7u) blocks[lane >> 3u] = prefix + value;
    GroupMemoryBarrierWithGroupSync();
    if (lane == 0u) {
        uint total = 0u, start = 0u;
        [unroll] for (uint k = 0; k < 8; ++k) {
            uint n = blocks[k]; blocks[k] = total; total += n;
        }
        if (tid.x < params[2].z) InterlockedAdd(totalOut[0], total, start);
        base = start;
    }
    GroupMemoryBarrierWithGroupSync();
    if (tid.x < params[2].z) offsetsOut[tid.x] = base + blocks[lane >> 3u] + prefix;
}
"#;
