// Copyright (c) 2026 Five Star Recomp contributors.
// SPDX-License-Identifier: GPL-3.0-or-later

using Xunit;

namespace FiveStar.Shaders.Tests.Toolchain;

/// <summary>
/// A test on real programs of the user's own dump: skipped unless FIVESTAR_DUMP is set and FIVESTAR_SHADER_DIR names
/// the folder of extracted .fssm programs (with their containers in the sibling dxbc folder), or when the tools are missing.
/// </summary>
[AttributeUsage(AttributeTargets.Method)]
public sealed class RealProgramTheoryAttribute : TheoryAttribute
{
    public const string DumpVariable = "FIVESTAR_DUMP";
    public const string ShaderFolderVariable = "FIVESTAR_SHADER_DIR";

    public RealProgramTheoryAttribute()
    {
        if (string.IsNullOrWhiteSpace(Environment.GetEnvironmentVariable(DumpVariable)))
        {
            Skip = $"{DumpVariable} is not set";
        }
        else if (Folder is null)
        {
            Skip = $"{ShaderFolderVariable} does not name a folder of extracted .fssm programs";
        }
        else
        {
            Skip = ShaderToolchain.MissingReason();
        }
    }

    /// <summary>The folder of extracted programs, or null.</summary>
    public static string? Folder
    {
        get
        {
            var folder = Environment.GetEnvironmentVariable(ShaderFolderVariable);

            if (string.IsNullOrWhiteSpace(folder) || !Directory.Exists(folder))
            {
                return null;
            }

            return folder;
        }
    }
}
