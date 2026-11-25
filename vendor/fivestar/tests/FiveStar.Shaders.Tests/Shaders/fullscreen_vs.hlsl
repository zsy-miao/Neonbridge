// Copyright (c) 2026 Five Star Recomp contributors.
// SPDX-License-Identifier: GPL-3.0-or-later
// One triangle (0,0), (vw,0), (0,vh) in viewport pixels. With the viewport at twice the target size it covers the
// target without clipping (WARP splits a clipped triangle, which creates helper lanes along the split).
float4 main(uint vid : SV_VertexID) : SV_Position {
    float2 uv = float2((vid << 1) & 2, vid & 2);
    return float4(uv * float2(1, -1) + float2(-1, 1), 0, 1);
}
