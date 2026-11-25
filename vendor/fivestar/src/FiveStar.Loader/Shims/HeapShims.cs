// Copyright (c) 2026 Five Star Recomp contributors.
// SPDX-License-Identifier: GPL-3.0-or-later

using System.Runtime.InteropServices;
using FiveStar.Loader.Diagnostics;
using FiveStar.Loader.Native;

namespace FiveStar.Loader.Shims;

/// <summary>
/// HeapCreate and HeapDestroy from mapped code pass through to kernel32 unchanged; the memory map names the game frame
/// that made each heap (<see cref="HeapOwners"/>).
/// </summary>
internal static unsafe partial class HeapShims
{
    [Shim("*", "HeapCreate", SetsLastError = true)]
    [UnmanagedCallersOnly]
    private static nint HeapCreate(uint options, nuint initialSize, nuint maximumSize)
    {
        try
        {
            var runtime = ShimContext.Runtime;
            nint heap = Kernel32.HeapCreate(options, initialSize, maximumSize);
            runtime.LastError.Set(Marshal.GetLastPInvokeError());
            HeapOwners.Created(heap, runtime.Images);

            return heap;
        }
        catch (Exception exception)
        {
            UnmanagedGuard.Caught(exception, "HeapShims.HeapCreate");

            return default;
        }
    }

    [Shim("*", "HeapDestroy", SetsLastError = true)]
    [UnmanagedCallersOnly]
    private static int HeapDestroy(nint heap)
    {
        try
        {
            HeapOwners.Destroyed(heap);
            int done = Kernel32.HeapDestroy(heap);
            ShimContext.Runtime.LastError.Set(Marshal.GetLastPInvokeError());

            return done;
        }
        catch (Exception exception)
        {
            UnmanagedGuard.Caught(exception, "HeapShims.HeapDestroy");

            return default;
        }
    }
}
