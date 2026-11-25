// Copyright (c) 2026 Five Star Recomp contributors.
// SPDX-License-Identifier: GPL-3.0-or-later

using System.Globalization;
using FiveStar.Debugger.Session;

namespace FiveStar.Debugger;

/// <summary>Reads the fivestar-dbg command line: options, then <c>--</c> (optional) and the program with its arguments.</summary>
internal sealed class OptionParser
{
    private const string ProgramSeparator = "--";
    private const string OptionPrefix = "-";
    private const string ProgramLogOption = "--log";
    private const char ListSeparator = ',';

    private readonly IReadOnlyList<string> _args;
    private readonly HashSet<uint> _breakOn = [];
    private int _next;
    private bool _debugHeap;
    private bool _children;
    private bool _fullDump;
    private string? _symbols;
    private string? _dumpFile;
    private string? _logFile;
    private string? _loaderLog;
    private TimeSpan? _timeout;

    public OptionParser(IReadOnlyList<string> args)
    {
        _args = args;
    }

    public DebuggerOptions Parse()
    {
        while (_next < _args.Count && _args[_next].StartsWith(OptionPrefix, StringComparison.Ordinal))
        {
            string option = _args[_next++];

            if (option == ProgramSeparator)
            {
                break;
            }

            Apply(option);
        }

        if (_next >= _args.Count)
        {
            throw new UsageException("no program to run");
        }

        string program = _args[_next];
        var arguments = _args.Skip(_next + 1).ToArray();
        string? loaderLog = _loaderLog ?? ProgramLog(arguments);

        return new DebuggerOptions(program, arguments, _debugHeap, _children, _breakOn, _symbols, _dumpFile, _fullDump, _logFile, loaderLog, _timeout);
    }

    /// <summary>The value of the program's own --log option: the loader's log lists the images it mapped.</summary>
    private static string? ProgramLog(IReadOnlyList<string> arguments)
    {
        for (int i = 0; i + 1 < arguments.Count; i++)
        {
            if (arguments[i] == ProgramLogOption)
            {
                return arguments[i + 1];
            }
        }

        return null;
    }

    private void Apply(string option)
    {
        switch (option)
        {
            case "--debug-heap":
                _debugHeap = true;
                break;
            case "--children":
                _children = true;
                break;
            case "--full-dump":
                _fullDump = true;
                break;
            case "--break-on":
                AddBreakCodes(Value(option));
                break;
            case "--symbols":
                _symbols = Value(option);
                break;
            case "--dump-on-crash":
                _dumpFile = Value(option);
                break;
            case "--log":
                _logFile = Value(option);
                break;
            case "--loader-log":
                _loaderLog = Value(option);
                break;
            case "--timeout":
                _timeout = Seconds(option, Value(option));
                break;
            default:
                throw new UsageException($"unknown option {option}");
        }
    }

    private string Value(string option)
    {
        if (_next >= _args.Count)
        {
            throw new UsageException($"{option} needs a value");
        }

        return _args[_next++];
    }

    private void AddBreakCodes(string list)
    {
        foreach (string item in list.Split(ListSeparator, StringSplitOptions.RemoveEmptyEntries | StringSplitOptions.TrimEntries))
        {
            if (!ExceptionCodes.TryParse(item, out uint code))
            {
                throw new UsageException($"--break-on: `{item}` is neither a hex exception code nor a known name");
            }

            _breakOn.Add(code);
        }
    }

    private static TimeSpan Seconds(string option, string text)
    {
        if (!double.TryParse(text, NumberStyles.Float, CultureInfo.InvariantCulture, out double seconds) || seconds <= 0)
        {
            throw new UsageException($"{option} needs a positive number of seconds");
        }

        return TimeSpan.FromSeconds(seconds);
    }
}
