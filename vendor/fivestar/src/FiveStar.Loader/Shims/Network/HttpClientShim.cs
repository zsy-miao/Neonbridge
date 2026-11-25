// Copyright (c) 2026 Five Star Recomp contributors.
// SPDX-License-Identifier: GPL-3.0-or-later

using System.Runtime.InteropServices;
using FiveStar.Loader.Diagnostics;
using FiveStar.Loader.Native;

namespace FiveStar.Loader.Shims.Network;

/// <summary>
/// libHttpClient sealed: <c>HCInitialize</c> first installs a perform function (<c>HCSetHttpCallPerformFunction</c>, the
/// library's hook for custom HTTP stacks), then runs the library's own HCInitialize. Every <c>HCHttpCallPerformAsync</c>
/// then ends in that function, which logs the request, sets the call's network error to
/// HRESULT_FROM_WIN32(ERROR_WINHTTP_NAME_NOT_RESOLVED) and completes the call. The library's WinHTTP imports are
/// sealed as well (<see cref="WinHttpShim"/>).
/// </summary>
internal static unsafe partial class HttpClientShim
{
    private const string Module = "libhttpclient.gdk.dll";

    [Shim(Module, "HCInitialize")]
    [UnmanagedCallersOnly]
    private static int HCInitialize(nint args)
    {
        try
        {
            nint install = Export("HCSetHttpCallPerformFunction");
            nint initialize = Export("HCInitialize");

            if (install == 0 || initialize == 0)
            {
                ShimContext.Runtime.Log.Warn("libHttpClient is not mapped: HCInitialize refused, no HTTP stack");

                return HResult.Fail;
            }

            var perform = (delegate* unmanaged<nint, nint, nint, nint, void>)&Perform;
            int installed = ((delegate* unmanaged<nint, nint, int>)install)((nint)perform, 0);
            ShimContext.Runtime.Log.Info($"  [network] libHttpClient perform function sealed ({HResult.Format(installed)})");

            if (HResult.Failed(installed))
            {
                return installed;
            }

            return ((delegate* unmanaged<nint, int>)initialize)(args);
        }
        catch (Exception exception)
        {
            UnmanagedGuard.Caught(exception, "HttpClientShim.HCInitialize");

            return HResult.Fail;
        }
    }

    /// <summary>void HCCallPerformFunction(HCCallHandle call, XAsyncBlock* asyncBlock, void* context, HCPerformEnv env).</summary>
    [UnmanagedCallersOnly]
    private static void Perform(nint call, nint asyncBlock, nint context, nint environment)
    {
        try
        {
            FailCall(call, asyncBlock);
        }
        catch (Exception e)
        {
            ShimContext.Runtime.Gdk.Boundary.Report("HCCallPerformFunction", e);
        }
    }

    private static void FailCall(nint call, nint asyncBlock)
    {
        var runtime = ShimContext.Runtime;
        nint getUrl = Export("HCHttpCallRequestGetUrl");
        nint setError = Export("HCHttpCallResponseSetNetworkErrorCode");
        byte* method = null;
        byte* url = null;

        if (getUrl != 0)
        {
            _ = ((delegate* unmanaged<nint, byte**, byte**, int>)getUrl)(call, &method, &url);
        }

        runtime.Network.Block("HCHttpCallPerformAsync", $"{SocketAddressText.Ansi(method)} {SocketAddressText.Ansi(url)}");

        if (setError != 0)
        {
            _ = ((delegate* unmanaged<nint, int, uint, int>)setError)(call, NetworkErrors.NameNotResolvedResult, NetworkErrors.NameNotResolved);
        }

        runtime.Gdk.Async.Complete(asyncBlock, HResult.Ok, 0);
    }

    /// <summary>The address of a libHttpClient export, 0 when the library is not mapped.</summary>
    private static nint Export(string name)
    {
        var image = ShimContext.Runtime.Loader.FindMapped(Module);
        var export = image?.Exports.Find(name, 0);

        if (export is null)
        {
            return 0;
        }

        return export.Address;
    }
}
