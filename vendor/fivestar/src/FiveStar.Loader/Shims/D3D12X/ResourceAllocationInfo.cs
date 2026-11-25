// Copyright (c) 2026 Five Star Recomp contributors.
// SPDX-License-Identifier: GPL-3.0-or-later

using System.Runtime.InteropServices;

namespace FiveStar.Loader.Shims.D3D12X;

/// <summary>D3D12_RESOURCE_ALLOCATION_INFO (d3d12.h): size and alignment; the D3D12.X device returns the same 16 bytes.</summary>
[StructLayout(LayoutKind.Sequential)]
internal struct ResourceAllocationInfo
{
    public ulong Size;
    public ulong Alignment;
}
