// Copyright (c) 2026 Five Star Recomp contributors.
// SPDX-License-Identifier: GPL-3.0-or-later

namespace FiveStar.Loader.Shims.D3D12X.Resources;

/// <summary>
/// Slot-61 sampler descs to D3D12_SAMPLER_DESC. The filter is encoded as D3D12_ENCODE_BASIC_FILTER (min, mag, mip,
/// reduction) or D3D12_ENCODE_ANISOTROPIC_FILTER (d3d12.h); a comparison function other than NEVER makes a comparison
/// sampler, else the console filter mode 1 and 2 become MINIMUM and MAXIMUM.
/// </summary>
internal static class ConsoleSamplers
{
    /// <summary>Console filter values (<see cref="ConsoleSamplerDescription"/>).</summary>
    private const int ConsoleLinear = 2;
    private const int ConsoleAnisotropic = 3;
    private const int ConsoleMinimum = 1;
    private const int ConsoleMaximum = 2;

    /// <summary>D3D12_FILTER_TYPE_POINT and _LINEAR, and the D3D12_FILTER bit positions (d3d12.h).</summary>
    private const int DesktopPoint = 0;
    private const int DesktopLinear = 1;
    private const int MinShift = 4;
    private const int MagShift = 2;
    private const int ReductionShift = 7;
    private const int AnisotropicFilter = 0x55;

    /// <summary>D3D12_FILTER_REDUCTION_TYPE.</summary>
    private const int StandardReduction = 0;
    private const int ComparisonReduction = 1;
    private const int MinimumReduction = 2;
    private const int MaximumReduction = 3;

    /// <summary>D3D12_COMPARISON_FUNC_NONE and _NEVER, D3D12_TEXTURE_ADDRESS_MODE_WRAP to _MIRROR_ONCE, the anisotropy range.</summary>
    private const int NoComparison = 0;
    private const int NeverComparison = 1;
    private const int FirstAddressMode = 1;
    private const int LastAddressMode = 5;
    private const int MinAnisotropy = 1;
    private const int MaxAnisotropy = 16;

    [HotPath]
    public static DesktopSamplerDescription ToDesktop(in ConsoleSamplerDescription console)
    {
        int reduction = Reduction(console);
        int comparison = console.ComparisonFunc;

        if (comparison == NoComparison)
        {
            comparison = NeverComparison;
        }

        return new DesktopSamplerDescription
        {
            Filter = Filter(console, reduction),
            AddressU = AddressMode(console.AddressU),
            AddressV = AddressMode(console.AddressV),
            AddressW = AddressMode(console.AddressW),
            MipLodBias = console.MipLodBias,
            MaxAnisotropy = Math.Clamp(console.MaxAnisotropy, MinAnisotropy, MaxAnisotropy),
            ComparisonFunc = comparison,
            Border = console.Border,
            MinLod = console.MinLod,
            MaxLod = console.MaxLod,
        };
    }

    private static int Reduction(in ConsoleSamplerDescription console)
    {
        if (console.ComparisonFunc != NoComparison && console.ComparisonFunc != NeverComparison)
        {
            return ComparisonReduction;
        }

        if (console.Reduction == ConsoleMinimum)
        {
            return MinimumReduction;
        }

        if (console.Reduction == ConsoleMaximum)
        {
            return MaximumReduction;
        }

        return StandardReduction;
    }

    private static int Filter(in ConsoleSamplerDescription console, int reduction)
    {
        if (console.MinFilter == ConsoleAnisotropic || console.MagFilter == ConsoleAnisotropic)
        {
            return AnisotropicFilter | (reduction << ReductionShift);
        }

        return (Linear(console.MinFilter) << MinShift) | (Linear(console.MagFilter) << MagShift) | Linear(console.MipFilter) | (reduction << ReductionShift);
    }

    private static int Linear(int filter)
    {
        if (filter == ConsoleLinear)
        {
            return DesktopLinear;
        }

        return DesktopPoint;
    }

    private static int AddressMode(int mode)
    {
        if (mode < FirstAddressMode || mode > LastAddressMode)
        {
            return FirstAddressMode;
        }

        return mode;
    }
}
