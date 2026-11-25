// Copyright (c) 2026 Five Star Recomp contributors.
// SPDX-License-Identifier: GPL-3.0-or-later

using FiveStar.Loader.Diagnostics;
using FiveStar.Loader.Native;

namespace FiveStar.Loader.Codegen;

/// <summary>
/// Finishes a generated function whose prolog is a single <c>sub rsp,size</c>: appends its UNWIND_INFO and
/// RUNTIME_FUNCTION, writes it to the code heap and registers it with RtlAddFunctionTable, so stack walks and
/// exceptions unwind through it. The caller seals the heap before the function can run.
/// </summary>
internal static class FramedFunction
{
    public static nint Commit(CodeHeap heap, X64Emitter code, byte prologSize, byte frameSize, LoaderLog log, string owner)
    {
        int codeLength = code.Length;
        code.PadTo(UnwindFormat.Alignment);
        int unwindOffset = code.Length;
        byte allocation = (byte)((((frameSize - UnwindFormat.AllocSmallUnit) / UnwindFormat.AllocSmallUnit) << UnwindFormat.OpInfoShift) | UnwindFormat.OpAllocSmall);

        // UNWIND_INFO: version, prolog size, one code, no frame register; the code slot is padded to 4 bytes.
        code.Data([UnwindFormat.Version1, prologSize, 1, 0, prologSize, allocation, 0, 0]);

        // RUNTIME_FUNCTION relative to the function's own start.
        int runtimeFunctionOffset = code.Length;
        code.Data32(0);
        code.Data32((uint)codeLength);
        code.Data32((uint)unwindOffset);

        nint address = heap.Write(code.ToArray());

        if (Ntdll.RtlAddFunctionTable(address + runtimeFunctionOffset, 1, (ulong)address) == 0)
        {
            log.Warn($"{owner}: RtlAddFunctionTable failed");
        }

        return address;
    }
}
