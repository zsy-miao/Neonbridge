struct Varyings {
    float2 uv : TEXCOORD0;
    nointerpolation uint tile : TILE;
    float4 position : SV_Position;
};

Varyings VSMain(uint vertex : SV_VertexID, uint instance : SV_InstanceID) {
    uint index = 3 * instance + vertex;
    // The first three corners turn at index 1. The second turn is at index 0.
    const float2 corners[6] = {
        float2(0, 0), float2(1, 0), float2(1, 1),
        float2(0, 0), float2(1, 0), float2(0, 1)
    };
    Varyings v;
    v.uv = corners[index];
    v.tile = instance;
    float2 pixel = (index < 3 ? float2(1, 2) : float2(9, 9)) + 6 * v.uv;
    v.position = float4(pixel * float2(0.125, -0.125) + float2(-1, 1), 0.5, 1);
    return v;
}

float4 PSMain(Varyings v, uint id : SV_PrimitiveID) : SV_Target {
    return float4(v.uv.x > 0.5, v.uv.y > 0.5, id == 1 || v.tile == 1, 1);
}
