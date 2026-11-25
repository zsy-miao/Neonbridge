// Copyright (c) 2026 Five Star Recomp contributors.
// SPDX-License-Identifier: GPL-3.0-or-later

namespace FiveStar.Loader.Shims.D3D12X;

/// <summary>
/// A D3D12_SAMPLER_DESC (13 dwords, d3d12.h) for samplers whose console fields are not known: MIN_MAG_MIP_LINEAR,
/// WRAP on U, V and W, no LOD bias, anisotropy 1, COMPARISON_FUNC_NEVER, a transparent black border, LOD 0 to
/// D3D12_FLOAT32_MAX.
/// </summary>
internal static unsafe class TrilinearSampler
{
    public const int Dwords = 13;

    private const uint LinearFilter = 0x15;
    private const uint WrapAddress = 1;
    private const uint NoAnisotropy = 1;
    private const uint NeverCompare = 1;
    private const int AddressUDword = 1;
    private const int AddressVDword = 2;
    private const int AddressWDword = 3;
    private const int AnisotropyDword = 5;
    private const int ComparisonDword = 6;
    private const int MaxLodDword = 12;

    /// <summary>Writes the desc into <see cref="Dwords"/> dwords at <paramref name="sampler"/>.</summary>
    public static void Write(uint* sampler)
    {
        new Span<uint>(sampler, Dwords).Clear();

        sampler[0] = LinearFilter;
        sampler[AddressUDword] = WrapAddress;
        sampler[AddressVDword] = WrapAddress;
        sampler[AddressWDword] = WrapAddress;

        sampler[AnisotropyDword] = NoAnisotropy;
        sampler[ComparisonDword] = NeverCompare;
        *(float*)(sampler + MaxLodDword) = float.MaxValue;
    }
}
