struct Payload { uint value; };
struct Vertex {
    float4 position : SV_Position;
    nointerpolation uint value : TEXCOORD0;
};
groupshared Payload sharedPayload;
[numthreads(1, 1, 1)]
void amplify() {
    sharedPayload.value = 0x1c3d5e7f;
    DispatchMesh(1, 1, 1, sharedPayload);
}
[numthreads(1, 1, 1)]
[outputtopology("triangle")]
void mesh(
#ifdef WITH_PAYLOAD
    in payload Payload incoming,
#endif
    out vertices Vertex verticesOut[3],
    out indices uint3 indicesOut[1]) {
    SetMeshOutputCounts(3, 1);
    static const float2 corners[3] = {float2(-1,-1), float2(-1,3), float2(3,-1)};
    for (uint i = 0; i < 3; ++i) {
        verticesOut[i].position = float4(corners[i], 0.5, 1.0);
#ifdef WITH_PAYLOAD
        verticesOut[i].value = incoming.value;
#else
        verticesOut[i].value = 0x2468ace0;
#endif
    }
    indicesOut[0] = uint3(0, 1, 2);
}
RWByteAddressBuffer outputColor : register(u0);
void pixel(Vertex vertex) {
    uint2 xy = uint2(vertex.position.xy);
    outputColor.Store(4 * (xy.x + xy.y * 8), vertex.value);
}
