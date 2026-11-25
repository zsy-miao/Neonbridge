// Copyright (c) 2026 Five Star Recomp contributors.
// SPDX-License-Identifier: GPL-3.0-or-later

using Xunit;

namespace FiveStar.Formats.Tests.Support;

/// <summary>
/// A test on the user's dump and the PC GDK's <c>xgameruntime.lib</c>: skipped unless FIVESTAR_DUMP names the dump folder
/// and FIVESTAR_GDK_LIB the library file.
/// </summary>
[AttributeUsage(AttributeTargets.Method)]
public sealed class GdkFactAttribute : FactAttribute
{
    public const string Variable = "FIVESTAR_GDK_LIB";

    public GdkFactAttribute()
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

            if (string.IsNullOrWhiteSpace(path))
            {
                return null;
            }

            return path;
        }
    }
}
