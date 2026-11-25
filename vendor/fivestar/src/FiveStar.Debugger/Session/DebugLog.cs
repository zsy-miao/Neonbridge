// Copyright (c) 2026 Five Star Recomp contributors.
// SPDX-License-Identifier: GPL-3.0-or-later

using System.Diagnostics;
using System.Globalization;

namespace FiveStar.Debugger.Session;

/// <summary>The debugger's output: each line prefixed with the seconds since the start, to the console and the optional file.</summary>
internal sealed class DebugLog : IDisposable
{
    /// <summary>Seconds right-aligned to this width, so the lines of a long run stay in columns.</summary>
    private const int TimeWidth = 8;

    private readonly TextWriter _console;
    private readonly TextWriter? _file;
    private readonly Stopwatch _clock = Stopwatch.StartNew();

    public DebugLog(TextWriter console, TextWriter? file)
    {
        _console = console;
        _file = file;
    }

    public void Write(string text)
    {
        string seconds = _clock.Elapsed.TotalSeconds.ToString("F3", CultureInfo.InvariantCulture).PadLeft(TimeWidth);
        string line = $"[{seconds}] {text}";
        _console.WriteLine(line);
        _file?.WriteLine(line);
    }

    public void Dispose()
    {
        _file?.Dispose();
    }
}
