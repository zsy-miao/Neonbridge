// Copyright (c) 2026 Five Star Recomp contributors.
// SPDX-License-Identifier: GPL-3.0-or-later

using FiveStar.Shaders.Compilation;
using Xunit;

namespace FiveStar.Loader.Tests.Support;

/// <summary>
/// A test that makes real programs on demand on D3D12 WARP: skipped unless FIVESTAR_SHADER_DIR names the extracted
/// <c>fssm</c> folder and dxcompiler.dll (beside FIVESTAR_DXC, default the Windows SDK) and WARP exist.
/// </summary>
[AttributeUsage(AttributeTargets.Method)]
public sealed class OnDemandFactAttribute : FactAttribute
{
    private const string WarpLibrary = "d3d10warp.dll";

    public OnDemandFactAttribute()
    {
        if (PipelineFactAttribute.ShaderFolder is null)
        {
            Skip = $"{PipelineFactAttribute.ShaderFolderVariable} is not set";
        }
        else if (!File.Exists(Path.Combine(Environment.SystemDirectory, WarpLibrary)))
        {
            Skip = "D3D12 WARP is not available";
        }
        else if (!File.Exists(Library))
        {
            Skip = $"{Library} not found (set FIVESTAR_DXC)";
        }
    }

    /// <summary>The dxcompiler.dll beside the test dxc.</summary>
    public static string Library => DxcLibraryCompiler.LibraryPath(PipelineFactAttribute.Dxc);
}
