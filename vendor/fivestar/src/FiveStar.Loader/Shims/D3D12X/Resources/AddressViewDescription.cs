// Copyright (c) 2026 Five Star Recomp contributors.
// SPDX-License-Identifier: GPL-3.0-or-later

using System.Runtime.InteropServices;

namespace FiveStar.Loader.Shims.D3D12X.Resources;

/// <summary>
/// The 64-byte view desc device slot 70 takes (built at 0x14501f6a6): the desktop D3D12_SHADER_RESOURCE_VIEW_DESC
/// (0x28 bytes, copied per view dimension by 0x144ecc890), the texture's console GPU address at +0x28 (0x144ecc8f0),
/// and at +0x3C 3 or -4 depending on a texture flag (meaning unknown).
/// </summary>
[StructLayout(LayoutKind.Explicit, Size = Size)]
internal struct AddressViewDescription
{
    public const int Size = 0x40;

    /// <summary>Bytes of the desktop SRV desc at the start.</summary>
    public const int DesktopViewSize = 0x28;

    /// <summary>DXGI_FORMAT of the view (desktop SRV desc field 0).</summary>
    [FieldOffset(0)]
    public int Format;

    /// <summary>D3D12_SRV_DIMENSION (desktop SRV desc field 1).</summary>
    [FieldOffset(4)]
    public int ViewDimension;

    /// <summary>Shader4ComponentMapping (desktop SRV desc field 2).</summary>
    [FieldOffset(8)]
    public int ComponentMapping;

    [FieldOffset(DesktopViewSize)]
    public ulong Address;

    [FieldOffset(0x3C)]
    public int Unknown3C;
}
