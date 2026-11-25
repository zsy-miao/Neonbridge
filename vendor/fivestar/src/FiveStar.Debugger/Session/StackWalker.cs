// Copyright (c) 2026 Five Star Recomp contributors.
// SPDX-License-Identifier: GPL-3.0-or-later

using FiveStar.Debugger.Native;

namespace FiveStar.Debugger.Session;

/// <summary>
/// Walks a stopped thread's stack with StackWalk64, which unwinds through the .pdata of every module dbghelp knows,
/// the images the loader mapped included once they are registered. JIT-compiled frames have no image, so a walk may
/// end at the first managed frame.
/// </summary>
internal sealed unsafe class StackWalker
{
    public const int MaxFrames = 64;

    private readonly DbgHelpExports _exports;

    public StackWalker(DbgHelpExports exports)
    {
        _exports = exports;
    }

    /// <summary>The program counter of each frame, innermost first.</summary>
    public IReadOnlyList<ulong> Walk(nint process, nint thread, ThreadContext context)
    {
        using var scratch = context.Copy();
        var registers = scratch.Pointer;
        var frame = default(StackFrame64);
        frame.ProgramCounter = Flat(registers->Rip);
        frame.Frame = Flat(registers->Rbp);
        frame.Stack = Flat(registers->Rsp);

        var frames = new List<ulong>();
        ulong lastStack = 0;

        while (frames.Count < MaxFrames && Step(process, thread, &frame, registers))
        {
            ulong pc = frame.ProgramCounter.Offset;

            // A frame that repeats the previous stack pointer and address means the unwinder made no progress.
            if (pc == 0 || (frames.Count > 0 && frame.Stack.Offset == lastStack && pc == frames[^1]))
            {
                break;
            }

            frames.Add(pc);
            lastStack = frame.Stack.Offset;
        }

        return frames;
    }

    private static Address64 Flat(ulong offset)
    {
        return new Address64 { Offset = offset, Mode = Address64.FlatMode };
    }

    private bool Step(nint process, nint thread, StackFrame64* frame, Amd64Context* registers)
    {
        return DbgHelp.StackWalk64(DbgHelp.MachineAmd64, process, thread, frame, registers, 0, _exports.FunctionTableAccess, _exports.GetModuleBase, 0) != 0;
    }
}
