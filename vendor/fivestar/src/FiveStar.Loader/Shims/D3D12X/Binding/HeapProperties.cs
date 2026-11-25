// Copyright (c) 2026 Five Star Recomp contributors.
// SPDX-License-Identifier: GPL-3.0-or-later

using System.Runtime.InteropServices;

namespace FiveStar.Loader.Shims.D3D12X.Binding;

/// <summary>D3D12_HEAP_PROPERTIES (d3d12.h).</summary>
[StructLayout(LayoutKind.Sequential)]
internal struct HeapProperties
{
    /// <summary>D3D12_HEAP_TYPE_UPLOAD.</summary>
    public const int Upload = 2;

    /// <summary>D3D12_HEAP_TYPE_CUSTOM.</summary>
    public const int Custom = 4;

    /// <summary>D3D12_CPU_PAGE_PROPERTY_WRITE_COMBINE.</summary>
    public const int WriteCombine = 2;

    /// <summary>D3D12_MEMORY_POOL_L0 (system memory on a discrete GPU).</summary>
    public const int SystemPool = 1;

    public int Type;
    public int CpuPageProperty;
    public int MemoryPool;
    public uint CreationNodeMask;
    public uint VisibleNodeMask;
}
