// Copyright (c) 2026 Five Star Recomp contributors.
// SPDX-License-Identifier: GPL-3.0-or-later

using System.Runtime.InteropServices;

namespace FiveStar.Loader.Shims.D3D12X;

/// <summary>D3D12_RESOURCE_ALLOCATION_INFO1 (d3d12.h): where one desc of a GetResourceAllocationInfo1 query lies.</summary>
[StructLayout(LayoutKind.Sequential)]
internal struct ResourceAllocationInfo1
{
    public ulong Offset;
    public ulong Alignment;
    public ulong Size;
}
