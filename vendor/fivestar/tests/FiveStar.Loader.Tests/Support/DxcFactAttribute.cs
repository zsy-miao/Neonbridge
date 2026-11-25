// Copyright (c) 2026 Five Star Recomp contributors.
// SPDX-License-Identifier: GPL-3.0-or-later

using Xunit;

namespace FiveStar.Loader.Tests.Support;

/// <summary>A test that compiles HLSL with dxc (FIVESTAR_DXC, default the Windows SDK): skipped without it.</summary>
[AttributeUsage(AttributeTargets.Method)]
public sealed class DxcFactAttribute : FactAttribute
{
    public DxcFactAttribute()
    {
        if (!File.Exists(PipelineFactAttribute.Dxc))
        {
            Skip = "dxc not found (set FIVESTAR_DXC)";
        }
    }
}
