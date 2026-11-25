// Copyright (c) 2026 Five Star Recomp contributors.
// SPDX-License-Identifier: GPL-3.0-or-later

using FiveStar.Loader.Codegen;
using FiveStar.Loader.Diagnostics;

namespace FiveStar.Loader.Shims.Gdk;

/// <summary>
/// Generates an adapter from a COM-style call to a plain C function: drops <c>this</c> (rcx) and shifts every argument
/// one position left (rdx to rcx, r8 to rdx, r9 to r8, stack argument 5 to r9, stack arguments 6..12 to 5..11, xmm1..3
/// to xmm0..2, a float fifth argument to xmm3), following the Microsoft x64 calling convention. A hidden struct-return
/// pointer (which COM passes after <c>this</c>) is shifted like any argument, which matches the C ABI of the target.
/// At most 12 arguments.
/// </summary>
internal static class DropThisThunk
{
    /// <summary>Home space (0x20) plus 7 shifted stack arguments, with rsp 16-byte aligned at the call.</summary>
    private const byte FrameSize = 0x58;

    /// <summary>The caller's fifth argument: entry rsp+0x28, after the prolog rsp+0x80.</summary>
    private const int CallerFifthArgument = 0x80;

    /// <summary>The caller's sixth argument after the prolog.</summary>
    private const int CallerSixthArgument = 0x88;

    /// <summary>Our fifth argument slot: just above the home space.</summary>
    private const byte CalleeFifthArgument = 0x20;

    /// <summary>Stack arguments moved down: callers' 6..12 become 5..11.</summary>
    private const int ShiftedStackArguments = 7;

    public static nint Create(CodeHeap heap, LoaderLog log, nint target)
    {
        var code = new X64Emitter();
        code.SubRsp(FrameSize);
        byte prologSize = (byte)code.Length;

        // Integer and stack arguments.
        code.ShiftIntegerArguments();
        code.LoadR9FromStack(CallerFifthArgument);

        for (int k = 0; k < ShiftedStackArguments; k++)
        {
            code.LoadRaxFromStackFar(CallerSixthArgument + (sizeof(ulong) * k));
            code.StoreRaxToStack((byte)(CalleeFifthArgument + (sizeof(ulong) * k)));
        }

        // Floating-point arguments.
        code.ShiftFloatArguments();
        code.LoadXmm3FromStack(CallerFifthArgument);

        code.MovRax((ulong)target);
        code.CallRax();
        code.AddRsp(FrameSize);
        code.Ret();

        return FramedFunction.Commit(heap, code, prologSize, FrameSize, log, "[gdk] DropThisThunk");
    }
}
