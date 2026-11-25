// Copyright (c) 2026 Five Star Recomp contributors.
// SPDX-License-Identifier: GPL-3.0-or-later

using System.ComponentModel;
using System.Diagnostics;
using System.Runtime.InteropServices;
using System.Text;
using FiveStar.Debugger.Heap;
using FiveStar.Debugger.Native;
using FiveStar.Debugger.Symbols;

namespace FiveStar.Debugger.Session;

/// <summary>
/// Starts the program and runs the debug event loop on the calling thread until every debugged process has exited.
/// First-chance exceptions go back to the program's handlers; breakpoints after ntdll's startup breakpoint are reported
/// and stepped over; a second-chance exception is reported with every thread's stack and the optional minidump.
/// </summary>
internal sealed unsafe class DebugSession
{
    /// <summary>The program's exit code when --timeout ends it.</summary>
    public const uint TimeoutExitCode = 124;

    private const uint PollMilliseconds = 200;

    /// <summary>First-chance exceptions of one code logged as lines; the rest are only counted.</summary>
    private const int FirstChanceLinesPerCode = 20;

    private const int MaxDebugStringBytes = 0x2000;

    /// <summary>ERROR_SEM_TIMEOUT: WaitForDebugEvent saw no event in time.</summary>
    private const int WaitTimedOut = 121;

    private readonly DebuggerOptions _options;
    private readonly DebugLog _log;
    private readonly ExceptionReporter _reporter;
    private readonly SymbolPath _symbolPath;
    private readonly SymbolDownloader _downloader;
    private readonly MappedImageLog? _loaderLog;
    private readonly Dictionary<uint, DebuggeeProcess> _processes = [];
    private readonly Dictionary<uint, int> _firstChanceCounts = [];
    private readonly Stopwatch _clock = new();
    private uint _rootId;
    private uint? _rootExitCode;
    private bool _dumpWritten;
    private bool _timedOut;

    public DebugSession(DebuggerOptions options, DebugLog log, ExceptionReporter reporter, SymbolPath symbolPath, SymbolDownloader downloader)
    {
        _options = options;
        _log = log;
        _reporter = reporter;
        _symbolPath = symbolPath;
        _downloader = downloader;

        if (options.LoaderLog is not null)
        {
            _loaderLog = new MappedImageLog(options.LoaderLog);
        }
    }

    /// <summary>Runs the program to its end and returns its exit code.</summary>
    public int Run()
    {
        _rootId = ProcessLauncher.Start(_options);
        _clock.Start();
        _log.Write($"started process {_rootId}: {CommandLine.Build(_options.Program, _options.Arguments)}");
        _log.Write(HeapDescription());

        DebugEvent debugEvent;

        while (_rootExitCode is null || _processes.Count > 0)
        {
            CheckTimeout();

            if (Kernel32.WaitForDebugEvent(&debugEvent, PollMilliseconds) == 0)
            {
                ThrowUnlessTimedOut();

                continue;
            }

            var status = Dispatch(debugEvent);
            _ = Kernel32.ContinueDebugEvent(debugEvent.ProcessId, debugEvent.ThreadId, status);
        }

        LogSuppressedCounts();

        return unchecked((int)_rootExitCode.Value);
    }

    private static void ThrowUnlessTimedOut()
    {
        int error = Marshal.GetLastPInvokeError();

        if (error != WaitTimedOut)
        {
            throw new Win32Exception(error);
        }
    }

    private string HeapDescription()
    {
        if (_options.DebugHeap)
        {
            return "heap: ntdll debug heap (tail and free checks, parameter validation) and full page heap";
        }

        return $"heap: normal ({DebuggeeEnvironment.NoDebugHeapVariable}=1)";
    }

    private ContinueStatus Dispatch(DebugEvent debugEvent)
    {
        switch (debugEvent.Code)
        {
            case DebugEventCode.CreateProcess:
                return OnCreateProcess(debugEvent);
            case DebugEventCode.ExitProcess:
                return OnExitProcess(debugEvent);
            case DebugEventCode.CreateThread:
                return OnCreateThread(debugEvent);
            case DebugEventCode.ExitThread:
                return OnExitThread(debugEvent);
            case DebugEventCode.LoadDll:
                return OnLoadDll(debugEvent);
            case DebugEventCode.UnloadDll:
                return OnUnloadDll(debugEvent);
            case DebugEventCode.Exception:
                return OnException(debugEvent);
            case DebugEventCode.OutputDebugString:
                return OnDebugString(debugEvent);
            case DebugEventCode.Rip:
                return OnRip(debugEvent);
            default:
                return ContinueStatus.Continue;
        }
    }

    private ContinueStatus OnCreateProcess(DebugEvent debugEvent)
    {
        var info = debugEvent.CreateProcess;
        var symbols = new SymbolSession(info.Process, _symbolPath.Folders, _downloader);
        var process = new DebuggeeProcess(debugEvent.ProcessId, info.Process, symbols);
        _processes[debugEvent.ProcessId] = process;

        process.AddThread(debugEvent.ThreadId, info.Thread);
        var module = ModuleOf(process, info.File, info.BaseOfImage);
        process.AddModule(module);
        _log.Write($"{debugEvent.ProcessId}/{debugEvent.ThreadId} process {module.Name} @ 0x{module.Base:x} ({module.Path})");

        if (_options.DebugHeap)
        {
            string result = "on";

            if (!PageHeap.Enable(info.Process))
            {
                result = "could not be set";
            }

            _log.Write($"{debugEvent.ProcessId} page heap {result}");
        }

        return ContinueStatus.Continue;
    }

    private ContinueStatus OnExitProcess(DebugEvent debugEvent)
    {
        _log.Write($"{debugEvent.ProcessId} exited with 0x{debugEvent.ExitCode:x} ({ExceptionCodes.Describe(debugEvent.ExitCode)})");

        if (debugEvent.ProcessId == _rootId)
        {
            _rootExitCode = debugEvent.ExitCode;
        }

        if (_processes.Remove(debugEvent.ProcessId, out var process))
        {
            process.Dispose();
        }

        return ContinueStatus.Continue;
    }

    private ContinueStatus OnCreateThread(DebugEvent debugEvent)
    {
        var process = _processes[debugEvent.ProcessId];
        process.AddThread(debugEvent.ThreadId, debugEvent.CreateThread.Thread);
        ulong start = debugEvent.CreateThread.StartAddress;
        _log.Write($"{debugEvent.ProcessId}/{debugEvent.ThreadId} thread at 0x{start:x} {process.Describe(start)}");

        return ContinueStatus.Continue;
    }

    private ContinueStatus OnExitThread(DebugEvent debugEvent)
    {
        _processes[debugEvent.ProcessId].RemoveThread(debugEvent.ThreadId);
        _log.Write($"{debugEvent.ProcessId}/{debugEvent.ThreadId} thread exited with 0x{debugEvent.ExitCode:x}");

        return ContinueStatus.Continue;
    }

    private ContinueStatus OnLoadDll(DebugEvent debugEvent)
    {
        var process = _processes[debugEvent.ProcessId];
        var module = ModuleOf(process, debugEvent.LoadDll.File, debugEvent.LoadDll.BaseOfDll);
        process.AddModule(module);
        _log.Write($"{debugEvent.ProcessId} module {module.Name} @ 0x{module.Base:x} ({module.Path})");

        return ContinueStatus.Continue;
    }

    private ContinueStatus OnUnloadDll(DebugEvent debugEvent)
    {
        var module = _processes[debugEvent.ProcessId].RemoveModule(debugEvent.UnloadedBase);

        if (module is not null)
        {
            _log.Write($"{debugEvent.ProcessId} unloaded {module.Name}");
        }

        return ContinueStatus.Continue;
    }

    private ContinueStatus OnException(DebugEvent debugEvent)
    {
        var process = _processes[debugEvent.ProcessId];
        var record = debugEvent.Exception.Record;
        bool firstChance = debugEvent.Exception.FirstChance != 0;
        bool breakpoint = ExceptionCodes.IsBreakpoint(record.Code);

        if (breakpoint && !process.StartupBreakpointSeen)
        {
            process.StartupBreakpointSeen = true;
            _log.Write($"{process.Id}/{debugEvent.ThreadId} startup breakpoint");

            return ContinueStatus.Continue;
        }

        RefreshMappedImages(process);

        if (!firstChance)
        {
            return OnSecondChance(process, debugEvent.ThreadId, record);
        }

        if (_options.BreakOn.Contains(record.Code))
        {
            _reporter.Record(process, debugEvent.ThreadId, record, true, false);
            WriteDumpOnce(process, debugEvent.ThreadId, record);
        }
        else if (breakpoint)
        {
            _reporter.Record(process, debugEvent.ThreadId, record, true, false);
        }
        else
        {
            LogFirstChance(process, debugEvent.ThreadId, record);
        }

        // A breakpoint is stepped over, as a debugger's go does; everything else goes to the program's handlers.
        if (breakpoint)
        {
            return ContinueStatus.Continue;
        }

        return ContinueStatus.ExceptionNotHandled;
    }

    private ContinueStatus OnSecondChance(DebuggeeProcess process, uint threadId, ExceptionRecord record)
    {
        _reporter.Record(process, threadId, record, false, true);
        WriteDumpOnce(process, threadId, record);

        return ContinueStatus.ExceptionNotHandled;
    }

    /// <summary>
    /// The minidump of --dump-on-crash, at the first second-chance or --break-on exception: a .NET host usually ends
    /// the process on an unhandled exception without a second chance, so the first-chance stop is the last one.
    /// </summary>
    private void WriteDumpOnce(DebuggeeProcess process, uint threadId, ExceptionRecord record)
    {
        string? path = _options.DumpFile;

        if (path is null || _dumpWritten || !process.Threads.TryGetValue(threadId, out nint thread))
        {
            return;
        }

        _dumpWritten = true;
        using var context = ThreadContext.Capture(thread);

        if (context is null)
        {
            _log.Write($"dump: no context for thread {threadId}");

            return;
        }

        int error = CrashDumpWriter.Write(path, process, threadId, record, context, _options.FullDump);

        if (error != 0)
        {
            _log.Write($"dump: MiniDumpWriteDump failed with error {error}");

            return;
        }

        _log.Write($"dump: {Path.GetFullPath(path)}");
    }

    private void LogFirstChance(DebuggeeProcess process, uint threadId, ExceptionRecord record)
    {
        int count = _firstChanceCounts.GetValueOrDefault(record.Code) + 1;
        _firstChanceCounts[record.Code] = count;

        if (count <= FirstChanceLinesPerCode)
        {
            _reporter.Line(process, threadId, record, true);
        }

        if (count == FirstChanceLinesPerCode)
        {
            _log.Write($"further first-chance {ExceptionCodes.Describe(record.Code)} are counted, not logged");
        }
    }

    private void LogSuppressedCounts()
    {
        foreach (var (code, count) in _firstChanceCounts)
        {
            if (count > FirstChanceLinesPerCode)
            {
                _log.Write($"first-chance {ExceptionCodes.Describe(code)}: {count} in all");
            }
        }
    }

    private ContinueStatus OnDebugString(DebugEvent debugEvent)
    {
        var info = debugEvent.DebugString;
        int length = Math.Min((int)info.Length, MaxDebugStringBytes);
        var bytes = new byte[length];
        int read = RemoteMemory.Read(_processes[debugEvent.ProcessId].Handle, info.Data, bytes);
        Encoding encoding = Encoding.Latin1;

        if (info.Unicode != 0)
        {
            encoding = Encoding.Unicode;
        }

        string text = encoding.GetString(bytes, 0, read).TrimEnd('\0', '\r', '\n');

        foreach (string line in text.Split('\n'))
        {
            _log.Write($"{debugEvent.ProcessId}/{debugEvent.ThreadId} debug: {line.TrimEnd('\r')}");
        }

        return ContinueStatus.Continue;
    }

    private ContinueStatus OnRip(DebugEvent debugEvent)
    {
        _log.Write($"{debugEvent.ProcessId}/{debugEvent.ThreadId} debugging ended by system error {debugEvent.Rip.Error} (type {debugEvent.Rip.Type})");

        return ContinueStatus.Continue;
    }

    private void RefreshMappedImages(DebuggeeProcess process)
    {
        if (_loaderLog is null)
        {
            return;
        }

        _loaderLog.Refresh();
        process.Merge(_loaderLog.Images);
    }

    private void CheckTimeout()
    {
        if (_timedOut || _options.Timeout is null || _clock.Elapsed < _options.Timeout.Value)
        {
            return;
        }

        _timedOut = true;
        _log.Write($"timeout after {_options.Timeout.Value.TotalSeconds:F0} s: ending the program");

        foreach (var process in _processes.Values)
        {
            _ = Kernel32.TerminateProcess(process.Handle, TimeoutExitCode);
        }
    }

    private static LoadedModule ModuleOf(DebuggeeProcess process, nint file, ulong imageBase)
    {
        string path = RemoteImage.TakePath(file) ?? $"image@{imageBase:x}";

        return new LoadedModule(Path.GetFileName(path), path, imageBase, RemoteImage.SizeOf(process.Handle, imageBase), false);
    }
}
