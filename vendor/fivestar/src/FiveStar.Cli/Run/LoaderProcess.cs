// Copyright (c) 2026 Five Star Recomp contributors.
// SPDX-License-Identifier: GPL-3.0-or-later

using System.Diagnostics;
using System.Text;

namespace FiveStar.Cli.Run;

/// <summary>
/// Starts <c>fivestar-loader</c> and passes the lines <see cref="LoaderOutput"/> keeps to the console while it runs.
/// Ctrl+C reaches the loader (same console); this process then waits for it to end so the summary still prints.
/// </summary>
internal static class LoaderProcess
{
    /// <summary>Runs the loader to its end; returns its exit code.</summary>
    public static int Run(string loader, IReadOnlyList<string> arguments, string folder, LoaderOutput output, TextWriter console)
    {
        var start = new ProcessStartInfo(loader)
        {
            UseShellExecute = false,
            RedirectStandardOutput = true,
            StandardOutputEncoding = Encoding.UTF8,
            WorkingDirectory = folder,
        };

        foreach (var argument in arguments)
        {
            start.ArgumentList.Add(argument);
        }

        ConsoleCancelEventHandler wait = (_, args) => args.Cancel = true;
        Console.CancelKeyPress += wait;

        try
        {
            using var process = Process.Start(start) ?? throw new InvalidOperationException($"cannot start {loader}");
            string? line;

            while ((line = process.StandardOutput.ReadLine()) is not null)
            {
                if (output.Observe(line))
                {
                    console.WriteLine(line);
                }
            }

            process.WaitForExit();

            return process.ExitCode;
        }
        finally
        {
            Console.CancelKeyPress -= wait;
        }
    }
}
