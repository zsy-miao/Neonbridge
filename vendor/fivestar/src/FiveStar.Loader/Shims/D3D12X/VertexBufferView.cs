// Copyright (c) 2026 Five Star Recomp contributors.
// SPDX-License-Identifier: GPL-3.0-or-later

using System.Runtime.InteropServices;

namespace FiveStar.Loader.Shims.D3D12X;

/// <summary>
/// D3D12_VERTEX_BUFFER_VIEW and D3D12_INDEX_BUFFER_VIEW (d3d12.h): a GPU address, a size and the stride (vertex
/// buffers) or DXGI format (index buffers). Both are 16 bytes with the same first two fields.
/// </summary>
[StructLayout(LayoutKind.Sequential)]
internal struct VertexBufferView
{
    public ulong Location;
    public uint Size;

    /// <summary>The vertex stride, or the index format of an index-buffer view.</summary>
    public uint StrideOrFormat;
}
