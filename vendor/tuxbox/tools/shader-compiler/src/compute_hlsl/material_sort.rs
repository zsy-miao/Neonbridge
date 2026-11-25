//! In-place material sorting with a 128-bin histogram. Each guest invocation
//! keeps up to eight records while the group calculates the bin offsets.
use crate::ShaderExtract;

pub(super) fn translate(shader: &ShaderExtract) -> Option<String> {
    if shader.entry.as_deref() != Some("MaterialSortLocal")
        || !shader.resources.iter().map(|r| (r.res_type, r.space, r.lower, r.upper))
            .eq([(2, 0, 0, 0), (8, 0, 0, 0)])
    {
        return None;
    }
    let [threads, 1, 1] = shader.psv_threads? else { return None };
    let hash = shader.isa.iter().fold(0xcbf29ce484222325u64, |hash, &byte| {
        (hash ^ u64::from(byte)).wrapping_mul(0x100000001b3)
    });
    let records_per_thread = match (threads, shader.isa.len(), hash) {
        (256, 1008, 0x915d83ded0726a3b) => 1,
        (256, 1156, 0x667c4bf10cbe2aa5) | (512, 1156, 0xd2c90a0b82745a36) => 2,
        (512, 1332, 0x80c77436c222e58d) => 4,
        (512, 1652, 0xd05b8f6e9fbb288a) => 8,
        _ => return None,
    };
    Some(SOURCE.replace("GROUP_SIZE", &threads.to_string())
        .replace("BATCH_SIZE", &records_per_thread.to_string()))
}

const SOURCE: &str = r#"
cbuffer Parameters : register(b0) { uint recordCount; };
RWStructuredBuffer<uint3> records : register(u0);
groupshared uint bins[128], blockTotals[8];

[numthreads(GROUP_SIZE, 1, 1)]
void main(uint lane : SV_GroupIndex, uint3 group : SV_GroupID) {
    if (lane < 128u) bins[lane] = 0u;
    GroupMemoryBarrierWithGroupSync();
    uint base = group.x * (GROUP_SIZE * BATCH_SIZE);
    uint3 values[BATCH_SIZE];
    uint ranks[BATCH_SIZE];
    [unroll] for (uint i = 0; i < BATCH_SIZE; ++i) {
        uint index = base + i * GROUP_SIZE + lane;
        if (index < recordCount) {
            values[i] = records[index];
            InterlockedAdd(bins[values[i].y & 127u], 1u, ranks[i]);
        }
    }
    // The UAV holds both input and output. All records in the group must be
    // loaded before any lane can overwrite a different lane's input.
    AllMemoryBarrierWithGroupSync();
    uint prefix = 0u;
    if (lane < 128u) {
        uint first = lane & ~15u;
        for (uint j = first; j < lane; ++j) prefix += bins[j];
        if ((lane & 15u) == 15u) blockTotals[lane >> 4] = prefix + bins[lane];
    }
    GroupMemoryBarrierWithGroupSync();
    if (lane < 128u) {
        for (uint j = 0; j < (lane >> 4); ++j) prefix += blockTotals[j];
        bins[lane] = prefix;
    }
    GroupMemoryBarrierWithGroupSync();
    [unroll] for (uint i = 0; i < BATCH_SIZE; ++i) {
        if (base + i * GROUP_SIZE + lane < recordCount) {
            records[base + bins[values[i].y & 127u] + ranks[i]] = values[i];
        }
    }
}
"#;
