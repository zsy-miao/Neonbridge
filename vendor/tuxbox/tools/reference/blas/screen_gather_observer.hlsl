// Record the first ray in each invocation. This distinguishes the compacted
// queue's ray origin, direction and distance from a shader that always misses.
struct Payload { uint words[5]; };
RWStructuredBuffer<uint4> observations : register(u0, space2);
void observe(bool hit) {
    uint index = DispatchRaysIndex().x, prior;
    if (hit) InterlockedAdd(observations[index].x, 1, prior);
    else InterlockedAdd(observations[index].y, 1, prior);
    if (prior == 0) {
        uint base = DispatchRaysDimensions().x + 2 * index;
        observations[base] = asuint(float4(WorldRayOrigin(), RayTMin()));
        observations[base+1] = asuint(float4(WorldRayDirection(), RayTCurrent()));
    }
}
[shader("miss")]
void ProbeMiss(inout Payload p) { observe(false); p.words[0] = asuint(-1.0f); }
[shader("closesthit")]
void ProbeHit(inout Payload p, BuiltInTriangleIntersectionAttributes attr) { observe(true); p.words[0] = asuint(RayTCurrent()); }
