// Copyright (c) 2026 Five Star Recomp contributors.
// SPDX-License-Identifier: GPL-3.0-or-later

using System.Runtime.InteropServices;

namespace FiveStar.Loader.Shims.D3D12X.Resources;

/// <summary>D3D12_SAMPLER_DESC (d3d12.h), 0x34 bytes.</summary>
[StructLayout(LayoutKind.Sequential)]
internal struct DesktopSamplerDescription
{
    /// <summary>D3D12_FILTER.</summary>
    public int Filter;
    public int AddressU;
    public int AddressV;
    public int AddressW;
    public float MipLodBias;
    public int MaxAnisotropy;
    public int ComparisonFunc;
    public SamplerBorderColor Border;
    public float MinLod;
    public float MaxLod;
}
