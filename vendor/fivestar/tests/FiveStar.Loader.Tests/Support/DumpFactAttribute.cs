// Copyright (c) 2026 Five Star Recomp contributors.
// SPDX-License-Identifier: GPL-3.0-or-later

using Xunit;

namespace FiveStar.Loader.Tests.Support;

/// <summary>A test on the user's own console dump: skipped unless FIVESTAR_DUMP names its folder.</summary>
[AttributeUsage(AttributeTargets.Method)]
public sealed class DumpFactAttribute : FactAttribute
{
    public const string Variable = "FIVESTAR_DUMP";

    public DumpFactAttribute()
    {
        if (Folder is null)
        {
            Skip = $"{Variable} is not set";
        }
    }

    /// <summary>The dump folder, or null when the variable is not set.</summary>
    public static string? Folder
    {
        get
        {
            string? folder = Environment.GetEnvironmentVariable(Variable);

            if (string.IsNullOrWhiteSpace(folder))
            {
                return null;
            }

            return folder;
        }
    }
}
