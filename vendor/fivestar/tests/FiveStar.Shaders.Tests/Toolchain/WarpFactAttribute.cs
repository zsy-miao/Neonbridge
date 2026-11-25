// Copyright (c) 2026 Five Star Recomp contributors.
// SPDX-License-Identifier: GPL-3.0-or-later

using Xunit;

namespace FiveStar.Shaders.Tests.Toolchain;

/// <summary>A single translator test run on D3D12 WARP: skipped with the reason when dxc or WARP is missing.</summary>
[AttributeUsage(AttributeTargets.Method)]
public sealed class WarpFactAttribute : FactAttribute
{
    public WarpFactAttribute()
    {
        var reason = ShaderToolchain.MissingReason();

        if (reason is not null)
        {
            Skip = reason;
        }
    }
}
