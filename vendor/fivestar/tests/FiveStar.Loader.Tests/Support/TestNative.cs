// Copyright (c) 2026 Five Star Recomp contributors.
// SPDX-License-Identifier: GPL-3.0-or-later

using System.Runtime.InteropServices;
using FiveStar.Loader.Native;

namespace FiveStar.Loader.Tests.Support;

/// <summary>Win32 helpers for the tests: events, address-space reservations and XAsyncBlocks.</summary>
internal static unsafe class TestNative
{
    /// <summary>sizeof(XAsyncBlock) on x64.</summary>
    private const int AsyncBlockSize = 0x38;

    [DllImport("kernel32", EntryPoint = "CreateEventW")]
    public static extern nint CreateEvent(nint attributes, int manualReset, int initialState, nint name);

    [DllImport("kernel32")]
    public static extern int SetEvent(nint handle);

    [DllImport("kernel32")]
    public static extern int CloseHandle(nint handle);

    /// <summary>Reserves 64 KiB of address space; an image whose preferred base is here must relocate.</summary>
    public static nint Reserve()
    {
        return Kernel32.VirtualAlloc(0, 0x10000, Win32Constants.MemReserve, Win32Constants.PageNoAccess);
    }

    /// <summary>An address that is free right now: reserved and released at once.</summary>
    public static ulong FreeAddress()
    {
        nint address = Kernel32.VirtualAlloc(0, 0x100000, Win32Constants.MemReserve, Win32Constants.PageNoAccess);
        Release(address);

        return (ulong)address;
    }

    public static void Release(nint address)
    {
        _ = Kernel32.VirtualFree(address, 0, Win32Constants.MemRelease);
    }

    /// <summary>A zeroed XAsyncBlock with its queue and completion routine.</summary>
    public static nint AsyncBlock(nint queue, nint callback)
    {
        nint block = (nint)NativeMemory.AllocZeroed(AsyncBlockSize);
        *(nint*)block = queue;
        *(nint*)(block + 16) = callback;

        return block;
    }
}
