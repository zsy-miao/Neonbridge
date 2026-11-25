// Copyright (c) 2026 Five Star Recomp contributors.
// SPDX-License-Identifier: GPL-3.0-or-later

namespace FiveStar.Shaders.Tests.Cases;

/// <summary>Every pixel-shader case, by name.</summary>
public static class PixelCases
{
    public static IEnumerable<PixelCase> All()
    {
        return PixelValuCases.All()
            .Concat(PixelScalarCases.All())
            .Concat(PixelPackedCases.All())
            .Concat(PixelLaneCases.All())
            .Concat(PixelControlFlowCases.All())
            .Concat(PixelMemoryCases.All());
    }

    public static PixelCase Find(string name)
    {
        return All().Single(testCase => testCase.Name == name);
    }
}
