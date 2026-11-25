// Copyright (c) 2026 Five Star Recomp contributors.
// SPDX-License-Identifier: GPL-3.0-or-later

using FiveStar.Loader.Native;

namespace FiveStar.Loader.Shims.D3D12X;

/// <summary>ID3D12Device::CheckFeatureSupport(D3D12_FEATURE_FORMAT_SUPPORT) for one DXGI format (d3d12.h).</summary>
internal static unsafe class FormatSupport
{
    /// <summary>D3D12_FORMAT_SUPPORT1_RENDER_TARGET and D3D12_FORMAT_SUPPORT1_TYPED_UNORDERED_ACCESS_VIEW.</summary>
    public const uint RenderTarget = 0x4000;
    public const uint TypedUnorderedAccess = 0x2000000;

    /// <summary>ID3D12Device::CheckFeatureSupport and D3D12_FEATURE_FORMAT_SUPPORT.</summary>
    private const int CheckFeatureSupportSlot = 13;
    private const int FormatSupportFeature = 3;

    /// <summary>D3D12_FEATURE_DATA_FORMAT_SUPPORT: Format, Support1, Support2.</summary>
    private const uint DataSize = 12;
    private const int Support1Dword = 1;

    /// <summary>The D3D12_FORMAT_SUPPORT1 bits of <paramref name="format"/>; 0 for a format the device does not know.</summary>
    public static uint Support1(nint device, int format)
    {
        uint* data = stackalloc uint[] { (uint)format, 0, 0 };
        var check = (delegate* unmanaged<nint, int, void*, uint, int>)ComObject.Slot(device, CheckFeatureSupportSlot);

        if (HResult.Failed(check(device, FormatSupportFeature, data, DataSize)))
        {
            return 0;
        }

        return data[Support1Dword];
    }

    /// <summary>Whether <paramref name="device"/> renders to <paramref name="format"/>.</summary>
    public static bool RendersTo(nint device, int format)
    {
        return (Support1(device, format) & RenderTarget) != 0;
    }
}
