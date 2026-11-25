struct NativeMaterialPayload {uint words[16];};
RaytracingAccelerationStructure scene : register(t0,space3);
RWStructuredBuffer<uint> result : register(u0,space3);

[shader("raygeneration")]
void MaterialProbeRay() {
    uint i = DispatchRaysIndex().x;
    bool reverse = (i & 1u) != 0;
    float3 pos = float3(i >= 4 ? 0 : i < 2 ? -1 : 3, 0, reverse ? 1 : -1);
    RayDesc ray = {pos, 0, float3(0,0,reverse ? -1 : 1), 2};
    NativeMaterialPayload p = (NativeMaterialPayload)0;
    [unroll] for (uint j = 0; j < 16; ++j) p.words[j] = 0x00001000u + j;
    p.words[9] = i < 2 ? 0u : 1u << 25;
    TraceRay(scene, RAY_FLAG_FORCE_OPAQUE, 255, 0, 0, 0, ray, p);
    [unroll] for (uint j = 0; j < 16; ++j) result[16*i+j] = p.words[j];
}

// Two non-opaque hits for each payload-mask state, one geometric miss,
// then a forced-opaque hit on each instance to exercise the same closest-hit export.
void traceMaskedMaterial(bool localOpacity) {
    uint i = DispatchRaysIndex().x;
    bool back = (i & 1u) != 0u;
    float x = i == 4 ? 0 : (i < 2 || i == 5 ? -1 : 3);
    if (localOpacity && i < 4) x = -1;
    RayDesc ray = {float3(x, 0, back ? 1 : -1), 0, float3(0, 0, back ? -1 : 1), 2};
    NativeMaterialPayload p;
    [unroll] for (uint j = 0; j < 16; ++j) p.words[j] = 0x1000u + j;
    p.words[9] = !localOpacity && (i == 2 || i == 3) ? 1u << 26 : 0u;
    uint flags = i < 5 ? RAY_FLAG_FORCE_NON_OPAQUE : RAY_FLAG_FORCE_OPAQUE;
    uint material = localOpacity && (i == 2 || i == 3 || i == 6) ? 1 : 0;
    TraceRay(scene, flags, 255, material, 0, 0, ray, p);
    [unroll] for (uint j = 0; j < 16; ++j) result[i * 16 + j] = p.words[j];
}

[shader("raygeneration")]
void MaskedMaterialProbeRay() { traceMaskedMaterial(false); }

// Both non-opaque record variants trace the same triangle from either face.
[shader("raygeneration")]
void LocalOpacityProbeRay() { traceMaskedMaterial(true); }

[shader("miss")]
void MaterialProbeMiss(inout NativeMaterialPayload p) {
    p.words[0] = asuint(-1.0);
}
