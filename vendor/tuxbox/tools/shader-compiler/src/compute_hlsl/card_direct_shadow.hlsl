// Card direct-light shadow tiles, from the Series compute program using an 8x4 group.
cbuffer DispatchConstants : register(b0) { uint4 dispatchWords[35]; }
cbuffer AtlasConstants : register(b1) { uint4 atlasWords[2]; }
cbuffer LightConstants : register(b2) { uint4 lightWords[6]; }
SamplerState atlasSampler : register(s0);
StructuredBuffer<uint4> cards : register(t0);
StructuredBuffer<uint4> pages : register(t1);
Texture2D<float4> normalAtlas : register(t2);
Texture2D<float4> depthAtlas : register(t3);
StructuredBuffer<uint> queueCounts : register(t4);
StructuredBuffer<uint> packedTiles : register(t6);
RWStructuredBuffer<uint> shadowMask : register(u0);
// The runtime resolves the guest raw t5 view to its corresponding built TLAS.
RaytracingAccelerationStructure scene : register(t0, space1);

bool shadowSegment(float3 origin, float3 direction, float nearT, float farT, uint mask) {
    if (!(farT > max(nearT, 0.0))) return false;
    // The guest includes nearT and excludes farT (0xa34/0xa50 and 0x11cc/0x11e8).
    // Start at zero so a triangle exactly at a positive nearT reaches the filter.
    RayDesc ray = {origin, 0.0, direction, farT};
    RayQuery<RAY_FLAG_FORCE_NON_OPAQUE | RAY_FLAG_SKIP_PROCEDURAL_PRIMITIVES> query;
    query.TraceRayInline(scene, RAY_FLAG_NONE, mask, ray);
    while (query.Proceed()) {
        if (query.CandidateType() == CANDIDATE_NON_OPAQUE_TRIANGLE) {
            float distance = query.CandidateTriangleRayT();
            if (distance >= nearT && distance < farT) {
                query.CommitNonOpaqueTriangleHit();
                query.Abort();
            }
        }
    }
    return query.CommittedStatus() == COMMITTED_TRIANGLE_HIT;
}

bool traceCard(uint packedTile, uint pixel) {
    // Five float4 records per page, seven per card (0x01d8-0x02ac).
    uint page = (packedTile & 0xffffffu) * 5u;
    uint card = pages[page].x * 7u;
    float2 texel = float2(((packedTile >> 21) & 120u) | (pixel & 7u),
                          ((packedTile >> 25) & 120u) + (pixel >> 3)) + 0.5;
    float2 uv = mad(asfloat(atlasWords[1].zw), texel, asfloat(pages[page + 2].xy));
    float2 normalXY = mad(normalAtlas.SampleLevel(atlasSampler, uv, 0).xy, 2.0, -1.0);
    float depth = depthAtlas.SampleLevel(atlasSampler, uv, 0).x;

    float2 pageBias = asfloat(pages[page + 1].xy);
    float2 pageScale = asfloat(pages[page + 3].xy);
    float localX = mad(mad(-pageScale.x, texel.x, 1.0 - pageBias.x), -2.0, 1.0);
    float localY = mad(mad(pageScale.y, texel.y, pageBias.y), -2.0, 1.0);
    float3 extent = abs(asfloat(cards[card + 3].xyz));
    float3 localPosition = float3(localX * extent.x, localY * extent.y,
                                  mad(extent.z + extent.z, 1.0 - depth, -extent.z));
    float3 localNormal = float3(normalXY, sqrt(max(0.0001,
        mad(-normalXY.x, normalXY.x, mad(-normalXY.y, normalXY.y, 1.0)))));
    float4 row0 = asfloat(cards[card]);
    float4 row1 = asfloat(cards[card + 1]);
    float4 row2 = asfloat(cards[card + 2]);
    float3 position = float3(dot(row0.xyz, localPosition), dot(row1.xyz, localPosition),
                             dot(row2.xyz, localPosition)) + float3(row0.w, row1.w, row2.w);
    float3 normal = float3(dot(row0.xyz, localNormal), dot(row1.xyz, localNormal),
                           dot(row2.xyz, localNormal));
    float3 direction = asfloat(lightWords[5].xyz);
    float2 interval = asfloat(dispatchWords[31].yz);
    if (dispatchWords[30].x != 0) {
        float3 delta = asfloat(lightWords[3].xyz) - position;
        float lengthSquared = dot(delta, delta);
        interval = min(interval, sqrt(lengthSquared));
        direction = delta * rsqrt(lengthSquared);
    }

    // Different origins and instance-mask bits on the two walks (0x03f0-0x0484,
    // 0x0848, 0x0bc0-0x0c34, and the high-byte selection at 0x0fec).
    if (shadowSegment(position + asfloat(dispatchWords[32].xyz), direction,
                      interval.x, interval.y, 0x20u)) return true;
    float bias = mad(asfloat(dispatchWords[33].x), 1.0 - dot(direction, normal),
                     asfloat(dispatchWords[32].w));
    float3 biasedOrigin = mad(bias, direction, position);
    return shadowSegment(biasedOrigin, direction, 0.0, interval.x, 0x01u);
}

[numthreads(8, 4, 1)]
void main(uint3 id : SV_DispatchThreadID) {
    uint pixel = id.x;
    if (pixel >= 64u) return;
    uint light = dispatchWords[28].x;
    uint queueBase = light * dispatchWords[27].x;
    uint shift = (pixel & 15u) * 2u;
    // Groups can revisit the queue as it grows. Its live count is loaded at
    // both the entry test and the 0x1394 backedge.
    for (uint tile = id.y; tile < queueCounts[light]; tile += dispatchWords[31].x) {
        uint word = dispatchWords[34].x + tile * 4u + (pixel >> 4);
        if ((shadowMask[word] & (2u << shift)) == 0u) {
            bool occluded = traceCard(packedTiles[queueBase + tile], pixel);
            InterlockedOr(shadowMask[word], (2u | uint(occluded)) << shift);
        }
    }
}
