// Copyright (c) 2026 Five Star Recomp contributors.
// SPDX-License-Identifier: GPL-3.0-or-later

using System.Runtime.InteropServices;
using FiveStar.Loader.Diagnostics;
using FiveStar.Loader.Native;

namespace FiveStar.Loader.Shims.Apu;

/// <summary>
/// xapu.dll entry points; the work is in <see cref="XApuDevice"/>. Signatures are inferred from the executable's call
/// sites: XApuConnect and XApuDequeueResult are tested as HRESULTs, XApuGetResultCount as a count.
/// </summary>
internal static unsafe partial class XApuShim
{
    [Shim(AudioHardwareModules.XApuModule, "XApuConnect")]
    [UnmanagedCallersOnly]
    private static int XApuConnect(nint config, nint* sharedMemory, nint* handle)
    {
        try
        {
            return ShimContext.Runtime.XApu.Connect(sharedMemory, handle);
        }
        catch (Exception exception)
        {
            UnmanagedGuard.Caught(exception, "XApuShim.XApuConnect");

            return HResult.Fail;
        }
    }

    [Shim(AudioHardwareModules.XApuModule, "XApuEnqueueCommand")]
    [UnmanagedCallersOnly]
    private static int XApuEnqueueCommand(nint handle, byte* command, uint flags)
    {
        try
        {
            return ShimContext.Runtime.XApu.Enqueue(handle, command);
        }
        catch (Exception exception)
        {
            UnmanagedGuard.Caught(exception, "XApuShim.XApuEnqueueCommand");

            return HResult.Fail;
        }
    }

    [Shim(AudioHardwareModules.XApuModule, "XApuGetResultCount")]
    [UnmanagedCallersOnly]
    private static uint XApuGetResultCount(nint handle, uint flags)
    {
        try
        {
            return ShimContext.Runtime.XApu.ResultCount(handle);
        }
        catch (Exception exception)
        {
            UnmanagedGuard.Caught(exception, "XApuShim.XApuGetResultCount");

            return default;
        }
    }

    [Shim(AudioHardwareModules.XApuModule, "XApuDequeueResult")]
    [UnmanagedCallersOnly]
    private static int XApuDequeueResult(nint handle, byte* result)
    {
        try
        {
            return ShimContext.Runtime.XApu.Dequeue(handle, result);
        }
        catch (Exception exception)
        {
            UnmanagedGuard.Caught(exception, "XApuShim.XApuDequeueResult");

            return HResult.Fail;
        }
    }

    /// <summary>The result is not read.</summary>
    [Shim(AudioHardwareModules.XApuModule, "XApuDisconnect")]
    [UnmanagedCallersOnly]
    private static int XApuDisconnect(nint handle)
    {
        try
        {
            ShimContext.Runtime.XApu.Disconnect();

            return 0;
        }
        catch (Exception exception)
        {
            UnmanagedGuard.Caught(exception, "XApuShim.XApuDisconnect");

            return HResult.Fail;
        }
    }
}
