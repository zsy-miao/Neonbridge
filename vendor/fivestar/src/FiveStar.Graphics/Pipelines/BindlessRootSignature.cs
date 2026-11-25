// Copyright (c) 2026 Five Star Recomp contributors.
// SPDX-License-Identifier: GPL-3.0-or-later

using Vortice.Direct3D12;

namespace FiveStar.Graphics.Pipelines;

/// <summary>
/// The root signature every translated program runs with. Parameter 0 is the user data at b0: a root CBV over the 128
/// dwords of <c>cbuffer UserData</c>, or with <see cref="UserConstants"/> 32 root constants (programs compiled with
/// <see cref="UserConstantsDefine"/>); parameter 1 is a descriptor table with the mesh fallback's u0..u2 and t0 in
/// register space 9. Everything else is reached through <c>ResourceDescriptorHeap[]</c> and <c>SamplerDescriptorHeap[]</c>.
/// </summary>
public static class BindlessRootSignature
{
    /// <summary>Root parameter index of the user-data CBV.</summary>
    public const int UserDataParameter = 0;

    /// <summary>Root parameter index of the mesh fallback table.</summary>
    public const int FallbackTableParameter = 1;

    /// <summary>Register space of the mesh fallback bindings (rdna_prelude / MeshComputeFallback).</summary>
    public const int FallbackSpace = 9;

    /// <summary>Number of fallback UAVs (u0 vertex records, u1 indices, u2 draw arguments), followed by one SRV (t0).</summary>
    public const int FallbackUavs = 3;

    /// <summary>
    /// User-data dwords as root constants: s0..s31, every user SGPR an RDNA program is preloaded with (32 user-data
    /// registers per stage), so no translated program reads past them.
    /// </summary>
    public const int UserConstants = 32;

    /// <summary>The define of programs that read their user data from <see cref="UserConstants"/> root constants (rdna_prelude.hlsli).</summary>
    public const string UserConstantsDefine = "FIVESTAR_USER_CONSTANTS";

    /// <summary>The most a root signature holds: 64 DWORDs, a root constant one, a root descriptor two, a table one.</summary>
    public const int MaxDwords = 64;

    private const int RootDescriptorDwords = 2;
    private const int TableDwords = 1;
    private const int UserDataRegister = 0;
    private const int UserDataSpace = 0;
    private const int FallbackSrvs = 1;

    /// <summary>The DWORDs of the root signature: the user data (a root CBV or the root constants) and the fallback table.</summary>
    public static int Dwords(bool userConstants)
    {
        if (userConstants)
        {
            return UserConstants + TableDwords;
        }

        return RootDescriptorDwords + TableDwords;
    }

    public static RootSignatureDescription1 Description()
    {
        return Description(userConstants: false);
    }

    /// <summary>The layout with the user data as a root CBV, or as <see cref="UserConstants"/> root constants.</summary>
    public static RootSignatureDescription1 Description(bool userConstants)
    {
        var flags = RootSignatureFlags.ConstantBufferViewShaderResourceViewUnorderedAccessViewHeapDirectlyIndexed
            | RootSignatureFlags.SamplerHeapDirectlyIndexed
            | RootSignatureFlags.AllowInputAssemblerInputLayout;
        var volatileRange = DescriptorRangeFlags.DataVolatile | DescriptorRangeFlags.DescriptorsVolatile;
        DescriptorRange1[] fallbackRanges =
        [
            new(DescriptorRangeType.UnorderedAccessView, FallbackUavs, 0, FallbackSpace, 0, volatileRange),
            new(DescriptorRangeType.ShaderResourceView, FallbackSrvs, 0, FallbackSpace, FallbackUavs, volatileRange),
        ];
        var userData = new RootParameter1(RootParameterType.ConstantBufferView, new RootDescriptor1(UserDataRegister, UserDataSpace, RootDescriptorFlags.DataVolatile), ShaderVisibility.All);

        if (userConstants)
        {
            userData = new RootParameter1(new RootConstants(UserDataRegister, UserDataSpace, UserConstants), ShaderVisibility.All);
        }

        RootParameter1[] parameters = [userData, new(new RootDescriptorTable1(fallbackRanges), ShaderVisibility.All)];

        return new RootSignatureDescription1(flags, parameters);
    }

    public static ID3D12RootSignature Create(ID3D12Device device)
    {
        return device.CreateRootSignature(Description());
    }
}
