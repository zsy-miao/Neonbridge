// Appended after rtz_triangle.hlsl and inline219_candidate.hlsl by the control.
StructuredBuffer<float4> tb_probe_origins : register(t3, space127);
RWStructuredBuffer<uint4> tb_probe_results : register(u1, space127);

[numthreads(1, 1, 1)]
void main(uint3 id : SV_DispatchThreadID) {
    RayDesc ray = {tb_probe_origins[id.x].xyz, 0, float3(0, 0, 1), 2};
    RayQuery<RAY_FLAG_FORCE_NON_OPAQUE> q;
    q.TraceRayInline(tb_companion, RAY_FLAG_NONE, 0xff, ray);
    uint count = 0;
    while (q.Proceed()) {
        ++count;
        float distance;
        if (tb_candidate_219(q, 0, 2, 0, distance))
            q.CommitProceduralPrimitiveHit(distance);
    }
    uint status = q.CommittedStatus();
    tb_probe_results[id.x] = uint4(count, status, status ? asuint(q.CommittedRayT()) : 0,
                                  status ? (q.CommittedInstanceID() << 16) | q.CommittedPrimitiveIndex() : 0);
}
