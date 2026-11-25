// Procedural broad phase plus experimentally validated TRI0 arithmetic.
// This is not a complete Xbox triangle or shader 219 translation.
#define TUXBOX_RTZ_FLOAT32
#include "../bvh-node/rtz_triangle.hlsl"
RaytracingAccelerationStructure scene : register(t0);
StructuredBuffer<float4> origins : register(t1);
ByteAddressBuffer nodes : register(t2);
ByteAddressBuffer directory : register(t3);
RWStructuredBuffer<uint4> results : register(u0);

[numthreads(1, 1, 1)]
void main(uint3 id : SV_DispatchThreadID) {
    RayDesc ray = {origins[id.x].xyz, 0, float3(0, 0, 1), 2};
    RayQuery<RAY_FLAG_NONE> query;
    query.TraceRayInline(scene, RAY_FLAG_NONE, 0xff, ray);
    uint candidates = 0;
    while (query.Proceed()) {
        if (query.CandidateType() == CANDIDATE_PROCEDURAL_PRIMITIVE) {
            candidates++;
            uint node[32];
            uint primitive = query.CandidatePrimitiveIndex();
            uint start = 16 + directory.Load(4) * 16;
            uint2 entry = directory.Load2(start + primitive * 16);
            [unroll] for (uint i = 0; i < 16; ++i) node[i] = nodes.Load(entry.x + 4 * i);
            [unroll] for (uint j = 16; j < 32; ++j) node[j] = 0;
            float4 hit = tb_bvh_triangle_rtz(node, entry.y, query.CandidateObjectRayOrigin(),
                                             query.CandidateObjectRayDirection());
            float distance = hit.x / hit.y;
            if (isfinite(distance) && distance >= 0 && distance < 2)
                query.CommitProceduralPrimitiveHit(distance);
        }
    }
    uint status = query.CommittedStatus();
    results[id.x] = uint4(candidates, status, status ? asuint(query.CommittedRayT()) : 0,
                          status ? (query.CommittedInstanceID() << 16) | query.CommittedPrimitiveIndex() : 0);
}
