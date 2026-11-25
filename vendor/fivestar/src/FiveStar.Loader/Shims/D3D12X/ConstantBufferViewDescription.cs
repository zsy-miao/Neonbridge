// Copyright (c) 2026 Five Star Recomp contributors.
// SPDX-License-Identifier: GPL-3.0-or-later

using System.Runtime.InteropServices;

namespace FiveStar.Loader.Shims.D3D12X;

/// <summary>D3D12_CONSTANT_BUFFER_VIEW_DESC (d3d12.h): GPU address and size.</summary>
[StructLayout(LayoutKind.Sequential)]
internal struct ConstantBufferViewDescription
{
    /// <summary>D3D12_CONSTANT_BUFFER_DATA_PLACEMENT_ALIGNMENT: the desktop size granularity; the console takes 128.</summary>
    public const uint Alignment = 256;

    public ulong Location;
    public uint Size;
}
