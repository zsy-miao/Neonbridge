// Copyright (c) 2026 Five Star Recomp contributors.
// SPDX-License-Identifier: GPL-3.0-or-later

using Xunit;

namespace FiveStar.Cli.Tests.Support;

/// <summary>
/// A test that translates real programs and renders on D3D12 WARP: skipped unless FIVESTAR_DUMP and
/// FIVESTAR_SHADER_DIR name their folders and dxc (FIVESTAR_DXC, default the Windows SDK) and WARP exist.
/// </summary>
[AttributeUsage(AttributeTargets.Method)]
public sealed class RenderFactAttribute : FactAttribute
{
    private const string DxcVariable = "FIVESTAR_DXC";
    private const string DefaultDxc = @"C:\Program Files (x86)\Windows Kits\10\bin\10.0.26100.0\x64\dxc.exe";
    private const string WarpLibrary = "d3d10warp.dll";

    public RenderFactAttribute()
    {
        Skip = MissingReason();
    }

    public static string Dxc => Environment.GetEnvironmentVariable(DxcVariable) ?? DefaultDxc;

    private static string? MissingReason()
    {
        if (DumpFactAttribute.Folder is null || DumpFactAttribute.ShaderFolder is null)
        {
            return $"{DumpFactAttribute.Variable} and {DumpFactAttribute.ShaderFolderVariable} are not both set";
        }

        if (!OperatingSystem.IsWindows() || !File.Exists(Path.Combine(Environment.SystemDirectory, WarpLibrary)))
        {
            return "D3D12 WARP is not available";
        }

        if (!File.Exists(Dxc))
        {
            return $"dxc not found (set {DxcVariable})";
        }

        return null;
    }
}
