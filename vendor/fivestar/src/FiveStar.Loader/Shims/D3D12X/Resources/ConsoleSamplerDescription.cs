// Copyright (c) 2026 Five Star Recomp contributors.
// SPDX-License-Identifier: GPL-3.0-or-later

using System.Runtime.InteropServices;

namespace FiveStar.Loader.Shims.D3D12X.Resources;

/// <summary>
/// The 0x6c-byte sampler desc device slot 61 takes, as the game's only caller fills it (0x1412b96b0, zeroed first).
/// Field meanings are *inferred* from the values written: filters are 1 point, 2 linear, 3 anisotropic.
/// </summary>
[StructLayout(LayoutKind.Explicit, Size = Size)]
internal struct ConsoleSamplerDescription
{
    public const int Size = 0x6c;

    /// <summary>Always 0 at the call site.</summary>
    [FieldOffset(0x00)]
    public int Unknown0;

    /// <summary>1 (point) when flag 0x4 is set, else 2; 3 for anisotropic samplers (flag 0x40).</summary>
    [FieldOffset(0x04)]
    public int MagFilter;

    /// <summary>1 when flag 0x10 is set, else 2; 3 for anisotropic samplers.</summary>
    [FieldOffset(0x08)]
    public int MinFilter;

    /// <summary>2 (linear) when flag 0x1 is set, else 1.</summary>
    [FieldOffset(0x0C)]
    public int MipFilter;

    /// <summary>2 when flag 0x4 is set, else 1 (the opposite of <see cref="MagFilter"/>); unknown, read as a depth filter.</summary>
    [FieldOffset(0x10)]
    public int Unknown10;

    /// <summary>A table entry picked by the flags from bit 7 up (0x146172fa0): 0, 1 or 2, read as the AMD filter mode (blend, min, max).</summary>
    [FieldOffset(0x14)]
    public int Reduction;

    /// <summary>Address modes in D3D12_TEXTURE_ADDRESS_MODE numbering (bytes 1 to 3 of the game's sampler state).</summary>
    [FieldOffset(0x18)]
    public int AddressU;

    [FieldOffset(0x1C)]
    public int AddressV;

    [FieldOffset(0x20)]
    public int AddressW;

    [FieldOffset(0x24)]
    public float MinLod;

    [FieldOffset(0x28)]
    public float MaxLod;

    [FieldOffset(0x30)]
    public float MipLodBias;

    /// <summary>The game's anisotropy byte masked with 0x1f.</summary>
    [FieldOffset(0x34)]
    public int MaxAnisotropy;

    /// <summary>D3D12_COMPARISON_FUNC: the game's byte when flag 0x80 is set, else 1 (NEVER).</summary>
    [FieldOffset(0x40)]
    public int ComparisonFunc;

    [FieldOffset(0x44)]
    public SamplerBorderColor Border;
}
