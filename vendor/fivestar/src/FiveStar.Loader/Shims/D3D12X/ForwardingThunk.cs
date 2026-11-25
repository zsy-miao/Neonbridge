// Copyright (c) 2026 Five Star Recomp contributors.
// SPDX-License-Identifier: GPL-3.0-or-later

using FiveStar.Loader.Codegen;

namespace FiveStar.Loader.Shims.D3D12X;

/// <summary>
/// Tail-call thunks from a wrapper vtable slot to a slot of the desktop object the wrapper holds. No managed code runs
/// on these calls, so they meet the [HotPath] rule by construction.
/// </summary>
internal static class ForwardingThunk
{
    /// <summary>
    /// Unwraps the listed argument registers (<see cref="EmitUnwrap"/>), then <c>mov rcx,[rcx+inner]; mov rax,[rcx];
    /// jmp [rax+desktop*8]</c>, or for a struct-returning slot (object in rdx) <c>mov rdx,[rdx+inner]; mov rax,[rdx];
    /// xchg rcx,rdx; jmp [rax+desktop*8]</c>. Stack arguments stay in place. With <paramref name="counter"/> (a byte
    /// offset in the wrapper, 0 for none) the thunk first counts the call there: <c>inc dword [rcx+counter]</c>.
    /// </summary>
    public static nint Emit(CodeHeap heap, ForwardedSlot slot, nint vtables, int vtableBytes, int counter)
    {
        var emitter = new X64Emitter();

        if (counter != 0 && !slot.StructReturn)
        {
            emitter.IncDwordAtRcx(counter);
        }

        EmitUnwraps(emitter, slot.Unwrap, vtables, vtableBytes);

        if (slot.StructReturn)
        {
            emitter.LoadRdxFromRdx(D3D12XLayout.InnerOffset);
            emitter.LoadRaxFromRdx();
            emitter.SwapRcxRdx();
        }
        else
        {
            emitter.LoadRcxFromRcx(D3D12XLayout.InnerOffset);
            emitter.LoadRaxFromRcx();
        }

        emitter.JmpRaxIndirect(slot.DesktopSlot * IntPtr.Size);

        return heap.Write(emitter.ToArray());
    }

    private static void EmitUnwraps(X64Emitter emitter, ArgumentRegisters registers, nint vtables, int vtableBytes)
    {
        if (registers.HasFlag(ArgumentRegisters.Rdx))
        {
            EmitUnwrap(emitter, X64Emitter.Rdx, vtables, vtableBytes);
        }

        if (registers.HasFlag(ArgumentRegisters.R8))
        {
            EmitUnwrap(emitter, X64Emitter.R8, vtables, vtableBytes);
        }

        if (registers.HasFlag(ArgumentRegisters.R9))
        {
            EmitUnwrap(emitter, X64Emitter.R9, vtables, vtableBytes);
        }
    }

    /// <summary>
    /// <c>test reg,reg; jz done; mov r10,[reg]; mov r11,vtables; sub r10,r11; cmp r10,bytes; jae done;
    /// mov reg,[reg+inner]; done:</c>: a non-null pointer whose vtable lies in the shim's vtable block is a wrapper
    /// and becomes its desktop object; anything else passes through.
    /// </summary>
    private static void EmitUnwrap(X64Emitter emitter, int register, nint vtables, int vtableBytes)
    {
        emitter.TestRegister(register);
        int isNull = emitter.JzForward();
        emitter.LoadR10FromRegister(register);

        emitter.MovR11((ulong)vtables);
        emitter.SubR10R11();
        emitter.CmpR10((uint)vtableBytes);
        int notWrapped = emitter.JaeForward();

        emitter.LoadRegisterFromItself(register, D3D12XLayout.InnerOffset);
        emitter.PatchJumpHere(isNull);
        emitter.PatchJumpHere(notWrapped);
    }
}
