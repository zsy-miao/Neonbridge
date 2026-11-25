float4 VSMain(uint vertex : SV_VertexID) : SV_Position {
    const float2 corners[3] = { float2(-1, -1), float2(-1, 3), float2(3, -1) };
    return float4(corners[vertex], 0.5, 1);
}

float4 VSFromBuffer(float2 position : POSITION) : SV_Position {
    return float4(position, 0.5, 1);
}

float4 PSMain() : SV_Target0 {
    return float4(1, 0, 0, 1);
}
