// Copyright (c) 2026 Five Star Recomp contributors.
// SPDX-License-Identifier: GPL-3.0-or-later

// The hardware wave a mesh shader runs in (MeshWaveProbe), compiled at run time by the DXIL backend. Descriptor 0 of
// the heap is a raw UAV of {least lane count, greatest lane count}; it starts as {0xffffffff, 0}.
struct Vertex
{
    float4 position : SV_Position;
};

[numthreads(128, 1, 1)]
[outputtopology("triangle")]
void main(uint thread : SV_GroupIndex, out vertices Vertex outputs[1], out indices uint3 triangles[1])
{
    SetMeshOutputCounts(0, 0);

    // Never written: no vertex is output, but the validator asks for a write of every element.
    if (thread == 128u)
    {
        outputs[0].position = 0;
    }

    RWByteAddressBuffer lanes = ResourceDescriptorHeap[0];
    lanes.InterlockedMin(0, WaveGetLaneCount());
    lanes.InterlockedMax(4, WaveGetLaneCount());
}
