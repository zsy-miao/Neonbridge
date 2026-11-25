// Copyright (c) 2026 Five Star Recomp contributors.
// SPDX-License-Identifier: GPL-3.0-or-later

using System.Runtime.InteropServices;

namespace FiveStar.Debugger.Native;

/// <summary>The ntdll functions of the debugger.</summary>
internal static unsafe partial class Ntdll
{
    private const string Library = "ntdll";

    /// <summary>NtQueryInformationProcess; returns an NTSTATUS, 0 on success.</summary>
    [LibraryImport(Library)]
    public static partial int NtQueryInformationProcess(nint process, int informationClass, void* information, int length, int* returnLength);
}
