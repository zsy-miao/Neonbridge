// Copyright (c) 2026 Five Star Recomp contributors.
// SPDX-License-Identifier: GPL-3.0-or-later

using System.Runtime.InteropServices;

namespace FiveStar.Debugger.Tests;

/// <summary>
/// The program the tests run under the debugger. Both faults happen inside ntdll, so they have a module and exported
/// symbols to report without any PDB.
/// </summary>
public static partial class CrashTarget
{
    public const string AccessViolation = "access-violation";
    public const string HeapOverrun = "heap-overrun";

    /// <summary>
    /// The block the overrun writes one byte past. A multiple of 16, so the page heap puts its end right at the guard
    /// page and the write itself faults.
    /// </summary>
    private const int BlockSize = 32;

    private const byte Fill = 0xAB;

    public static int Main(string[] args)
    {
        if (args.Length == 1 && args[0] == AccessViolation)
        {
            // A write to the first page, which is never mapped.
            RtlFillMemory(0x10, 16, Fill);
        }

        if (args.Length == 1 && args[0] == HeapOverrun)
        {
            nint heap = GetProcessHeap();
            nint block = HeapAlloc(heap, 0, BlockSize);
            RtlFillMemory(block, BlockSize + 1, Fill);
            _ = HeapFree(heap, 0, block);
        }

        return 0;
    }

    [LibraryImport("ntdll")]
    private static partial void RtlFillMemory(nint destination, nuint length, byte fill);

    [LibraryImport("kernel32")]
    private static partial nint GetProcessHeap();

    [LibraryImport("kernel32")]
    private static partial nint HeapAlloc(nint heap, uint flags, nuint bytes);

    [LibraryImport("kernel32")]
    [return: MarshalAs(UnmanagedType.Bool)]
    private static partial bool HeapFree(nint heap, uint flags, nint block);
}
