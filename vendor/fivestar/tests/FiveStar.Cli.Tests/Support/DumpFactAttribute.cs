// Copyright (c) 2026 Five Star Recomp contributors.
// SPDX-License-Identifier: GPL-3.0-or-later

using Xunit;

namespace FiveStar.Cli.Tests.Support;

/// <summary>A test on the user's own console dump: skipped unless FIVESTAR_DUMP names its folder.</summary>
[AttributeUsage(AttributeTargets.Method)]
public sealed class DumpFactAttribute : FactAttribute
{
    public const string Variable = "FIVESTAR_DUMP";
    public const string ShaderFolderVariable = "FIVESTAR_SHADER_DIR";

    public DumpFactAttribute()
    {
        if (Folder is null)
        {
            Skip = $"{Variable} is not set";
        }
    }

    /// <summary>Gets the dump folder, or null when the variable is not set.</summary>
    public static string? Folder => Existing(Environment.GetEnvironmentVariable(Variable));

    /// <summary>Gets the folder of extracted .fssm programs, or null when FIVESTAR_SHADER_DIR does not name one.</summary>
    public static string? ShaderFolder => Existing(Environment.GetEnvironmentVariable(ShaderFolderVariable));

    private static string? Existing(string? folder)
    {
        if (string.IsNullOrWhiteSpace(folder) || !Directory.Exists(folder))
        {
            return null;
        }

        return folder;
    }
}
