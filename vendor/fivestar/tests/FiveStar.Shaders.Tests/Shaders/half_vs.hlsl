// Copyright (c) 2026 Five Star Recomp contributors.
// SPDX-License-Identifier: GPL-3.0-or-later
// Partial coverage for helper-lane tests (8x8 target, 16x16 viewport): vertices (0,0), (7.5,0), (0,7.5) in pixels, so
// pixel (x, y) is covered iff x + y <= 6 and the quads along the edge hold helper lanes.
float4 main(uint vid : SV_VertexID) : SV_Position {
    float2 uv = float2((vid << 1) & 2, vid & 2) * 0.5;
    return float4(uv * float2(0.9375, -0.9375) + float2(-1, 1), 0, 1);
}
