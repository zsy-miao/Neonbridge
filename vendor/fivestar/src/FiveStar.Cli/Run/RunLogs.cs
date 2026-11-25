// Copyright (c) 2026 Five Star Recomp contributors.
// SPDX-License-Identifier: GPL-3.0-or-later

using System.Globalization;

namespace FiveStar.Cli.Run;

/// <summary>The loader logs of <c>fivestar run</c>: <c>logs/run-&lt;yyyyMMdd-HHmmss&gt;.log</c>, the newest few kept.</summary>
internal static class RunLogs
{
    /// <summary>Logs kept, the new one included; a full log can reach gigabytes.</summary>
    public const int Kept = 5;

    private const string Prefix = "run-";
    private const string Extension = ".log";
    private const string StampFormat = "yyyyMMdd-HHmmss";

    /// <summary>Removes the older logs so that <see cref="Kept"/> remain with the new one; returns the new one's path.</summary>
    public static string Next(string folder, DateTimeOffset now)
    {
        Directory.CreateDirectory(folder);
        var old = Directory.GetFiles(folder, Prefix + "*" + Extension).Order(StringComparer.Ordinal).ToList();

        foreach (var path in old.Take(Math.Max(0, old.Count - Kept + 1)))
        {
            File.Delete(path);
        }

        return Path.Combine(folder, Prefix + now.ToString(StampFormat, CultureInfo.InvariantCulture) + Extension);
    }
}
