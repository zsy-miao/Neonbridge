// Copyright (c) 2026 Five Star Recomp contributors.
// SPDX-License-Identifier: GPL-3.0-or-later

using System.Runtime.InteropServices;

namespace FiveStar.Debugger.Native;

/// <summary>
/// The dbghelp functions of the debugger. Windows ships dbghelp.dll in System32; it is single-threaded, so the debugger
/// calls it from its event loop only.
/// </summary>
internal static unsafe partial class DbgHelp
{
    public const string Library = "dbghelp.dll";

    /// <summary>IMAGE_FILE_MACHINE_AMD64 (winnt.h), the machine type of <see cref="StackWalk64"/>.</summary>
    public const uint MachineAmd64 = 0x8664;

    [LibraryImport(Library)]
    public static partial SymbolOptions SymSetOptions(SymbolOptions options);

    [LibraryImport(Library, EntryPoint = "SymInitializeW", SetLastError = true)]
    public static partial int SymInitialize(nint process, char* searchPath, int invadeProcess);

    [LibraryImport(Library, SetLastError = true)]
    public static partial int SymCleanup(nint process);

    [LibraryImport(Library, EntryPoint = "SymSetSearchPathW", SetLastError = true)]
    public static partial int SymSetSearchPath(nint process, char* searchPath);

    /// <summary>SymLoadModuleExW; returns the module base, or 0 on failure.</summary>
    [LibraryImport(Library, EntryPoint = "SymLoadModuleExW", SetLastError = true)]
    public static partial ulong SymLoadModuleEx(nint process, nint file, char* imageName, char* moduleName, ulong baseOfDll, uint dllSize, nint data, uint flags);

    [LibraryImport(Library, SetLastError = true)]
    public static partial int SymUnloadModule64(nint process, ulong baseOfDll);

    [LibraryImport(Library, EntryPoint = "SymFromAddrW", SetLastError = true)]
    public static partial int SymFromAddr(nint process, ulong address, ulong* displacement, SymbolInfo* symbol);

    /// <summary>
    /// StackWalk64. The routines are native function pointers: 0 for memory reads (ReadProcessMemory), and dbghelp's own
    /// SymFunctionTableAccess64 and SymGetModuleBase64 (<see cref="DbgHelpExports"/>).
    /// </summary>
    [LibraryImport(Library, SetLastError = true)]
    public static partial int StackWalk64(
        uint machineType,
        nint process,
        nint thread,
        StackFrame64* frame,
        Amd64Context* context,
        nint readMemory,
        nint functionTableAccess,
        nint getModuleBase,
        nint translateAddress);

    [LibraryImport(Library, SetLastError = true)]
    public static partial int MiniDumpWriteDump(
        nint process,
        uint processId,
        nint file,
        MinidumpType dumpType,
        MinidumpExceptionInformation* exception,
        nint userStream,
        nint callback);
}
