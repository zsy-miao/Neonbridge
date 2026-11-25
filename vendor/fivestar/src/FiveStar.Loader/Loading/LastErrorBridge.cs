// Copyright (c) 2026 Five Star Recomp contributors.
// SPDX-License-Identifier: GPL-3.0-or-later

using System.Runtime.InteropServices;
using FiveStar.Loader.Codegen;
using FiveStar.Loader.Diagnostics;
using FiveStar.Loader.Native;

namespace FiveStar.Loader.Loading;

/// <summary>
/// SetLastError for shims. A value set inside an [UnmanagedCallersOnly] method does not survive the return to native
/// code: the runtime's reverse-P/Invoke epilogue calls Win32 functions (TlsGetValue) that reset it. Shims marked
/// <c>SetsLastError</c> are therefore entered through a generated wrapper that calls the managed method and then,
/// when the shim asked for it, writes TEB.LastErrorValue itself. The requested value travels in a TLS slot that the
/// wrapper reads straight from TEB.TlsSlots, so it calls nothing. Register arguments pass through untouched; stack
/// arguments (the fifth and later) are copied into the wrapper's own outgoing area.
/// </summary>
internal sealed class LastErrorBridge : IDisposable
{
    /// <summary>Most stack arguments a wrapped shim may take; keeps the frame within one UWOP_ALLOC_SMALL.</summary>
    public const int MaxStackArguments = 8;

    /// <summary>Microsoft x64 ABI: 0x20 bytes of home space for the four register arguments, then the stack arguments.</summary>
    private const int HomeSpace = 0x20;
    private const int SlotSize = 8;
    private const int StackAlignment = 16;

    /// <summary>The return address the call pushed, between the wrapper's frame and its caller's home space.</summary>
    private const int ReturnAddressSize = 8;

    private readonly CodeHeap _heap;
    private readonly LoaderLog _log;
    private readonly uint _slot;

    public LastErrorBridge(CodeHeap heap, LoaderLog log)
    {
        _heap = heap;
        _log = log;
        _slot = Kernel32.TlsAlloc();
    }

    /// <summary>TEB.TlsSlots holds 64 entries; a higher slot lives in the expansion array, which the wrapper cannot read.</summary>
    public bool Available => _slot < ThreadEnvironmentBlock.TlsSlotCount;

    /// <summary>Requests <paramref name="error"/> as the last error the native caller of the current shim sees.</summary>
    public void Set(int error)
    {
        if (!Available)
        {
            Marshal.SetLastSystemError(error);

            return;
        }

        // Stored plus one, so 0 means "no request".
        _ = Kernel32.TlsSetValue(_slot, (nint)(uint)error + 1);
    }

    /// <summary>Returns the entry point that calls <paramref name="target"/> and then applies a requested last error.</summary>
    /// <param name="target">The shim's native entry point.</param>
    /// <param name="stackArguments">How many of its arguments travel on the stack (all after the fourth).</param>
    public nint Wrap(nint target, int stackArguments)
    {
        if (stackArguments is < 0 or > MaxStackArguments)
        {
            throw new ArgumentOutOfRangeException(nameof(stackArguments), stackArguments, $"at most {MaxStackArguments} stack arguments");
        }

        if (!Available)
        {
            return target;
        }

        // Home space, the copied stack arguments and the saved rax; rsp is 16-byte aligned at the call.
        byte savedRaxOffset = (byte)(HomeSpace + (stackArguments * SlotSize));
        int frame = savedRaxOffset + SlotSize;

        if ((frame + ReturnAddressSize) % StackAlignment != 0)
        {
            frame += SlotSize;
        }

        byte frameSize = (byte)frame;
        var code = new X64Emitter();
        code.SubRsp(frameSize);
        byte prologSize = (byte)code.Length;

        CopyStackArguments(code, frameSize, stackArguments);
        EmitCallAndApply(code, target, savedRaxOffset);
        code.AddRsp(frameSize);
        code.Ret();

        return FramedFunction.Commit(_heap, code, prologSize, frameSize, _log, nameof(LastErrorBridge));
    }

    public void Dispose()
    {
        _ = Kernel32.TlsFree(_slot);
    }

    private static void CopyStackArguments(X64Emitter code, byte frameSize, int stackArguments)
    {
        // The caller's stack arguments sit above its home space, behind the return address and this frame.
        int source = frameSize + ReturnAddressSize + HomeSpace;

        for (int i = 0; i < stackArguments; i++)
        {
            code.LoadRaxFromStackFar(source + (i * SlotSize));
            code.StoreRaxToStack((byte)(HomeSpace + (i * SlotSize)));
        }
    }

    private void EmitCallAndApply(X64Emitter code, nint target, byte savedRaxOffset)
    {
        int slotOffset = ThreadEnvironmentBlock.TlsSlots + (sizeof(ulong) * (int)_slot);

        // Call the shim; rcx, rdx, r8 and r9 are untouched.
        code.MovRax((ulong)target);
        code.CallRax();
        code.StoreRaxToStack(savedRaxOffset);

        // Skip ahead unless the shim requested an error.
        code.LoadRcxFromGs(slotOffset);
        code.TestRcxRcx();
        int skip = code.JzForward();

        // TEB.LastErrorValue = request - 1, then consume the request.
        code.DecRcx();
        code.StoreEcxToGs(ThreadEnvironmentBlock.LastErrorValue);
        code.StoreQwordToGs(slotOffset, 0);
        code.PatchJumpHere(skip);

        code.LoadRaxFromStack(savedRaxOffset);
    }
}
