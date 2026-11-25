struct ProbePayload { uint words[16]; };
RaytracingAccelerationStructure scene : register(t0, space3);
RWStructuredBuffer<uint> result : register(u0, space3);

[shader("raygeneration")]
void LightingProbeRay() {
    uint i = DispatchRaysIndex().x;
    ProbePayload p;
    [unroll] for (uint j = 0; j < 16; ++j) p.words[j] = 0x1000u + j;
    uint sample = i >> 2;
    float3 n = 0;
    n[sample % 3u] = (sample / 3u) % 2u == 0 ? 1 : -1;
    p.words[0] = 0; // light index
    p.words[3] = (i & 2u) ? 0x34003000u : 0; // initial radiance R,G
    p.words[4] = (f32tof16(n.x) << 16) | ((i & 2u) ? 0x3800u : 0u);
    p.words[5] = f32tof16(n.y) | (f32tof16(n.z) << 16);
    p.words[6] = 0x3c003c00u; // base color
    p.words[7] = 0x38003c00u;
    p.words[8] = 0x38000000u;
    p.words[9] = 1u << 16; // default lit
    p.words[10] = f32tof16(-n.x) | (f32tof16(-n.y) << 16);
    p.words[11] = f32tof16(-n.z);
    p.words[12] = 0xffffffffu;
    [unroll] for (uint j = 0; j < 16; ++j) result[i * 32 + j] = p.words[j];
    bool hidden = i >= 24u;
    RayDesc ray = { float3(hidden ? -1 : 0, 0, -1), 0, float3(0, 0, 1), 2 };
    uint flags = (i == 24u) ? RAY_FLAG_FORCE_NON_OPAQUE : RAY_FLAG_FORCE_OPAQUE;
    TraceRay(scene, flags, 255, 0, 0, hidden ? 2u : i & 1u, ray, p);
    [unroll] for (uint j = 0; j < 16; ++j) result[i * 32 + 16 + j] = p.words[j];
}

[shader("miss")]
void HiddenProbeMiss(inout ProbePayload p) { p.words[0] = 0xfeedu; }
