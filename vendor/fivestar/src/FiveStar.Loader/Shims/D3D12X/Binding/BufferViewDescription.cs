// Copyright (c) 2026 Five Star Recomp contributors.
// SPDX-License-Identifier: GPL-3.0-or-later

using System.Runtime.InteropServices;

namespace FiveStar.Loader.Shims.D3D12X.Binding;

/// <summary>
/// The buffer form of D3D12_SHADER_RESOURCE_VIEW_DESC and D3D12_UNORDERED_ACCESS_VIEW_DESC (d3d12.h), both 40 bytes:
/// format, view dimension, then the SRV's component mapping and the buffer fields at their own offsets.
/// </summary>
[StructLayout(LayoutKind.Explicit, Size = Size)]
internal struct BufferViewDescription
{
    /// <summary>D3D12_SRV_DIMENSION_BUFFER and D3D12_UAV_DIMENSION_BUFFER.</summary>
    public const int BufferDimension = 1;

    /// <summary>DXGI_FORMAT_R32_TYPELESS, the format of raw views (dxgiformat.h).</summary>
    public const int RawFormat = 39;

    /// <summary>D3D12_BUFFER_SRV_FLAG_RAW and D3D12_BUFFER_UAV_FLAG_RAW.</summary>
    public const int RawFlag = 1;

    /// <summary>D3D12_DEFAULT_SHADER_4_COMPONENT_MAPPING.</summary>
    public const int DefaultMapping = 0x1688;

    private const int Size = 40;

    [FieldOffset(0)]
    public int Format;

    [FieldOffset(4)]
    public int Dimension;

    [FieldOffset(8)]
    public int SrvMapping;

    [FieldOffset(16)]
    public ulong SrvFirstElement;

    [FieldOffset(24)]
    public uint SrvElements;

    [FieldOffset(28)]
    public uint SrvStride;

    [FieldOffset(32)]
    public int SrvFlags;

    [FieldOffset(8)]
    public ulong UavFirstElement;

    [FieldOffset(16)]
    public uint UavElements;

    [FieldOffset(20)]
    public uint UavStride;

    [FieldOffset(32)]
    public int UavFlags;

    /// <summary>A raw SRV of <paramref name="dwords"/> dwords from dword <paramref name="first"/> of the buffer.</summary>
    public static BufferViewDescription RawShaderResource(uint dwords, ulong first = 0)
    {
        return new BufferViewDescription { Format = RawFormat, Dimension = BufferDimension, SrvMapping = DefaultMapping, SrvFirstElement = first, SrvElements = dwords, SrvFlags = RawFlag };
    }

    /// <summary>A raw UAV of <paramref name="dwords"/> dwords from dword <paramref name="first"/> of the buffer.</summary>
    public static BufferViewDescription RawUnorderedAccess(uint dwords, ulong first = 0)
    {
        return new BufferViewDescription { Format = RawFormat, Dimension = BufferDimension, UavFirstElement = first, UavElements = dwords, UavFlags = RawFlag };
    }
}
