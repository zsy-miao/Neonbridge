struct VSOutput {
#if EXTRA
    float4 uv : TEXCOORD0;
    nointerpolation uint id : PRIMITIVE_ID0;
#endif
    centroid float4 normal : TEXCOORD10_centroid0;
    centroid float4 tangent : TEXCOORD11_centroid0;
#if !EXTRA
    nointerpolation uint id : PRIMITIVE_ID0;
#endif
    float3 k : TEXCOORD7;
    precise float4 pos : SV_Position;
};
VSOutput main(uint id : SV_VertexID) {
    VSOutput output = (VSOutput)0;
    output.pos = float4(id == 1 ? 3 : -1, id == 2 ? 3 : -1, .25, 1);
    return output;
}
