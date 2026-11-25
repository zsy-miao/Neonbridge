// Copyright (c) 2026 Five Star Recomp contributors.
// SPDX-License-Identifier: GPL-3.0-or-later

using System.Runtime.InteropServices;
using FiveStar.Debugger.Native;

namespace FiveStar.Debugger.Session;

/// <summary>Writes a minidump of a stopped debuggee with the exception that stopped it.</summary>
internal static unsafe class CrashDumpWriter
{
    /// <summary>Threads, stacks, data segments, the memory they point to, handles and the memory map.</summary>
    private const MinidumpType Standard =
        MinidumpType.WithDataSegments | MinidumpType.WithHandleData | MinidumpType.WithUnloadedModules
        | MinidumpType.WithIndirectlyReferencedMemory | MinidumpType.WithProcessThreadData | MinidumpType.WithFullMemoryInfo
        | MinidumpType.WithThreadInfo;

    /// <summary>Returns the Win32 error, or 0 when the dump was written.</summary>
    public static int Write(string path, DebuggeeProcess process, uint threadId, ExceptionRecord record, ThreadContext context, bool fullMemory)
    {
        var type = Standard;

        if (fullMemory)
        {
            type |= MinidumpType.WithFullMemory;
        }

        var pointers = new ExceptionPointers { Record = &record, Context = context.Pointer };
        var exception = new MinidumpExceptionInformation { ThreadId = threadId, ExceptionPointers = (nint)(&pointers), ClientPointers = 0 };
        Directory.CreateDirectory(Path.GetDirectoryName(Path.GetFullPath(path))!);

        using var file = new FileStream(path, FileMode.Create, FileAccess.ReadWrite, FileShare.None);

        if (DbgHelp.MiniDumpWriteDump(process.Handle, process.Id, file.SafeFileHandle.DangerousGetHandle(), type, &exception, 0, 0) == 0)
        {
            return Marshal.GetLastPInvokeError();
        }

        return 0;
    }
}
