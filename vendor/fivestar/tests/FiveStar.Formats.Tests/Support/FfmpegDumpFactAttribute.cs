// Copyright (c) 2026 Five Star Recomp contributors.
// SPDX-License-Identifier: GPL-3.0-or-later

using Xunit;

namespace FiveStar.Formats.Tests.Support;

/// <summary>A test against FFmpeg on the dump: skipped unless FIVESTAR_DUMP is set and FIVESTAR_FFMPEG names ffmpeg.</summary>
[AttributeUsage(AttributeTargets.Method)]
public sealed class FfmpegDumpFactAttribute : FactAttribute
{
    public const string Variable = "FIVESTAR_FFMPEG";

    public FfmpegDumpFactAttribute()
    {
        if (DumpFactAttribute.Folder is null)
        {
            Skip = $"{DumpFactAttribute.Variable} is not set";
        }
        else if (Program is null)
        {
            Skip = $"{Variable} does not name an ffmpeg executable";
        }
    }

    /// <summary>Gets the ffmpeg path, or null when the variable is not set or names no file.</summary>
    public static string? Program
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
