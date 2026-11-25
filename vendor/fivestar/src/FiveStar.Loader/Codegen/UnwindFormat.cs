// Copyright (c) 2026 Five Star Recomp contributors.
// SPDX-License-Identifier: GPL-3.0-or-later

namespace FiveStar.Loader.Codegen;

/// <summary>x64 exception-handling data layout ("x64 exception handling" in the Microsoft C++ ABI documentation).</summary>
internal static class UnwindFormat
{
    /// <summary>UNWIND_INFO version 1 in the low 3 bits of the first byte; the flags follow.</summary>
    public const byte Version1 = 1;
    public const int FlagsShift = 3;
    public const byte FlagExceptionHandler = 1;
    public const byte FlagTerminationHandler = 2;

    /// <summary>UWOP_ALLOC_SMALL: allocates 8 to 128 bytes; the op info is (size - 8) / 8.</summary>
    public const byte OpAllocSmall = 2;
    public const int OpInfoShift = 4;
    public const int AllocSmallUnit = 8;

    /// <summary>UNWIND_INFO and its code array start on a 4-byte boundary.</summary>
    public const int Alignment = 4;

    /// <summary>RUNTIME_FUNCTION: BeginAddress, EndAddress, UnwindInfoAddress.</summary>
    public const int RuntimeFunctionSize = 12;

    /// <summary>RUNTIME_FUNCTION as 32-bit fields.</summary>
    public const int RuntimeFunctionFields = RuntimeFunctionSize / sizeof(uint);
}
