// Copyright (c) 2026 Five Star Recomp contributors.
// SPDX-License-Identifier: GPL-3.0-or-later

using Xunit;

namespace FiveStar.Loader.Tests.Support;

/// <summary>A test that compiles with dxc and builds pipelines on D3D12 WARP: skipped unless both exist.</summary>
[AttributeUsage(AttributeTargets.Method)]
public sealed class HostPipelineFactAttribute : FactAttribute
{
    private const string WarpLibrary = "d3d10warp.dll";

    public HostPipelineFactAttribute()
    {
        if (!File.Exists(Path.Combine(Environment.SystemDirectory, WarpLibrary)))
        {
            Skip = "D3D12 WARP is not available";
        }
        else if (!File.Exists(PipelineFactAttribute.Dxc))
        {
            Skip = "dxc not found (set FIVESTAR_DXC)";
        }
    }
}
