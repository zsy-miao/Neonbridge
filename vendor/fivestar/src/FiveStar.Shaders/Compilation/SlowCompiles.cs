// Copyright (c) 2026 Five Star Recomp contributors.
// SPDX-License-Identifier: GPL-3.0-or-later

using System.Globalization;

namespace FiveStar.Shaders.Compilation;

/// <summary>
/// The sources whose optimized compile timed out, or took longer than a limit, in <see cref="TimedCompiler"/> logs
/// (<c>compile-times.tsv</c>): compiled with -Od from the start next time, since the driver optimizes the DXIL anyway.
/// </summary>
public sealed class SlowCompiles
{
    private const int Columns = 7;
    private const int FileColumn = 0;
    private const int OptimizationColumn = 2;
    private const int MillisecondsColumn = 5;
    private const int ResultColumn = 6;
    private const string Optimized = "-O3";
    private const string TimedOut = "timeout";

    private readonly HashSet<string> _files;

    private SlowCompiles(HashSet<string> files)
    {
        _files = files;
    }

    /// <summary>Number of sources named.</summary>
    public int Count => _files.Count;

    /// <summary>The slow sources of the logs that exist; a damaged line is skipped.</summary>
    public static SlowCompiles Read(IEnumerable<string> logs, TimeSpan limit)
    {
        var files = new HashSet<string>(StringComparer.OrdinalIgnoreCase);

        foreach (var log in logs.Where(File.Exists))
        {
            using var stream = new FileStream(log, FileMode.Open, FileAccess.Read, FileShare.ReadWrite | FileShare.Delete);
            using var reader = new StreamReader(stream);

            string? line;

            while ((line = reader.ReadLine()) is not null)
            {
                var fields = line.Split('\t');

                if (fields.Length == Columns && fields[OptimizationColumn] == Optimized && IsSlow(fields, limit))
                {
                    files.Add(fields[FileColumn]);
                }
            }
        }

        return new SlowCompiles(files);
    }

    /// <summary>Whether the source file (name with extension, as the log has it) is slow at -O3.</summary>
    public bool Contains(string hlslFile)
    {
        return _files.Contains(hlslFile);
    }

    private static bool IsSlow(string[] fields, TimeSpan limit)
    {
        if (fields[ResultColumn] == TimedOut)
        {
            return true;
        }

        return long.TryParse(fields[MillisecondsColumn], NumberStyles.None, CultureInfo.InvariantCulture, out var milliseconds) && milliseconds >= limit.TotalMilliseconds;
    }
}
