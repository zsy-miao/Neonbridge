//! Compact changed distance-field mip requests into the streaming queue.
use crate::ShaderExtract;

pub(super) fn translate(shader: &ShaderExtract) -> Option<String> {
    if shader.entry.as_deref() != Some("GenerateDistanceFieldAssetStreamingRequestsCS")
        || shader.psv_threads != Some([64, 1, 1])
        || shader.isa.len() != 336
        || shader.isa.iter().fold(0xcbf29ce484222325u64, |h, &v| {
            (h ^ u64::from(v)).wrapping_mul(0x100000001b3)
        }) != 0x95ce71deb19dc8bd
        || !shader.resources.iter().map(|r| (r.res_type, r.space, r.lower, r.upper))
            .eq([(2, 0, 0, 0), (5, 0, 0, 0), (5, 0, 1, 1), (8, 0, 0, 0)])
    {
        return None;
    }
    Some(SOURCE.to_string())
}

const SOURCE: &str = r#"
cbuffer Parameters : register(b0) { uint4 params[12]; };
StructuredBuffer<uint> assets : register(t0);
StructuredBuffer<uint> requests : register(t1);
RWStructuredBuffer<uint> outputQueue : register(u0);
groupshared uint selected[64], offsets[8], base;

[numthreads(64,1,1)]
void main(uint lane : SV_GroupIndex, uint3 tid : SV_DispatchThreadID) {
    uint wanted = 0, prior = 0;
    if (tid.x < params[11].x) {
        prior = assets[tid.x * 9u] >> 30u;
        wanted = requests[tid.x];
    }
    uint hit = uint(prior != 0u && wanted != 0u && prior != wanted);
    selected[lane] = hit;
    GroupMemoryBarrierWithGroupSync();
    uint prefix = 0;
    [unroll] for (uint k = 0; k < 8; ++k)
        if (k < (lane & 7u)) prefix += selected[(lane & ~7u) + k];
    if ((lane & 7u) == 7u) offsets[lane >> 3u] = prefix + hit;
    GroupMemoryBarrierWithGroupSync();
    if (lane == 0u) {
        uint total = 0u, start = 0u;
        [unroll] for (uint k = 0; k < 8; ++k) {
            uint n = offsets[k]; offsets[k] = total; total += n;
        }
        if (total != 0u) InterlockedAdd(outputQueue[0], total, start);
        base = start;
    }
    GroupMemoryBarrierWithGroupSync();
    uint index = base + offsets[lane >> 3u] + prefix;
    if (hit != 0u && index < params[11].y) {
        outputQueue[2u * index + 1u] = tid.x;
        outputQueue[2u * index + 2u] = wanted;
    }
}
"#;
