// Copyright (c) 2026 Five Star Recomp contributors.
// SPDX-License-Identifier: GPL-3.0-or-later

namespace FiveStar.Debugger.Session;

/// <summary>Exception codes the debugger names in its output and accepts by name in --break-on (ntstatus.h, winnt.h).</summary>
internal enum WellKnownException : uint
{
    GuardPage = 0x80000001,
    DataTypeMisalignment = 0x80000002,
    Breakpoint = 0x80000003,
    SingleStep = 0x80000004,

    /// <summary>STATUS_WX86_BREAKPOINT: the breakpoint of a 32-bit process under WOW64.</summary>
    Wx86Breakpoint = 0x4000001F,

    /// <summary>MS_VC_EXCEPTION: SetThreadName through RaiseException.</summary>
    SetThreadName = 0x406D1388,
    AccessViolation = 0xC0000005,
    InPageError = 0xC0000006,
    InvalidHandle = 0xC0000008,
    IllegalInstruction = 0xC000001D,
    IntegerDivideByZero = 0xC0000094,
    PrivilegedInstruction = 0xC0000096,
    StackOverflow = 0xC00000FD,
    HeapCorruption = 0xC0000374,

    /// <summary>STATUS_STACK_BUFFER_OVERRUN: also every __fastfail.</summary>
    StackBufferOverrun = 0xC0000409,
    InvalidCrtParameter = 0xC0000417,
    AssertionFailure = 0xC0000420,

    /// <summary>STATUS_VERIFIER_STOP: the page heap (Application Verifier) ended the process.</summary>
    VerifierStop = 0xC0000421,

    /// <summary>CLRDBG_NOTIFICATION_EXCEPTION_CODE: the .NET runtime announcing itself to a debugger at startup.</summary>
    ClrDebuggerNotification = 0x04242420,

    /// <summary>The MSVC C++ throw ('msc').</summary>
    CppException = 0xE06D7363,

    /// <summary>A .NET exception ('COM').</summary>
    ClrException = 0xE0434352,
}
