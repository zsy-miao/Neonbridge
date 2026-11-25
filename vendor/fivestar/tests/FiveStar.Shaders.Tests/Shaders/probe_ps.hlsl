// Copyright (c) 2026 Five Star Recomp contributors.
// SPDX-License-Identifier: GPL-3.0-or-later
// Per pixel: wave lane index, wave size, quad slot (read back through QuadReadLaneAt) and the wave (the first live lane's
// 1 + x + 256 y), so pixels of one wave are known. Lane-dependent expectations (v_mbcnt, EXEC from an integer,
// readfirstlane, reductions across the wave) are computed from it.
#include "rdna_prelude.hlsli"
uint4 main(float4 pos : SV_Position) : SV_Target0 {
    uint l = WaveGetLaneIndex();
    uint q = 4;
    [unroll] for (uint i = 0; i < 4; i++) if (QuadReadLaneAt(l, i) == l) q = i;
    uint wave = WaveReadLaneFirst(1u + (uint)pos.x + ((uint)pos.y << 8));
    return uint4(l, WaveGetLaneCount(), q, wave + user_sgpr(0) + user_sgpr(59));
}
