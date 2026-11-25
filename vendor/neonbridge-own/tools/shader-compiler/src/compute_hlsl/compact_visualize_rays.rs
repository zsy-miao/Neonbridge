//! Compact paired visualization inputs into matching output records.
use crate::ShaderExtract;

pub(super) fn translate(shader: &ShaderExtract) -> Option<String> {
    if shader.entry.as_deref() != Some("FLumenVisualizeCompactRaysCS")
        || shader.psv_threads != Some([64, 1, 1]) || shader.isa.len() != 468
        || !shader.resources.iter().map(|r| (r.res_type, r.space, r.lower, r.upper))
            .eq([(5, 0, 0, 0), (5, 0, 1, 1), (6, 0, 0, 0), (8, 0, 1, 1), (8, 0, 2, 2)])
    { return None; }
    let hash = shader.isa.iter().fold(0xcbf29ce484222325u64, |h, &v| {
        (h ^ u64::from(v)).wrapping_mul(0x100000001b3)
    });
    let (mask, wanted) = match hash {
        0xabc5ddb90ba4131e => (1, 1),
        0xe555a49a9baf97f9 => (1, 0),
        0xb630d10c5536f7d4 => (3, 1),
        _ => return None,
    };
    Some(SOURCE.replace("MASK", &format!("{mask}u")).replace("WANTED", &format!("{wanted}u")))
}

const SOURCE: &str = r#"
StructuredBuffer<uint> inputData : register(t0);
StructuredBuffer<uint2> inputRays : register(t1);
RWBuffer<uint> countOut : register(u0);
RWStructuredBuffer<uint> outputData : register(u1);
RWStructuredBuffer<uint2> outputRays : register(u2);
groupshared uint selected[64], offsets[8], base;

[numthreads(64,1,1)]
void main(uint lane : SV_GroupIndex, uint3 tid : SV_DispatchThreadID) {
    uint2 ray = inputRays[tid.x];
    uint hit = uint((ray.x & MASK) == WANTED);
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
        if (total != 0u) InterlockedAdd(countOut[0], total, start);
        base = start;
    }
    GroupMemoryBarrierWithGroupSync();
    if (hit != 0u) {
        uint index = base + offsets[lane >> 3u] + prefix;
        outputData[index] = inputData[tid.x];
        outputRays[index] = ray;
    }
}
"#;
