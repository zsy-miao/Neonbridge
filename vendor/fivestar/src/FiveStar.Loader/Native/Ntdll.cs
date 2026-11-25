// Copyright (c) 2026 Five Star Recomp contributors.
// SPDX-License-Identifier: GPL-3.0-or-later

using System.Runtime.InteropServices;

namespace FiveStar.Loader.Native;

/// <summary>The ntdll functions the loader calls: function tables, unwinding and process information.</summary>
internal static unsafe class Ntdll
{
    private const string Library = "ntdll";

    [DllImport(Library)]
    public static extern byte RtlAddFunctionTable(nint functionTable, uint entryCount, ulong baseAddress);

    [DllImport(Library)]
    public static extern byte RtlDeleteFunctionTable(nint functionTable);

    [DllImport(Library)]
    public static extern nint RtlLookupFunctionEntry(ulong controlPc, ulong* imageBase, nint historyTable);

    /// <summary>The real RtlPcToFileHeader; the shim of the same name falls back to it for addresses it does not own.</summary>
    [DllImport(Library)]
    public static extern nint RtlPcToFileHeader(nint pc, nint* baseOfImage);

    [DllImport(Library)]
    public static extern void RtlCaptureContext(byte* context);

    [DllImport(Library)]
    public static extern nint RtlVirtualUnwind(uint handlerType, ulong imageBase, ulong controlPc, nint functionEntry, byte* context, nint* handlerData, ulong* establisherFrame, nint contextPointers);

    [DllImport(Library)]
    public static extern int NtQueryInformationProcess(nint process, int informationClass, void* information, int length, int* returnLength);

    [DllImport(Library)]
    public static extern int NtQueryInformationThread(nint thread, int informationClass, void* information, int length, int* returnLength);

    /// <summary>Enters the loader lock, which thread attach and detach (with their TLS blocks) also hold.</summary>
    [DllImport(Library)]
    public static extern int LdrLockLoaderLock(uint flags, uint* disposition, nint* cookie);

    [DllImport(Library)]
    public static extern int LdrUnlockLoaderLock(uint flags, nint cookie);
}
