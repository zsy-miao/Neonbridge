// Copyright (c) 2026 Five Star Recomp contributors.
// SPDX-License-Identifier: GPL-3.0-or-later

using System.Runtime.InteropServices;
using FiveStar.Loader.Diagnostics;
using FiveStar.Loader.Native;

namespace FiveStar.Loader.Shims.Apu;

/// <summary>
/// chad.dll entry points; the work is in <see cref="XDspDevice"/>. Signatures are inferred from the executable's call
/// sites: connect, activate and submit succeed only on 0; the deactivate and disconnect results are not read.
/// </summary>
internal static unsafe partial class XDspShim
{
    [Shim(AudioHardwareModules.XDspModule, "XDspConnectWithMaximumStreamLimit")]
    [UnmanagedCallersOnly]
    private static int XDspConnectWithMaximumStreamLimit(nint memory, uint bytes, uint maxStreams, uint commandLimit, uint flags, nint* handle)
    {
        try
        {
            return ShimContext.Runtime.XDsp.Connect(memory, bytes, maxStreams, handle);
        }
        catch (Exception exception)
        {
            UnmanagedGuard.Caught(exception, "XDspShim.XDspConnectWithMaximumStreamLimit");

            return HResult.Fail;
        }
    }

    [Shim(AudioHardwareModules.XDspModule, "XDspActivate")]
    [UnmanagedCallersOnly]
    private static int XDspActivate(nint handle, byte* config, nint* status, nint* stream)
    {
        try
        {
            return ShimContext.Runtime.XDsp.Activate(handle, config, status, stream);
        }
        catch (Exception exception)
        {
            UnmanagedGuard.Caught(exception, "XDspShim.XDspActivate");

            return HResult.Fail;
        }
    }

    [Shim(AudioHardwareModules.XDspModule, "XDspSubmitCommand")]
    [UnmanagedCallersOnly]
    private static int XDspSubmitCommand(nint stream, byte* command, uint* id)
    {
        try
        {
            return ShimContext.Runtime.XDsp.Submit(stream, command, id);
        }
        catch (Exception exception)
        {
            UnmanagedGuard.Caught(exception, "XDspShim.XDspSubmitCommand");

            return HResult.Fail;
        }
    }

    [Shim(AudioHardwareModules.XDspModule, "XDspDeactivate")]
    [UnmanagedCallersOnly]
    private static int XDspDeactivate(nint stream)
    {
        try
        {
            ShimContext.Runtime.XDsp.Deactivate(stream);

            return HResult.Ok;
        }
        catch (Exception exception)
        {
            UnmanagedGuard.Caught(exception, "XDspShim.XDspDeactivate");

            return HResult.Fail;
        }
    }

    [Shim(AudioHardwareModules.XDspModule, "XDspDisconnect")]
    [UnmanagedCallersOnly]
    private static int XDspDisconnect(nint handle)
    {
        try
        {
            ShimContext.Runtime.XDsp.Disconnect();

            return HResult.Ok;
        }
        catch (Exception exception)
        {
            UnmanagedGuard.Caught(exception, "XDspShim.XDspDisconnect");

            return HResult.Fail;
        }
    }
}
