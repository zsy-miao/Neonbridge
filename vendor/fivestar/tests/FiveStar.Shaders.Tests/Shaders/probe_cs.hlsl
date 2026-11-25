// Copyright (c) 2026 Five Star Recomp contributors.
// SPDX-License-Identifier: GPL-3.0-or-later
// The compute wave layout: per thread its lane, the wave size, the group index of lane 0 of its wave and the group,
// 4 dwords per thread in the RWByteAddressBuffer at heap slot 1.
[numthreads(64, 1, 1)]
void main(uint gi : SV_GroupIndex, uint3 gid : SV_GroupID) {
    RWByteAddressBuffer o = ResourceDescriptorHeap[1];
    uint first = WaveReadLaneFirst(gi);
    o.Store4((gid.x * 64 + gi) * 16, uint4(WaveGetLaneIndex(), WaveGetLaneCount(), first, gid.x));
}
