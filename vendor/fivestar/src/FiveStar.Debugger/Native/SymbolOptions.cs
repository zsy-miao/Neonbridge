// Copyright (c) 2026 Five Star Recomp contributors.
// SPDX-License-Identifier: GPL-3.0-or-later

namespace FiveStar.Debugger.Native;

/// <summary>The SYMOPT_* flags the debugger sets (dbghelp.h).</summary>
[Flags]
internal enum SymbolOptions : uint
{
    None = 0,

    /// <summary>SYMOPT_UNDNAME: undecorated names.</summary>
    UndecoratedNames = 0x2,

    /// <summary>SYMOPT_DEFERRED_LOADS: symbols load on the first lookup in a module.</summary>
    DeferredLoads = 0x4,

    /// <summary>SYMOPT_FAIL_CRITICAL_ERRORS: no system error dialogs.</summary>
    FailCriticalErrors = 0x200,

    /// <summary>SYMOPT_NO_PROMPTS: no proxy or server prompts.</summary>
    NoPrompts = 0x80000,
}
