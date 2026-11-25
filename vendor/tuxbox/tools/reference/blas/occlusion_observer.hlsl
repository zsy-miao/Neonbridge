// Count callbacks reached by the real ray generator at each dispatch coordinate.
struct Payload { uint words[16]; };
RWStructuredBuffer<uint4> callbacks : register(u0, space2);

#if defined(TRACE_PROBE_INPUTS) || defined(TRACE_CARD_INPUTS)
void observe_input(uint index, float end) {
    uint count = DispatchRaysDimensions().x * DispatchRaysDimensions().y;
    callbacks[count + index] = asuint(float4(WorldRayOrigin(), RayTMin()));
    callbacks[2 * count + index] = asuint(float4(WorldRayDirection(), end));
}
#endif

#ifdef TRACE_CARD_INPUTS
// One invocation owns each dispatch coordinate. Record both callback inputs
// in execution order, independently of whether they hit or miss.
void observe_card_event(uint index, uint ordinal, float end, bool hit) {
    if (ordinal >= 2) return;
    uint count = DispatchRaysDimensions().x * DispatchRaysDimensions().y;
    callbacks[(5 + 2 * ordinal) * count + index] = asuint(float4(WorldRayOrigin(), RayTMin()));
    callbacks[(6 + 2 * ordinal) * count + index] = asuint(float4(WorldRayDirection(), end));
    callbacks[9 * count + index][ordinal] = hit ? 1 : 2;
}
#endif

[shader("miss")]
void ProbeMiss(inout Payload p) {
    uint2 xy = DispatchRaysIndex().xy;
    uint index = xy.y * DispatchRaysDimensions().x + xy.x;
    uint previous;
    InterlockedAdd(callbacks[index].y, 1, previous);
#ifdef TRACE_CARD_INPUTS
    observe_card_event(index, previous + callbacks[index].x, RayTCurrent(), false);
#endif
#if defined(TRACE_PROBE_INPUTS) || defined(TRACE_CARD_INPUTS)
    if (previous == 0) observe_input(index, RayTCurrent());
#endif
    if (previous == 0) {
        float3 d = WorldRayDirection();
        float3 a = abs(d);
        uint face = a.x > max(a.y, a.z) ? (d.x > 0 ? 0 : 1) :
                    a.y > a.z ? (d.y > 0 ? 2 : 3) : (d.z > 0 ? 4 : 5);
        callbacks[index].w = face;
    }
    p.words[0] = asuint(-1.0);
}

[shader("closesthit")]
void ProbeHit(inout Payload p, BuiltInTriangleIntersectionAttributes attributes) {
    uint2 xy = DispatchRaysIndex().xy;
    uint index = xy.y * DispatchRaysDimensions().x + xy.x;
    uint previous;
    InterlockedAdd(callbacks[index].x, 1, previous);
#ifdef TRACE_CARD_INPUTS
    observe_card_event(index, previous + callbacks[index].y, RayTCurrent(), true);
    if (previous == 0) {
        uint count = DispatchRaysDimensions().x * DispatchRaysDimensions().y;
        callbacks[3 * count + index] = asuint(float4(WorldRayOrigin(), RayTMin()));
        callbacks[4 * count + index] = asuint(float4(WorldRayDirection(), RayTCurrent()));
    }
#elif defined(TRACE_PROBE_INPUTS)
    if (previous == 0) observe_input(index, RayTCurrent());
#endif
    p.words[0] = asuint(RayTCurrent());
    if (previous == 0) callbacks[index].z = p.words[0];
}
