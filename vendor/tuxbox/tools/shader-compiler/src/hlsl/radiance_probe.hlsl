RaytracingAccelerationStructure probeScene : register(t0, space1);
ByteAddressBuffer probeInstances : register(t1, space1);
ByteAddressBuffer probeGeometry : register(t2, space1);

struct ProbeHit {
    float distance;
    uint instanceID;
    uint instanceIndex;
    uint geometry;
    uint contribution;
    float3 normal;
    bool incomplete;
};

// Each instance contains the full 64-bit native BLAS address at +56.
// Directory offsets are in bytes. Validate each table before indexing it.
bool objectNormal(uint instance, uint geometry, uint primitive, out float3 normal) {
    normal = 0.0;
    uint2 address = probeInstances.Load2(64u * instance + 56u);
    uint2 header = probeGeometry.Load2(0);
    uint bytes = header.y;
    if (bytes < 16u || header.x > (bytes - 16u) / 16u) return false;
    uint lo = 0u, hi = header.x;
    while (lo < hi) {
        uint mid = lo + ((hi - lo) >> 1u);
        uint4 blas = probeGeometry.Load4(16u + mid * 16u);
        if (all(blas.xy == address)) {
            if ((blas.z & 3u) || blas.z > bytes || blas.w > (bytes - blas.z) / 8u
                || geometry >= blas.w) return false;
            uint2 triangles = probeGeometry.Load2(blas.z + geometry * 8u);
            if ((triangles.x & 3u) || triangles.x > bytes || triangles.y > (bytes - triangles.x) / 12u
                || primitive >= triangles.y) return false;
            normal = asfloat(probeGeometry.Load3(triangles.x + primitive * 12u));
            return true;
        }
        if (blas.y < address.y || (blas.y == address.y && blas.x < address.x))
            lo = mid + 1u;
        else
            hi = mid;
    }
    return false;
}

ProbeHit traceProbe(float3 origin, float3 direction, float nearT, float farT
#ifdef TB_SCREEN_GATHER
    , uint rayFlags, uint budget
#endif
) {
    ProbeHit hit = (ProbeHit)0;
    hit.distance = -1.0;
    if (farT > max(0.0, nearT)) {
#ifdef TB_SCREEN_GATHER
        if (budget == 0u) {
            hit.incomplete = true;
            return hit;
        }
#endif
        RayDesc ray = {origin, 0.0, direction, farT};
        RayQuery<RAY_FLAG_FORCE_NON_OPAQUE | RAY_FLAG_SKIP_PROCEDURAL_PRIMITIVES> query;
#ifdef TB_SCREEN_GATHER
#ifdef TB_PROBE_FAR_FIELD
        query.TraceRayInline(probeScene, rayFlags, 0x20u, ray);
#else
        query.TraceRayInline(probeScene, rayFlags, 0x01u, ray);
#endif
#elif defined(TB_PROBE_FAR_FIELD)
        query.TraceRayInline(probeScene, RAY_FLAG_NONE, 0x20u, ray);
#else
        query.TraceRayInline(probeScene, RAY_FLAG_NONE, 0x01u, ray);
#endif
#ifdef TB_SCREEN_GATHER
        uint visits = 0u;
#endif
        while (query.Proceed()) {
            if (query.CandidateType() == CANDIDATE_NON_OPAQUE_TRIANGLE) {
                float distance = query.CandidateTriangleRayT();
                if (distance >= nearT && distance < farT)
                    query.CommitNonOpaqueTriangleHit();
            }
#ifdef TB_SCREEN_GATHER
            if (++visits >= budget) {
                hit.incomplete = true;
                break;
            }
#endif
        }
        if (query.CommittedStatus() == COMMITTED_TRIANGLE_HIT) {
            hit.distance = query.CommittedRayT();
            hit.instanceIndex = query.CommittedInstanceIndex();
            hit.instanceID = query.CommittedInstanceID();
            hit.geometry = query.CommittedGeometryIndex();
            hit.contribution = query.CommittedInstanceContributionToHitGroupIndex();
            float3 normal;
            if (objectNormal(hit.instanceIndex, hit.geometry, query.CommittedPrimitiveIndex(), normal)) {
                // 0x1144-0x1160 uses (C-A) x (B-A); 0x1168-0x11a4
                // multiplies by the three object-to-world rows and normalizes.
                hit.normal = normalize(mul((float3x3)query.CommittedObjectToWorld3x4(), -normal));
            } else {
                hit.distance = -1.0;
                hit.incomplete = true;
            }
        }
    }
    return hit;
}
