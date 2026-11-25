// Limited two-segment hypothesis for the controlled direct-light fixture.
// This is not a translation of packaged compute shader 219.
RaytracingAccelerationStructure scene : register(t0);
StructuredBuffer<float4> bounds : register(t1);
RWStructuredBuffer<uint4> result : register(u0);

bool trace(float minimum, float maximum, float origin_x, float origin_z) {
    RayDesc ray = {float3(origin_x, 0, origin_z), minimum, float3(0, 0, 1), maximum};
    RayQuery<RAY_FLAG_FORCE_OPAQUE | RAY_FLAG_ACCEPT_FIRST_HIT_AND_END_SEARCH> query;
    query.TraceRayInline(scene, RAY_FLAG_NONE, 0xff, ray);
    while (query.Proceed()) {}
    return query.CommittedStatus() == COMMITTED_TRIANGLE_HIT;
}

// The wider interval lets software apply 219's inclusive near / exclusive far
// candidate comparison. A native query can still suppress t=0 before Proceed;
// this experimental control does not establish behavior for that case.
bool filtered_trace(float minimum, float maximum, float origin_x, float origin_z, out bool rejected) {
    rejected = false;
    RayDesc ray = {float3(origin_x, 0, origin_z), 0, float3(0, 0, 1), maximum};
    RayQuery<RAY_FLAG_FORCE_NON_OPAQUE> query;
    query.TraceRayInline(scene, RAY_FLAG_NONE, 0xff, ray);
    while (query.Proceed()) {
        if (query.CandidateType() == CANDIDATE_NON_OPAQUE_TRIANGLE) {
            float distance = query.CandidateTriangleRayT();
            if (distance >= minimum && distance < maximum)
                query.CommitNonOpaqueTriangleHit();
            else
                rejected = true;
        }
    }
    return query.CommittedStatus() == COMMITTED_TRIANGLE_HIT;
}

[numthreads(1, 1, 1)]
void main(uint3 id : SV_DispatchThreadID) {
    float4 interval = bounds[id.x];
    bool first = trace(interval.x, interval.y, interval.z, interval.w);
    bool second = false;
    if (!first) second = trace(0, interval.x, interval.z, interval.w);
    bool rejected_first, rejected_second;
    bool filtered_first = filtered_trace(interval.x, interval.y, interval.z, interval.w, rejected_first);
    bool filtered_second = false;
    if (!filtered_first) filtered_second = filtered_trace(0, interval.x, interval.z, interval.w, rejected_second);
    result[id.x] = uint4(first, second, 2 | uint(first || second),
                         uint(filtered_first) | (uint(filtered_second) << 1) | (uint(rejected_first) << 2));
}
