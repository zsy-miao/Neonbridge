// Copyright (c) 2026 Five Star Recomp contributors.
// SPDX-License-Identifier: GPL-3.0-or-later

using System.Runtime.InteropServices;

namespace FiveStar.Debugger.Native;

/// <summary>The addresses of dbghelp's own StackWalk64 helper routines, passed to it as function pointers.</summary>
internal sealed record DbgHelpExports(nint FunctionTableAccess, nint GetModuleBase)
{
    /// <summary>Resolves the exports from the same dbghelp.dll the <see cref="DbgHelp"/> imports bind to.</summary>
    public static DbgHelpExports Load()
    {
        nint library = NativeLibrary.Load(DbgHelp.Library, typeof(DbgHelp).Assembly, null);

        return new DbgHelpExports(
            NativeLibrary.GetExport(library, "SymFunctionTableAccess64"),
            NativeLibrary.GetExport(library, "SymGetModuleBase64"));
    }
}
