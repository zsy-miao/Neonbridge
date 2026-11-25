// Copyright (c) 2026 Five Star Recomp contributors.
// SPDX-License-Identifier: GPL-3.0-or-later

namespace FiveStar.Shaders.Tests.Cases;

/// <summary>Every compute case, by name: those that run on any PC wave size and those that need a full AMD-sized wave.</summary>
public static class ComputeCases
{
    public static IEnumerable<ComputeCase> All()
    {
        return ComputeGroupCases.All().Concat(ComputeWaveCases.All()).Concat(ComputeMemoryCases.All()).Concat(ComputeBufferRuleCases.All()).Concat(ComputeImageCases.All()).Concat(ComputeBuilderCases.All());
    }

    public static IEnumerable<ComputeCase> FullWave()
    {
        return ComputeWaveCases.FullWave();
    }

    public static ComputeCase Find(string name)
    {
        return All().Concat(FullWave()).Single(testCase => testCase.Name == name);
    }
}
