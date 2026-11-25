// Copyright (c) 2026 Five Star Recomp contributors.
// SPDX-License-Identifier: GPL-3.0-or-later

using System.Diagnostics;

namespace FiveStar.Shaders.Tooling;

/// <summary>Runs a command-line tool, captures its output and kills it when it exceeds its time limit.</summary>
public static class ExternalTool
{
    /// <summary>Runs <paramref name="path"/> with the arguments; a null timeout waits forever.</summary>
    public static ToolResult Run(string path, IEnumerable<string> arguments, TimeSpan? timeout)
    {
        var start = new ProcessStartInfo(path)
        {
            RedirectStandardOutput = true,
            RedirectStandardError = true,
            UseShellExecute = false,
            CreateNoWindow = true,
        };

        foreach (var argument in arguments)
        {
            start.ArgumentList.Add(argument);
        }

        using var process = Process.Start(start) ?? throw new InvalidOperationException($"cannot start {path}");
        var output = process.StandardOutput.ReadToEndAsync();
        var error = process.StandardError.ReadToEndAsync();
        var limit = timeout ?? Timeout.InfiniteTimeSpan;

        if (!process.WaitForExit(limit))
        {
            process.Kill(entireProcessTree: true);
            process.WaitForExit();

            return new ToolResult(-1, output.Result, error.Result, TimedOut: true);
        }

        process.WaitForExit();

        return new ToolResult(process.ExitCode, output.Result, error.Result, TimedOut: false);
    }
}
