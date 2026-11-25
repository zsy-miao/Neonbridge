// Copyright (c) 2026 Five Star Recomp contributors.
// SPDX-License-Identifier: GPL-3.0-or-later

using Xunit;

namespace FiveStar.Loader.Tests.Support;

/// <summary>
/// A test that translates real programs and creates pipelines on D3D12 WARP: skipped unless FIVESTAR_DUMP and
/// FIVESTAR_SHADER_DIR (the extracted <c>fssm</c> folder, its <c>dxbc</c> sibling next to it) name folders and dxc
/// (FIVESTAR_DXC, default the Windows SDK) and WARP exist.
/// </summary>
[AttributeUsage(AttributeTargets.Method)]
public sealed class PipelineFactAttribute : FactAttribute
{
    public const string ShaderFolderVariable = "FIVESTAR_SHADER_DIR";

    private const string DxcVariable = "FIVESTAR_DXC";
    private const string DefaultDxc = @"C:\Program Files (x86)\Windows Kits\10\bin\10.0.26100.0\x64\dxc.exe";
    private const string WarpLibrary = "d3d10warp.dll";

    public PipelineFactAttribute()
    {
        Skip = MissingReason();
    }

    public static string Dxc => Environment.GetEnvironmentVariable(DxcVariable) ?? DefaultDxc;

    /// <summary>The folder of extracted .fssm programs, or null.</summary>
    public static string? ShaderFolder
    {
        get
        {
            string? folder = Environment.GetEnvironmentVariable(ShaderFolderVariable);

            if (string.IsNullOrWhiteSpace(folder) || !Directory.Exists(folder))
            {
                return null;
            }

            return folder;
        }
    }

    private static string? MissingReason()
    {
        if (DumpFactAttribute.Folder is null || ShaderFolder is null)
        {
            return $"{DumpFactAttribute.Variable} and {ShaderFolderVariable} are not both set";
        }

        if (!File.Exists(Path.Combine(Environment.SystemDirectory, WarpLibrary)))
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
