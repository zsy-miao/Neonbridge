//! Compact selected five-word hardware-ray records into a shared output queue.
use crate::ShaderExtract;

pub(super) fn translate(shader: &ShaderExtract) -> Option<String> {
    if shader.entry.as_deref() != Some("FLumenHWRTCompactRaysCS")
        || shader.psv_threads != Some([64, 1, 1])
        || !matches!(shader.isa.len(), 528 | 556)
        || !shader.resources.iter().map(|r| (r.res_type, r.space, r.lower, r.upper))
            .eq([(3, 0, 0, 0), (5, 0, 1, 1), (6, 0, 0, 0), (8, 0, 1, 1)])
    { return None; }
    let hash = shader.isa.iter().fold(0xcbf29ce484222325u64, |h, &v| {
        (h ^ u64::from(v)).wrapping_mul(0x100000001b3)
    });
    let (mask, wanted) = match hash {
        0x4319b5c62e715557 if shader.isa.len() == 528 => (0, 0),
        0x9b31f6d00401f923 if shader.isa.len() == 556 => (1, 1),
        0xfc1f92667d62b8a1 if shader.isa.len() == 556 => (1, 0),
        0xad5134af0527f210 if shader.isa.len() == 556 => (3, 1),
        _ => return None,
    };
    Some(SOURCE.replace("MASK", &format!("{mask}u")).replace("WANTED", &format!("{wanted}u")))
}

const SOURCE: &str = r#"
struct Ray { uint words[5]; };
Buffer<uint> counts : register(t0);
StructuredBuffer<Ray> inputRays : register(t1);
RWBuffer<uint> countOut : register(u0);
RWStructuredBuffer<Ray> outputRays : register(u1);
groupshared uint selected[64], offsets[8], base;

[numthreads(64,1,1)]
void main(uint lane : SV_GroupIndex, uint3 tid : SV_DispatchThreadID) {
    uint hit = 0;
    if (tid.x < counts[lane]) hit = uint((inputRays[tid.x].words[2] & MASK) == WANTED);
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
        [unroll] for (uint i = 0; i < 5; ++i)
            outputRays[index].words[i] = inputRays[tid.x].words[i];
    }
}
"#;
