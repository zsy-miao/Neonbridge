// Copyright (c) 2026 Five Star Recomp contributors.
// SPDX-License-Identifier: GPL-3.0-or-later

using System.Runtime.InteropServices;

namespace FiveStar.Loader.Shims.D3D12X;

/// <summary>D3D12_DESCRIPTOR_HEAP_DESC (d3d12.h): type, descriptor count, flags, node mask.</summary>
[StructLayout(LayoutKind.Sequential)]
internal struct DescriptorHeapDescription
{
    public int Type;
    public uint Count;
    public int Flags;
    public uint NodeMask;
}
