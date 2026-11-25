// The provider supplies Vertex, POSITION_FIELD and missing_corner from the VS
// output signature. The IA groups three vertices for each Xbox rectangle.
#ifdef PIXEL_PRIMITIVE_ID
struct Output {
    Vertex vertex;
    uint primitive_id : SV_PrimitiveID;
};
Output raster_vertex(Vertex vertex, uint primitive_id) {
    Output result;
    result.vertex = vertex;
    result.primitive_id = primitive_id;
    return result;
}
#else
#define Output Vertex
Vertex raster_vertex(Vertex vertex, uint primitive_id) {
    return vertex;
}
#endif

[maxvertexcount(4)]
void main(triangle Vertex vertices[3], uint primitive_id : SV_PrimitiveID,
          inout TriangleStream<Output> stream) {
    float2 p[3];
    [unroll] for (uint i = 0; i < 3; i++) {
        float4 position = vertices[i].POSITION_FIELD;
        p[i] = position.xy / position.w;
    }

    // Find the corner at the right angle, independent of vertex order.
    // The two adjacent edges have a zero dot product even for thin tiles.
    float2 edge01 = p[1] - p[0], edge12 = p[2] - p[1], edge20 = p[0] - p[2];
    float3 alignment = abs(float3(dot(edge20, edge01), dot(edge01, edge12), dot(edge12, edge20)));
    uint corner = alignment.x <= min(alignment.y, alignment.z) ? 0 : (alignment.y <= alignment.z ? 1 : 2);
    uint prev = (corner + 2) % 3, next = (corner + 1) % 3;
    Vertex fourth = missing_corner(vertices[prev], vertices[corner], vertices[next]);

    // The shared diagonal is next--prev. This strip retains the input winding.
    stream.Append(raster_vertex(vertices[corner], primitive_id));
    stream.Append(raster_vertex(vertices[next], primitive_id));
    stream.Append(raster_vertex(vertices[prev], primitive_id));
    stream.Append(raster_vertex(fourth, primitive_id));
}
