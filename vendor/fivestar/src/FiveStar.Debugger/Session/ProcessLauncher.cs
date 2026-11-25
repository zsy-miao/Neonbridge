// Copyright (c) 2026 Five Star Recomp contributors.
// SPDX-License-Identifier: GPL-3.0-or-later

using System.ComponentModel;
using FiveStar.Debugger.Heap;
using FiveStar.Debugger.Native;

namespace FiveStar.Debugger.Session;

/// <summary>Starts the program as a debuggee of the calling thread, which must then run the debug event loop.</summary>
internal static unsafe class ProcessLauncher
{
    /// <summary>Returns the process id; the creation handles are closed, the debug events bring their own.</summary>
    public static uint Start(DebuggerOptions options)
    {
        var flags = ProcessCreationFlags.CreateUnicodeEnvironment | ProcessCreationFlags.DebugOnlyThisProcess;

        if (options.Children)
        {
            flags = ProcessCreationFlags.CreateUnicodeEnvironment | ProcessCreationFlags.DebugProcess;
        }

        char[] commandLine = (CommandLine.Build(options.Program, options.Arguments) + '\0').ToCharArray();
        string environment = DebuggeeEnvironment.Build(Environment.GetEnvironmentVariables(), options.DebugHeap);
        var startup = new StartupInfo { Size = (uint)sizeof(StartupInfo) };
        ProcessInformation created;
        int ok;

        fixed (char* command = commandLine)
        {
            fixed (char* block = environment)
            {
                ok = Kernel32.CreateProcess(null, command, 0, 0, 0, flags, block, null, &startup, &created);
            }
        }

        if (ok == 0)
        {
            throw new Win32Exception();
        }

        _ = Kernel32.CloseHandle(created.Thread);
        _ = Kernel32.CloseHandle(created.Process);

        return created.ProcessId;
    }
}
