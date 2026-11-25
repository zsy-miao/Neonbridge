// Scalar reference operations for Xbox runtime-style BVH4 records.
// Scope: finite boxes and origins, finite or infinite inverse direction, zero box growth,
// closest-first sorting, and uncompressed single triangles. This is not a
// hardware-exact replacement for image_bvh64_intersect_ray.
float tb_bvh_bound(uint node[32], uint child, uint component, bool box16) {
    if (!box16)
        return asfloat(node[4 + child * 6 + component]);
    uint halfIndex = child * 6 + component;
    uint pair = node[4 + halfIndex / 2];
    return f16tof32((pair >> ((halfIndex & 1u) * 16u)) & 0xffffu);
}

uint4 tb_bvh_boxes(uint node[32], bool box16, float3 origin,
                   float3 inverseDirection, float extent, bool sortChildren) {
    uint child[4];
    float distance[4];
    [unroll] for (uint i = 0; i < 4; ++i) {
        float3 lower = float3(tb_bvh_bound(node, i, 0, box16),
                              tb_bvh_bound(node, i, 1, box16),
                              tb_bvh_bound(node, i, 2, box16));
        float3 upper = float3(tb_bvh_bound(node, i, 3, box16),
                              tb_bvh_bound(node, i, 4, box16),
                              tb_bvh_bound(node, i, 5, box16));
        float3 a = (lower - origin) * inverseDirection;
        float3 b = (upper - origin) * inverseDirection;
        float3 nearAxis = min(a, b);
        float3 farAxis = max(a, b);
        // Infinite reciprocal represents a parallel ray. Avoid 0 * infinity
        // at slab boundaries; those boundaries belong to the box.
        [unroll] for (uint axis = 0; axis < 3; ++axis) {
            if (isinf(inverseDirection[axis])) {
                bool inside = origin[axis] >= lower[axis] && origin[axis] <= upper[axis];
                nearAxis[axis] = asfloat(inside ? 0xff800000u : 0x7f800000u);
                farAxis[axis] = asfloat(inside ? 0x7f800000u : 0xff800000u);
            }
        }
        float nearT = max(0.0, max(nearAxis.x, max(nearAxis.y, nearAxis.z)));
        float farT = min(extent, min(farAxis.x, min(farAxis.y, farAxis.z)));
        child[i] = nearT <= farT ? node[i] : 0xffffffffu;
        distance[i] = nearT;
    }
    if (sortChildren) {
        // Four-way comparator network. Equal keys can change relative order;
        // tie order has not been checked against the native instruction.
        static const uint left[5] = {0, 1, 0, 2, 1};
        static const uint right[5] = {2, 3, 1, 3, 2};
        [unroll] for (uint i = 0; i < 5; ++i) {
            uint a = left[i], b = right[i];
            bool swap = child[a] == 0xffffffffu ||
                (child[b] != 0xffffffffu && distance[b] < distance[a]);
            if (swap) {
                uint c = child[a]; child[a] = child[b]; child[b] = c;
                float d = distance[a]; distance[a] = distance[b]; distance[b] = d;
            }
        }
    }
    return uint4(child[0], child[1], child[2], child[3]);
}

// Returns unremapped t numerator, determinant, and two barycentric
// numerators. For non-degenerate ordinary misses, gfx1036 preserves the
// determinant and uses an infinity with the same sign for the numerator.
// Edge ownership is tested for finite rays; general special values and
// arithmetic behavior near degenerate inputs remain unverified.
bool tb_bvh_edge_owned(float3 a, float3 b, float3 inside, float3 direction, uint axis) {
    uint x = (axis + 1) % 3, y = (axis + 2) % 3;
    float2 slope = float2(direction[x], direction[y]) / direction[axis];
    float2 pa = float2(a[x], a[y]) - slope * a[axis];
    float2 pb = float2(b[x], b[y]) - slope * b[axis];
    float2 pc = float2(inside[x], inside[y]) - slope * inside[axis];
    float2 edge = pb - pa;
    float2 inward = float2(-edge.y, edge.x);
    if (dot(inward, pc - pa) < 0.0)
        inward = -inward;
    return inward.x > 0.0 || (inward.x == 0.0 && inward.y * direction[axis] < 0.0);
}
float4 tb_bvh_triangle(float3 origin, float3 direction,
                       float3 a, float3 b, float3 c) {
    float3 edge1 = b - a;
    float3 edge2 = c - a;
    float3 fromVertex = origin - a;
    float3 p = cross(direction, edge2);
    float3 q = cross(fromVertex, edge1);
    float determinant = dot(edge1, p);
    float tNumerator = dot(edge2, q);
    float iNumerator = dot(fromVertex, p);
    float jNumerator = dot(direction, q);
    if (determinant == 0.0)
        return float4(asfloat(0x7f800000u), 0.0, iNumerator, jNumerator);
    float t = tNumerator / determinant;
    float i = iNumerator / determinant;
    float j = jNumerator / determinant;
    bool missed = t < 0.0 || i < 0.0 || j < 0.0 || i + j > 1.0;
    float3 magnitude = abs(direction);
    uint axis = magnitude.x >= magnitude.y && magnitude.x >= magnitude.z ? 0u :
                magnitude.y >= magnitude.z ? 1u : 2u;
    if (magnitude[axis] > 0.0) {
        if (i == 0.0 && !tb_bvh_edge_owned(a, c, b, direction, axis)) missed = true;
        if (j == 0.0 && !tb_bvh_edge_owned(a, b, c, direction, axis)) missed = true;
        if (i + j == 1.0 && !tb_bvh_edge_owned(b, c, a, direction, axis)) missed = true;
    }
    if (missed)
        return float4(asfloat(0x7f800000u | (asuint(determinant) & 0x80000000u)),
                      determinant, iNumerator, jNumerator);
    return float4(tNumerator, determinant, iNumerator, jNumerator);
}

// First uncompressed triangle in a 64-byte Xbox leaf record. The vertex
// positions occupy its first nine dwords. Barycentric remapping and the
// other triangle node types are deliberately left to their callers.
float4 tb_bvh_triangle_leaf(uint node[32], float3 origin, float3 direction) {
    float3 a = asfloat(uint3(node[0], node[1], node[2]));
    float3 b = asfloat(uint3(node[3], node[4], node[5]));
    float3 c = asfloat(uint3(node[6], node[7], node[8]));
    return tb_bvh_triangle(origin, direction, a, b, c);
}

// TRI0 and TRI1 of an uncompressed paired leaf, with the verified selector
// values 9, 2 and 4. Callers must validate the node type and selector first.
// Types TRI2/TRI3 and compressed triangle layouts remain unsupported.
float4 tb_bvh_paired_triangle(uint node[32], uint side,
                              float3 origin, float3 direction) {
    uint ia = side == 0 ? 0 : 1;
    uint ib = side == 0 ? 1 : 3;
    uint ic = 2;
    float3 a = asfloat(uint3(node[3 * ia], node[3 * ia + 1], node[3 * ia + 2]));
    float3 b = asfloat(uint3(node[3 * ib], node[3 * ib + 1], node[3 * ib + 2]));
    float3 c = asfloat(uint3(node[3 * ic], node[3 * ic + 1], node[3 * ic + 2]));
    float4 result = tb_bvh_triangle(origin, direction, a, b, c);
    uint selector = (node[15] >> (8 * side)) & 0xffu;
    float bary[3] = {result.y - result.z - result.w, result.z, result.w};
    result.z = bary[selector & 3u];
    result.w = bary[(selector >> 2) & 3u];
    return result;
}
