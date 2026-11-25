// Each mesh group contributes a distinct integer to every covered pixel.
struct Payload { uint value; };
struct Vertex { float4 position : SV_Position; nointerpolation uint value : TEXCOORD0; };
groupshared Payload parent;

[numthreads(1, 1, 1)]
void amplify(uint3 group : SV_GroupID) {
    parent.value = 100 + 13 * group.x + 19 * group.y + 31 * group.z;
    DispatchMesh(1, 2, 1, parent);
}

[numthreads(1, 1, 1)]
[outputtopology("triangle")]
void mesh(uint3 group : SV_GroupID,
#ifdef WITH_PAYLOAD
          in payload Payload incoming,
#endif
          out vertices Vertex verts[3], out indices uint3 faces[1]) {
    SetMeshOutputCounts(3, 1);
    const float2 corners[] = {float2(-1,-1), float2(-1,3), float2(3,-1)};
    uint value = 1 + 2 * group.x + 5 * group.y + 11 * group.z;
#ifdef WITH_PAYLOAD
    value += incoming.value;
#endif
    for (uint i = 0; i < 3; ++i) {
        verts[i].position = float4(corners[i], 0.5, 1);
        verts[i].value = value;
    }
    faces[0] = uint3(0, 1, 2);
}

RWByteAddressBuffer values : register(u0);
void pixel(Vertex v) {
    const uint2 xy = uint2(v.position.xy);
    values.InterlockedAdd(4 * (xy.x + xy.y * 8), v.value);
}
