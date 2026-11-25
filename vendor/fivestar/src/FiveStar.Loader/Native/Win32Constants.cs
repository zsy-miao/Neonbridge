// Copyright (c) 2026 Five Star Recomp contributors.
// SPDX-License-Identifier: GPL-3.0-or-later

namespace FiveStar.Loader.Native;

/// <summary>Flag and error values of the Win32 calls the loader makes, named as in the Windows SDK headers.</summary>
internal static class Win32Constants
{
    // winnt.h: VirtualAlloc / VirtualFree allocation types.
    public const uint MemCommit = 0x1000;
    public const uint MemReserve = 0x2000;
    public const uint MemDecommit = 0x4000;
    public const uint MemRelease = 0x8000;
    public const uint MemReserveCommit = MemReserve | MemCommit;

    // winnt.h: page protections.
    public const uint PageNoAccess = 0x01;
    public const uint PageReadOnly = 0x02;
    public const uint PageReadWrite = 0x04;
    public const uint PageExecute = 0x10;
    public const uint PageExecuteRead = 0x20;
    public const uint PageExecuteReadWrite = 0x40;
    public const uint PageGuard = 0x100;

    /// <summary>The x64 page size (winnt.h PAGE_SIZE on AMD64).</summary>
    public const int PageSize = 0x1000;

    // libloaderapi.h: LoadLibraryEx flags.
    public const uint LoadLibraryAsDatafile = 0x2;
    public const uint LoadLibraryAsImageResource = 0x20;
    public const uint LoadLibraryAsDatafileExclusive = 0x40;
    public const uint LoadLibrarySearchSystem32 = 0x800;
    public const uint LoadLibraryDataFlags = LoadLibraryAsDatafile | LoadLibraryAsImageResource | LoadLibraryAsDatafileExclusive;

    // libloaderapi.h: GetModuleHandleEx flags.
    public const uint GetModuleHandleExFlagUnchangedRefcount = 0x2;
    public const uint GetModuleHandleExFlagFromAddress = 0x4;

    // winerror.h: system error codes.
    public const int ErrorSuccess = 0;
    public const int ErrorBadExeFormat = 193;
    public const int ErrorInsufficientBuffer = 122;
    public const int ErrorModNotFound = 126;
    public const int ErrorProcNotFound = 127;

    // winerror.h: errors of overlapped reads.
    public const int ErrorHandleEof = 38;
    public const int ErrorIoPending = 997;

    // winnt.h / fileapi.h: CreateFile access, sharing, disposition and flags.
    public const uint GenericRead = 0x80000000;
    public const uint FileShareRead = 0x1;
    public const uint OpenExisting = 3;
    public const uint FileFlagOverlapped = 0x40000000;
    public const uint FileFlagNoBuffering = 0x20000000;

    // winnt.h: DllMain reasons.
    public const uint DllProcessDetach = 0;
    public const uint DllProcessAttach = 1;

    /// <summary>winnls.h CP_ACP: the system ANSI code page.</summary>
    public const uint CodePageAnsi = 0;

    /// <summary>MAX_PATH (minwindef.h) in UTF-16 units, doubled for long paths the shims format.</summary>
    public const int PathBufferLength = 520;
}
