// Experimental candidate-stage control for shader 219. This is not a complete
// translation: descriptor publication, traversal order, culling, reciprocal
// precision and output-commit/fallback behavior are not established.
// Compile with -Gis and TUXBOX_RTZ_FLOAT32=1, TUXBOX_RTZ_WIDE_FALLBACK=1;
// concatenate tools/reference/bvh-node/rtz_triangle.hlsl before this file.
RaytracingAccelerationStructure tb_companion : register(t0, space127);
ByteAddressBuffer tb_leaves : register(t1, space127);
ByteAddressBuffer tb_directory : register(t2, space127);
RWByteAddressBuffer tb_status : register(u0, space127);
// Root SRVs do not carry reliable resource extents. These must come from the
// owned buffer allocations, not from the untrusted directory header.
cbuffer TbCompanionBounds : register(b0, space127) {
    uint tb_directory_bytes;
    uint tb_leaf_bytes;
};

// The caller must resolve status before exposing guest output. A status bit
// alone cannot retract prior stores or provide a valid fallback.
void tb_candidate_unsupported(uint reason) {
    uint ignored;
    tb_status.InterlockedOr(0, reason, ignored);
}

// Conservative bounded lattice for the miss sentinel: for these inputs every
// projected intermediate is finite, and nonzero exact products stay far above
// the float32 underflow range. This does not prove general node equivalence.
bool tb_candidate_miss_lattice(float3 v) {
    return all(isfinite(v)) && all(abs(v) <= 256.0) &&
           all(v * 256.0 == trunc(v * 256.0));
}

bool tb_candidate_219(RayQuery<RAY_FLAG_FORCE_NON_OPAQUE> q,
                      float near_t, float far_t, uint cull_mode, out float distance) {
    distance = 0;
    if (q.CandidateType() != CANDIDATE_PROCEDURAL_PRIMITIVE ||
        !isfinite(near_t) || !isfinite(far_t) || near_t < 0 || far_t <= near_t || cull_mode != 0) {
        tb_candidate_unsupported(1u); return false;
    }
    uint bytes = tb_directory_bytes, leaf_bytes = tb_leaf_bytes;
    if (bytes < 16u) { tb_candidate_unsupported(2u); return false; }
    uint4 header = tb_directory.Load4(0);
    uint instances = header.y, candidates = header.z;
    if (header.x != 1u || header.w > leaf_bytes || instances > (bytes - 16u) / 16u) {
        tb_candidate_unsupported(2u); return false;
    }
    uint candidate_start = 16u + instances * 16u;
    if (candidates > (bytes - candidate_start) / 16u || q.CandidateInstanceIndex() >= instances) {
        tb_candidate_unsupported(2u); return false;
    }
    uint4 instance = tb_directory.Load4(16u + q.CandidateInstanceIndex() * 16u);
    uint primitive = q.CandidatePrimitiveIndex();
    if (instance.z != 0 || instance.w != 0 || instance.x > candidates ||
        instance.y > candidates - instance.x || primitive >= instance.y) {
        tb_candidate_unsupported(2u); return false;
    }
    uint4 entry = tb_directory.Load4(candidate_start + (instance.x + primitive) * 16u);
    if (entry.z != 0 || entry.w != 0 || entry.y > 1u || (entry.x & 63u) != 0 ||
        entry.x > header.w || header.w - entry.x < 64u) {
        tb_candidate_unsupported(2u); return false;
    }
    float3 origin = q.CandidateObjectRayOrigin();
    float3 direction = q.CandidateObjectRayDirection();
    if (!all(isfinite(origin)) || !all(isfinite(direction)) || !any(direction != 0)) {
        tb_candidate_unsupported(4u); return false;
    }
    uint node[32];
    [unroll] for (uint i = 0; i < 16; ++i) node[i] = tb_leaves.Load(entry.x + i * 4u);
    [unroll] for (uint j = 16; j < 32; ++j) node[j] = 0;
    bool paired = (node[12] & 0x80u) != 0;
    uint selector = (node[15] >> (8u * entry.y)) & 255u;
    if ((node[12] & 0x7fu) != 0 || (node[15] >> 16) != 0 ||
        paired != (node[14] != 0xffffffffu) || paired != ((node[15] >> 8) != 0) ||
        (entry.y == 1 && !paired) || (selector != 2u && selector != 4u && selector != 9u)) {
        tb_candidate_unsupported(2u); return false;
    }
    float4 candidate = tb_bvh_triangle_rtz(node, entry.y, origin, direction);
    // The helper emits infinity as a miss sentinel. Restrict interpretation of
    // that sentinel to an explicitly bounded range; other inputs fall back.
    if (isinf(candidate.x)) {
        uint ia = entry.y == 0 ? 0u : 1u, ib = entry.y == 0 ? 1u : 3u;
        float3 a = asfloat(uint3(node[3*ia], node[3*ia+1], node[3*ia+2]));
        float3 b = asfloat(uint3(node[3*ib], node[3*ib+1], node[3*ib+2]));
        float3 c = asfloat(uint3(node[6], node[7], node[8]));
        if (!tb_candidate_miss_lattice(origin) || !tb_candidate_miss_lattice(direction) ||
            !tb_candidate_miss_lattice(a) || !tb_candidate_miss_lattice(b) ||
            !tb_candidate_miss_lattice(c) || !all(isfinite(candidate.yzw)))
            tb_candidate_unsupported(4u);
        return false;
    }
    if (!all(isfinite(candidate)) || candidate.y == 0) {
        tb_candidate_unsupported(4u); return false;
    }
    distance = candidate.x * rcp(candidate.y);
    if (!isfinite(distance)) { tb_candidate_unsupported(4u); return false; }
    // Finite candidate, ordinary instance path only: measured distance
    // comparisons include the lower endpoint and exclude the upper endpoint.
    // This does not implement the other 219 culling/termination predicates.
    return distance >= near_t && distance < far_t;
}
