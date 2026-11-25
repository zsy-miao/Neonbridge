// Copyright (c) 2026 Five Star Recomp contributors.
// SPDX-License-Identifier: GPL-3.0-or-later

using System.Runtime.InteropServices;

namespace FiveStar.Debugger.Native;

/// <summary>The kernel32 functions of the debugger. Every signature is blittable; remote addresses are 64-bit values.</summary>
internal static unsafe partial class Kernel32
{
    private const string Library = "kernel32";

    [LibraryImport(Library, EntryPoint = "CreateProcessW", SetLastError = true)]
    public static partial int CreateProcess(
        char* applicationName,
        char* commandLine,
        nint processAttributes,
        nint threadAttributes,
        int inheritHandles,
        ProcessCreationFlags creationFlags,
        char* environment,
        char* currentDirectory,
        StartupInfo* startupInfo,
        ProcessInformation* processInformation);

    [LibraryImport(Library, SetLastError = true)]
    public static partial int WaitForDebugEvent(DebugEvent* debugEvent, uint milliseconds);

    [LibraryImport(Library, SetLastError = true)]
    public static partial int ContinueDebugEvent(uint processId, uint threadId, ContinueStatus continueStatus);

    [LibraryImport(Library, SetLastError = true)]
    public static partial int TerminateProcess(nint process, uint exitCode);

    /// <summary>GetThreadContext: <paramref name="context"/> is 16-byte aligned with ContextFlags set.</summary>
    [LibraryImport(Library, SetLastError = true)]
    public static partial int GetThreadContext(nint thread, Amd64Context* context);

    [LibraryImport(Library, SetLastError = true)]
    public static partial int ReadProcessMemory(nint process, ulong address, void* buffer, nuint size, nuint* read);

    [LibraryImport(Library, SetLastError = true)]
    public static partial int WriteProcessMemory(nint process, ulong address, void* buffer, nuint size, nuint* written);

    [LibraryImport(Library, EntryPoint = "GetFinalPathNameByHandleW", SetLastError = true)]
    public static partial uint GetFinalPathNameByHandle(nint file, char* path, uint length, uint flags);

    [LibraryImport(Library, SetLastError = true)]
    public static partial int CloseHandle(nint handle);
}
