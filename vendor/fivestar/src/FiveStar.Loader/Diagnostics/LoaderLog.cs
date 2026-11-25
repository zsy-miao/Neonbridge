// Copyright (c) 2026 Five Star Recomp contributors.
// SPDX-License-Identifier: GPL-3.0-or-later

namespace FiveStar.Loader.Diagnostics;

/// <summary>
/// The loader's thread-safe log. The console shows information, warnings and errors, and details only when verbose;
/// the optional file receives every line.
/// </summary>
internal sealed class LoaderLog
{
    private const string WarningPrefix = "WARN  ";
    private const string ErrorPrefix = "ERROR ";

    private readonly object _gate = new();
    private readonly TextWriter _console;
    private readonly TextWriter? _file;
    private readonly bool _verbose;

    public LoaderLog(TextWriter console, TextWriter? file, bool verbose)
    {
        _console = console;
        _file = file;
        _verbose = verbose;
    }

    /// <summary>Whether <see cref="Detail"/> writes anywhere; hot paths test it before formatting a message.</summary>
    public bool DetailEnabled => _verbose || _file is not null;

    public void Info(string message)
    {
        Write(message, true);
    }

    /// <summary>A line for the log file; on the console only when verbose.</summary>
    public void Detail(string message)
    {
        Write(message, _verbose);
    }

    public void Warn(string message)
    {
        Write(WarningPrefix + message, true);
    }

    public void Error(string message)
    {
        Write(ErrorPrefix + message, true);
    }

    private void Write(string message, bool toConsole)
    {
        lock (_gate)
        {
            if (toConsole)
            {
                _console.WriteLine(message);
            }

            _file?.WriteLine(message);
        }
    }
}
