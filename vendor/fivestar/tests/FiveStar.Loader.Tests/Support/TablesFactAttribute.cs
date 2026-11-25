// Copyright (c) 2026 Five Star Recomp contributors.
// SPDX-License-Identifier: GPL-3.0-or-later

using Xunit;

namespace FiveStar.Loader.Tests.Support;

/// <summary>
/// A test on the user's dump with the user's BCPack tables: skipped unless FIVESTAR_DUMP names the dump and
/// FIVESTAR_BCPACK_TABLES the table file (the project does not ship it).
/// </summary>
[AttributeUsage(AttributeTargets.Method)]
public sealed class TablesFactAttribute : FactAttribute
{
    public const string Variable = "FIVESTAR_BCPACK_TABLES";

    public TablesFactAttribute()
    {
        if (DumpFactAttribute.Folder is null)
        {
            Skip = $"{DumpFactAttribute.Variable} is not set";
        }
        else if (Path is null)
        {
            Skip = $"{Variable} is not set";
        }
    }

    /// <summary>Gets the table file, or null when the variable is not set.</summary>
    public static string? Path
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
