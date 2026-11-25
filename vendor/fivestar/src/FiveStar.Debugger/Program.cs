// Copyright (c) 2026 Five Star Recomp contributors.
// SPDX-License-Identifier: GPL-3.0-or-later

using System.ComponentModel;
using FiveStar.Debugger.Native;
using FiveStar.Debugger.Session;
using FiveStar.Debugger.Symbols;

namespace FiveStar.Debugger;

/// <summary>Entry point of fivestar-dbg.</summary>
public static class Program
{
    /// <summary>The command line could not be used, or the program could not be started.</summary>
    public const int UsageError = 2;

    private const string SymbolPathVariable = "_NT_SYMBOL_PATH";

    public static int Main(string[] args)
    {
        DebuggerOptions options;

        try
        {
            options = new OptionParser(args).Parse();
        }
        catch (UsageException e)
        {
            Console.Error.WriteLine($"fivestar-dbg: {e.Message}");
            Console.Error.WriteLine(Usage.Text);

            return UsageError;
        }

        if (!OperatingSystem.IsWindows() || !Environment.Is64BitProcess)
        {
            Console.Error.WriteLine("fivestar-dbg: Windows x64 only");

            return UsageError;
        }

        using var file = OpenLogFile(options.LogFile);
        using var log = new DebugLog(Console.Out, file);

        return Run(options, log);
    }

    /// <summary>Composes the session and runs it; the tests call this with their own log.</summary>
    internal static int Run(DebuggerOptions options, DebugLog log)
    {
        string defaultCache = Path.Combine(Environment.GetFolderPath(Environment.SpecialFolder.LocalApplicationData), "FiveStar", "Symbols");
        var symbolPath = SymbolPath.Parse(options.SymbolPath ?? Environment.GetEnvironmentVariable(SymbolPathVariable), defaultCache);
        using var downloader = new SymbolDownloader(symbolPath.Stores, log);
        var reporter = new ExceptionReporter(log, new StackWalker(DbgHelpExports.Load()));

        try
        {
            return new DebugSession(options, log, reporter, symbolPath, downloader).Run();
        }
        catch (Win32Exception e)
        {
            log.Write($"fivestar-dbg: {options.Program}: {e.Message}");

            return UsageError;
        }
    }

    private static StreamWriter? OpenLogFile(string? path)
    {
        if (path is null)
        {
            return null;
        }

        Directory.CreateDirectory(Path.GetDirectoryName(Path.GetFullPath(path))!);

        return new StreamWriter(path, false) { AutoFlush = true };
    }
}
