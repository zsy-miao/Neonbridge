// Copyright (c) 2026 Five Star Recomp contributors.
// SPDX-License-Identifier: GPL-3.0-or-later

using System.Runtime.InteropServices;
using FiveStar.Loader.Diagnostics;
using FiveStar.Loader.Native;

namespace FiveStar.Loader.Shims;

/// <summary>
/// The game's events and semaphores, seen for <see cref="WaitObjects"/> (<c>--watch-hang</c>): creation, signals,
/// closes and single-object waits pass through to kernel32 unchanged. The loader removes these shims unless the hang
/// watch is on (<see cref="Names"/>), so a normal run calls kernel32 directly.
/// </summary>
internal static unsafe partial class WaitObjectShims
{
    /// <summary>The shims, as "module!symbol" for <see cref="ShimTable.Disable"/>.</summary>
    /// <summary>WAIT_FAILED.</summary>
    private const uint WaitFailed = 0xFFFFFFFF;

    public static readonly string[] Names =
    [
        "*!CreateEventW", "*!CreateEventA", "*!CreateSemaphoreExW", "*!SetEvent", "*!ReleaseSemaphore", "*!CloseHandle",
        "*!WaitForSingleObject", "*!WaitForSingleObjectEx",
    ];

    [Shim("*", "CreateEventW", SetsLastError = true)]
    [UnmanagedCallersOnly]
    private static nint CreateEventW(nint security, int manualReset, int initialState, char* name)
    {
        try
        {
            nint handle = Kernel32.CreateEventW(security, manualReset, initialState, name);
            ShimContext.Runtime.LastError.Set(Marshal.GetLastPInvokeError());
            WaitObjects.Created(handle, EventKind(manualReset));

            return handle;
        }
        catch (Exception exception)
        {
            UnmanagedGuard.Caught(exception, "WaitObjectShims.CreateEventW");

            return default;
        }
    }

    [Shim("*", "CreateEventA", SetsLastError = true)]
    [UnmanagedCallersOnly]
    private static nint CreateEventA(nint security, int manualReset, int initialState, byte* name)
    {
        try
        {
            nint handle = Kernel32.CreateEventA(security, manualReset, initialState, name);
            ShimContext.Runtime.LastError.Set(Marshal.GetLastPInvokeError());
            WaitObjects.Created(handle, EventKind(manualReset));

            return handle;
        }
        catch (Exception exception)
        {
            UnmanagedGuard.Caught(exception, "WaitObjectShims.CreateEventA");

            return default;
        }
    }

    [Shim("*", "CreateSemaphoreExW")]
    [UnmanagedCallersOnly]
    private static nint CreateSemaphoreExW(nint security, int initial, int maximum, char* name, uint flags, uint access)
    {
        try
        {
            nint handle = Kernel32.CreateSemaphoreExW(security, initial, maximum, name, flags, access);
            WaitObjects.Created(handle, $"semaphore (initial {initial}, maximum {maximum})");

            return handle;
        }
        catch (Exception exception)
        {
            UnmanagedGuard.Caught(exception, "WaitObjectShims.CreateSemaphoreExW");

            return default;
        }
    }

    [Shim("*", "SetEvent", SetsLastError = true)]
    [UnmanagedCallersOnly]
    private static int SetEvent(nint handle)
    {
        try
        {
            WaitObjects.Signaled(handle);
            int done = Kernel32.SetEvent(handle);
            ShimContext.Runtime.LastError.Set(Marshal.GetLastPInvokeError());

            return done;
        }
        catch (Exception exception)
        {
            UnmanagedGuard.Caught(exception, "WaitObjectShims.SetEvent");

            return default;
        }
    }

    [Shim("*", "ReleaseSemaphore", SetsLastError = true)]
    [UnmanagedCallersOnly]
    private static int ReleaseSemaphore(nint handle, int count, int* previous)
    {
        try
        {
            WaitObjects.Signaled(handle);
            int done = Kernel32.ReleaseSemaphore(handle, count, previous);
            ShimContext.Runtime.LastError.Set(Marshal.GetLastPInvokeError());

            return done;
        }
        catch (Exception exception)
        {
            UnmanagedGuard.Caught(exception, "WaitObjectShims.ReleaseSemaphore");

            return default;
        }
    }

    [Shim("*", "CloseHandle", SetsLastError = true)]
    [UnmanagedCallersOnly]
    private static int CloseHandle(nint handle)
    {
        try
        {
            WaitObjects.Closed(handle);
            int done = Kernel32.CloseHandle(handle);
            ShimContext.Runtime.LastError.Set(Marshal.GetLastPInvokeError());

            return done;
        }
        catch (Exception exception)
        {
            UnmanagedGuard.Caught(exception, "WaitObjectShims.CloseHandle");

            return default;
        }
    }

    [Shim("*", "WaitForSingleObject", SetsLastError = true)]
    [UnmanagedCallersOnly]
    private static uint WaitForSingleObject(nint handle, uint milliseconds)
    {
        try
        {
            return Wait(handle, milliseconds, 0);
        }
        catch (Exception exception)
        {
            UnmanagedGuard.Caught(exception, "WaitObjectShims.WaitForSingleObject");

            return WaitFailed;
        }
    }

    [Shim("*", "WaitForSingleObjectEx", SetsLastError = true)]
    [UnmanagedCallersOnly]
    private static uint WaitForSingleObjectEx(nint handle, uint milliseconds, int alertable)
    {
        try
        {
            return Wait(handle, milliseconds, alertable);
        }
        catch (Exception exception)
        {
            UnmanagedGuard.Caught(exception, "WaitObjectShims.WaitForSingleObjectEx");

            return WaitFailed;
        }
    }

    private static uint Wait(nint handle, uint milliseconds, int alertable)
    {
        WaitObjects.Waiting(handle, milliseconds);
        uint result = Kernel32.WaitForSingleObjectEx(handle, milliseconds, alertable);
        ShimContext.Runtime.LastError.Set(Marshal.GetLastPInvokeError());
        WaitObjects.Done();

        return result;
    }

    private static string EventKind(int manualReset)
    {
        if (manualReset != 0)
        {
            return "manual-reset event";
        }

        return "auto-reset event";
    }
}
