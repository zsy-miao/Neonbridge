// Copyright (c) 2026 Five Star Recomp contributors.
// SPDX-License-Identifier: GPL-3.0-or-later

using Xunit;

namespace FiveStar.Cli.Tests.Support;

/// <summary>A test on the dump and the PC GDK library: skipped unless FIVESTAR_DUMP and FIVESTAR_GDK_LIB are both set.</summary>
[AttributeUsage(AttributeTargets.Method)]
public sealed class GdkDumpFactAttribute : FactAttribute
{
    public const string Variable = "FIVESTAR_GDK_LIB";

    public GdkDumpFactAttribute()
    {
        if (DumpFactAttribute.Folder is null)
        {
            Skip = $"{DumpFactAttribute.Variable} is not set";
        }
        else if (Library is null)
        {
            Skip = $"{Variable} is not set";
        }
    }

    /// <summary>Gets the library path, or null when the variable is not set.</summary>
    public static string? Library
    {
        get
        {
            var path = Environment.GetEnvironmentVariable(Variable);

            if (string.IsNullOrWhiteSpace(path) || !File.Exists(path))
            {
                return null;
            }

            return path;
        }
    }
}
