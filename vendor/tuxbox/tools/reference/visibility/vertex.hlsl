struct Output {
    nointerpolation uint id : TEXCOORD1;
#ifdef CLIP_INPUT
    nointerpolation int4 rect : TEXCOORD2;
#endif
    float4 position : SV_Position;
#ifdef MESH_INPUT
    nointerpolation uint4 cluster : TEXCOORD7;
#endif
};

Output main(uint vertex : SV_VertexID, uint layer : SV_InstanceID) {
    Output o;
#ifdef CLIP_INPUT
    o.rect = int4(5, 4, 12, 8);
#endif
    const float2 vertices[3] = {float2(-1, -1), float2(-1, 3), float2(3, -1)};
    o.position = float4(vertices[vertex], layer == 1 ? 0.25 : 0.75, 1);
    o.id = layer == 1 ? ~0u : layer == 0 ? 1u : 11u;
#ifdef MESH_INPUT
    o.cluster = uint4(o.id, layer == 1 ? 127u : 3u, 0, 0);
#endif
    return o;
}
