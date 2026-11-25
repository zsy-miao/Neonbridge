// Write distinct values into each physical sample of both input textures.
float4 vs(uint vertex : SV_VertexID) : SV_Position {
    return float4((vertex == 1u) ? 3.0 : -1.0, (vertex == 2u) ? -3.0 : 1.0, 0.5, 1.0);
}
struct Fragment {
    uint id : SV_Target0;
    float depth : SV_Target1;
};
Fragment ps(float4 position : SV_Position, uint sample : SV_SampleIndex) {
    uint pixel = (uint)position.y * 8u + (uint)position.x;
    uint mode = pixel % 5u;
    Fragment f;
    // Include an empty pixel, unique samples, an ID shared by two samples,
    // alternating absent samples, and geometry behind the scene's depth.
    f.id = 1u + 16u * pixel + ((mode == 2u && sample == 1u) ? 0u : sample);
    if (mode == 0u || (mode == 3u && (sample & 1u) == 0u)) f.id = 0xffffffffu;
    f.depth = (mode == 4u && (sample & 1u) != 0u) ? 0.125 : (8u + sample) / 32.0;
    return f;
}
