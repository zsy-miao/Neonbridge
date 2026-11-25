// Copyright (c) 2026 Five Star Recomp contributors.
// SPDX-License-Identifier: GPL-3.0-or-later

using System.Runtime.InteropServices;
using FiveStar.Loader.Diagnostics;
using FiveStar.Loader.Native;

namespace FiveStar.Loader.Shims.Gdk;

/// <summary>
/// XNetworking.h entry points: the connectivity hint and its change event from <see cref="ConnectivityService"/>; the
/// port-change event registers and never fires. URL security queries and certificate checks fail with HRESULT_FROM_WIN32(ERROR_NETWORK_UNREACHABLE).
/// </summary>
internal static unsafe partial class XNetworkingShim
{
    [Shim(GdkModule.Name, "XNetworkingGetConnectivityHint")]
    [UnmanagedCallersOnly]
    private static int XNetworkingGetConnectivityHint(byte* hint)
    {
        try
        {
            return ShimContext.Runtime.Gdk.Connectivity.GetHint(hint);
        }
        catch (Exception exception)
        {
            UnmanagedGuard.Caught(exception, "XNetworkingShim.XNetworkingGetConnectivityHint");

            return HResult.Fail;
        }
    }

    /// <summary>HRESULT XNetworkingRegisterConnectivityHintChanged(XTaskQueueHandle, void* context, callback, XTaskQueueRegistrationToken*).</summary>
    [Shim(GdkModule.Name, "XNetworkingRegisterConnectivityHintChanged")]
    [UnmanagedCallersOnly]
    private static int XNetworkingRegisterConnectivityHintChanged(nint queue, nint context, nint callback, ulong* token)
    {
        try
        {
            return ShimContext.Runtime.Gdk.Connectivity.Register(queue, context, callback, token);
        }
        catch (Exception exception)
        {
            UnmanagedGuard.Caught(exception, "XNetworkingShim.XNetworkingRegisterConnectivityHintChanged");

            return HResult.Fail;
        }
    }

    [Shim(GdkModule.Name, "XNetworkingUnregisterConnectivityHintChanged")]
    [Shim(GdkModule.Name, "XNetworkingUnregisterPreferredLocalUdpMultiplayerPortChanged")]
    [UnmanagedCallersOnly]
    private static byte XNetworkingUnregister(ulong token, byte wait)
    {
        try
        {
            return ShimContext.Runtime.Gdk.Registrations.Unregister(token);
        }
        catch (Exception exception)
        {
            UnmanagedGuard.Caught(exception, "XNetworkingShim.XNetworkingUnregister");

            return default;
        }
    }

    [Shim(GdkModule.Name, "XNetworkingQueryPreferredLocalUdpMultiplayerPort")]
    [UnmanagedCallersOnly]
    private static int XNetworkingQueryPreferredLocalUdpMultiplayerPort(ushort* port)
    {
        try
        {
            return NetworkingService.QueryPreferredPort(port);
        }
        catch (Exception exception)
        {
            UnmanagedGuard.Caught(exception, "XNetworkingShim.XNetworkingQueryPreferredLocalUdpMultiplayerPort");

            return HResult.Fail;
        }
    }

    [Shim(GdkModule.Name, NetworkingService.PortApi)]
    [UnmanagedCallersOnly]
    private static int XNetworkingQueryPreferredLocalUdpMultiplayerPortAsync(nint async)
    {
        try
        {
            var gdk = ShimContext.Runtime.Gdk;

            try
            {
                return gdk.Async.BeginManaged(async, NetworkingService.PortApi, new PortQueryOperation());
            }
            catch (Exception e)
            {
                return gdk.Boundary.Failed(NetworkingService.PortApi, e);
            }
        }
        catch (Exception exception)
        {
            UnmanagedGuard.Caught(exception, "XNetworkingShim.XNetworkingQueryPreferredLocalUdpMultiplayerPortAsync");

            return HResult.Fail;
        }
    }

    [Shim(GdkModule.Name, "XNetworkingQueryPreferredLocalUdpMultiplayerPortAsyncResult")]
    [UnmanagedCallersOnly]
    private static int XNetworkingQueryPreferredLocalUdpMultiplayerPortAsyncResult(nint async, ushort* port)
    {
        try
        {
            var gdk = ShimContext.Runtime.Gdk;

            if (port == null)
            {
                return HResult.Pointer;
            }

            try
            {
                return gdk.Async.ManagedResult(async, NetworkingService.PortApi, sizeof(ushort), (nint)port);
            }
            catch (Exception e)
            {
                return gdk.Boundary.Failed(nameof(XNetworkingQueryPreferredLocalUdpMultiplayerPortAsyncResult), e);
            }
        }
        catch (Exception exception)
        {
            UnmanagedGuard.Caught(exception, "XNetworkingShim.XNetworkingQueryPreferredLocalUdpMultiplayerPortAsyncResult");

            return HResult.Fail;
        }
    }

    [Shim(GdkModule.Name, "XNetworkingRegisterPreferredLocalUdpMultiplayerPortChanged")]
    [UnmanagedCallersOnly]
    private static int XNetworkingRegisterPreferredLocalUdpMultiplayerPortChanged(nint queue, nint context, nint callback, ulong* token)
    {
        try
        {
            return ShimContext.Runtime.Gdk.Registrations.Register("XNetworkingRegisterPreferredLocalUdpMultiplayerPortChanged", callback, token);
        }
        catch (Exception exception)
        {
            UnmanagedGuard.Caught(exception, "XNetworkingShim.XNetworkingRegisterPreferredLocalUdpMultiplayerPortChanged");

            return HResult.Fail;
        }
    }

    /// <summary>HRESULT XNetworkingQuerySecurityInformationForUrlAsync(const char* url, XAsyncBlock*): the URL's server cannot be reached.</summary>
    [Shim(GdkModule.Name, NetworkingService.SecurityInformationApi)]
    [UnmanagedCallersOnly]
    private static int XNetworkingQuerySecurityInformationForUrlAsync(byte* url, nint async)
    {
        try
        {
            if (url == null)
            {
                return HResult.InvalidArgument;
            }

            return AsyncCalls.Begin(async, NetworkingService.SecurityInformationApi, new OfflineOperation(HResult.NetworkUnreachable));
        }
        catch (Exception exception)
        {
            UnmanagedGuard.Caught(exception, "XNetworkingShim.XNetworkingQuerySecurityInformationForUrlAsync");

            return HResult.Fail;
        }
    }

    /// <summary>HRESULT XNetworkingQuerySecurityInformationForUrlAsyncResultSize(XAsyncBlock*, size_t* securityInformationBufferByteCount).</summary>
    [Shim(GdkModule.Name, "XNetworkingQuerySecurityInformationForUrlAsyncResultSize")]
    [UnmanagedCallersOnly]
    private static int XNetworkingQuerySecurityInformationForUrlAsyncResultSize(nint async, nuint* size)
    {
        try
        {
            return AsyncCalls.EmptyCount(async, size);
        }
        catch (Exception exception)
        {
            UnmanagedGuard.Caught(exception, "XNetworkingShim.XNetworkingQuerySecurityInformationForUrlAsyncResultSize");

            return HResult.Fail;
        }
    }

    /// <summary>
    /// HRESULT XNetworkingQuerySecurityInformationForUrlAsyncResult(XAsyncBlock*, size_t bufferByteCount, size_t*
    /// bufferByteCountUsed, uint8_t* buffer, XNetworkingSecurityInformation** securityInformation).
    /// </summary>
    [Shim(GdkModule.Name, "XNetworkingQuerySecurityInformationForUrlAsyncResult")]
    [UnmanagedCallersOnly]
    private static int XNetworkingQuerySecurityInformationForUrlAsyncResult(nint async, nuint size, nuint* used, nint buffer, nint* information)
    {
        try
        {
            if (used != null)
            {
                *used = 0;
            }

            if (information != null)
            {
                *information = 0;
            }

            return AsyncCalls.Result(async, NetworkingService.SecurityInformationApi, size, buffer);
        }
        catch (Exception exception)
        {
            UnmanagedGuard.Caught(exception, "XNetworkingShim.XNetworkingQuerySecurityInformationForUrlAsyncResult");

            return HResult.Fail;
        }
    }

    /// <summary>
    /// HRESULT XNetworkingVerifyServerCertificate(void* requestHandle, const XNetworkingSecurityInformation*): the
    /// game's HTTP stack hands its own request handle, and no request can be live offline, so the check fails closed.
    /// </summary>
    [Shim(GdkModule.Name, "XNetworkingVerifyServerCertificate")]
    [UnmanagedCallersOnly]
    private static int XNetworkingVerifyServerCertificate(nint request, nint securityInformation)
    {
        if (request == 0 || securityInformation == 0)
        {
            return HResult.InvalidArgument;
        }

        return HResult.NetworkUnreachable;
    }
}
