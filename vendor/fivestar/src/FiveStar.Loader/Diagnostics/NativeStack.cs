// Copyright (c) 2026 Five Star Recomp contributors.
// SPDX-License-Identifier: GPL-3.0-or-later

using FiveStar.Loader.Loading;
using FiveStar.Loader.Native;

namespace FiveStar.Loader.Diagnostics;

/// <summary>
/// Explicit x64 stack walk with RtlCaptureContext, RtlLookupFunctionEntry and RtlVirtualUnwind, the machinery the
/// exception dispatcher uses. Walking from a shim back through a mapped image into the managed caller only works when
/// the image's .pdata is registered.
/// </summary>
internal static unsafe class NativeStack
{
    /// <summary>The number of frames a walk follows.</summary>
    public const int DefaultDepth = 16;

    // CONTEXT (winnt.h, AMD64): size, alignment and the Rsp and Rip fields.
    private const int ContextSize = 1232;
    private const int ContextAlignment = 16;
    private const int RspOffset = 0x98;
    private const int RipOffset = 0xF8;

    /// <summary>UNW_FLAG_NHANDLER: unwind without calling handlers.</summary>
    private const uint NoHandler = 0;

    private const int FrameNumberWidth = 2;
    private const string NoModule = "(no module: JIT/generated code)";
    private const string MappedPrefix = "[mapped] ";

    public static List<StackFrame> Walk(ImageTable images, int depth)
    {
        byte* raw = stackalloc byte[ContextSize + ContextAlignment];
        byte* context = Align(raw);
        Ntdll.RtlCaptureContext(context);

        return WalkContext(images, context, depth);
    }

    /// <summary>Walks from a CONTEXT, such as the one of an exception; the CONTEXT is copied, not changed.</summary>
    public static List<StackFrame> WalkFrom(ImageTable images, byte* start, int depth)
    {
        byte* raw = stackalloc byte[ContextSize + ContextAlignment];
        byte* context = Align(raw);
        Buffer.MemoryCopy(start, context, ContextSize, ContextSize);

        return WalkContext(images, context, depth);
    }

    private static byte* Align(byte* raw)
    {
        return (byte*)(((ulong)raw + ContextAlignment - 1) & ~(ulong)(ContextAlignment - 1));
    }

    private static List<StackFrame> WalkContext(ImageTable images, byte* context, int depth)
    {
        var frames = new List<StackFrame>();

        for (int i = 0; i < depth; i++)
        {
            ulong pc = *(ulong*)(context + RipOffset);

            if (pc == 0)
            {
                break;
            }

            ulong imageBase = 0;
            nint function = Ntdll.RtlLookupFunctionEntry(pc, &imageBase, 0);
            frames.Add(new StackFrame(pc, Describe(images, pc), function != 0));
            ulong rsp = *(ulong*)(context + RspOffset);

            // Only the innermost frame may be a leaf; past it, a frame without unwind data ends the walk, as does a
            // stack pointer that cannot be read (a fault report must not fault).
            if ((function == 0 && i > 0) || !IsReadable(rsp))
            {
                break;
            }

            Unwind(context, pc, imageBase, function);

            if (*(ulong*)(context + RspOffset) <= rsp)
            {
                break;
            }
        }

        return frames;
    }

    public static void Log(ImageTable images, LoaderLog log, string title)
    {
        var frames = Walk(images, DefaultDepth);
        log.Info($"  {title}: {frames.Count} frames");

        for (int i = 0; i < frames.Count; i++)
        {
            string marker = frames[i].HasUnwindInfo ? "  " : "! ";
            log.Info($"    #{i.ToString(System.Globalization.CultureInfo.InvariantCulture).PadLeft(FrameNumberWidth)} 0x{frames[i].Pc:x16} {marker}{frames[i].Where}");
        }
    }

    private static string Describe(ImageTable images, ulong pc)
    {
        var image = images.FindByAddress((nint)pc);

        if (image is not null)
        {
            return $"{MappedPrefix}{image.Name}+0x{pc - (ulong)image.Base:x}";
        }

        string where = SystemModules.Describe((nint)pc);

        if (where == SystemModules.Unknown)
        {
            return NoModule;
        }

        return where;
    }

    private static bool IsReadable(ulong address)
    {
        MemoryBasicInformation info;

        if (address == 0 || Kernel32.VirtualQueryEx(Kernel32.GetCurrentProcess(), (nint)address, &info, (nuint)sizeof(MemoryBasicInformation)) == 0)
        {
            return false;
        }

        bool blocked = (info.Protect & (Win32Constants.PageNoAccess | Win32Constants.PageGuard)) != 0;

        return info.State == Win32Constants.MemCommit && !blocked;
    }

    private static void Unwind(byte* context, ulong pc, ulong imageBase, nint function)
    {
        if (function == 0)
        {
            // A leaf function: the return address is at [rsp].
            ulong rsp = *(ulong*)(context + RspOffset);
            *(ulong*)(context + RipOffset) = *(ulong*)rsp;
            *(ulong*)(context + RspOffset) = rsp + sizeof(ulong);

            return;
        }

        nint handlerData;
        ulong establisherFrame;
        Ntdll.RtlVirtualUnwind(NoHandler, imageBase, pc, function, context, &handlerData, &establisherFrame, 0);
    }
}
