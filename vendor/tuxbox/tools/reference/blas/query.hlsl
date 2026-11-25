RaytracingAccelerationStructure scene : register(t0);
struct Probe {
    float3 origin;
    float minimum;
    float3 direction;
    float maximum;
    uint flags;
};
StructuredBuffer<Probe> probes : register(t1);
RWStructuredBuffer<uint4> results : register(u0);

[numthreads(1, 1, 1)]
void main(uint3 tid : SV_DispatchThreadID) {
    Probe probe = probes[tid.x];
    RayDesc ray = {probe.origin, probe.minimum, probe.direction, probe.maximum};
    RayQuery<RAY_FLAG_NONE> query;
    query.TraceRayInline(scene, probe.flags, 0xff, ray);
    while (query.Proceed()) {
        if (query.CandidateType() == CANDIDATE_NON_OPAQUE_TRIANGLE) {
            query.CommitNonOpaqueTriangleHit();
        }
    }
    if (query.CommittedStatus() == COMMITTED_TRIANGLE_HIT) {
        results[tid.x] = uint4(query.CommittedInstanceID(), query.CommittedGeometryIndex(),
                               query.CommittedPrimitiveIndex(), asuint(query.CommittedRayT()));
    } else {
        results[tid.x] = uint4(~0u, ~0u, ~0u, 0u);
    }
}
