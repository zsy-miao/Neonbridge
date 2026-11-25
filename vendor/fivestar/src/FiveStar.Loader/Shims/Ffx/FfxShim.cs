// Copyright (c) 2026 Five Star Recomp contributors.
// SPDX-License-Identifier: GPL-3.0-or-later

using System.Runtime.InteropServices;
using FiveStar.Loader.Diagnostics;

namespace FiveStar.Loader.Shims.Ffx;

/// <summary>
/// amd_fidelityfx_loader_xbox.dll, the console FidelityFX API loader. Its upscaler provider is console-only (FSR 4
/// shaders cooked for the console GPU), so the exports bind to <see cref="FidelityFxBridge"/> instead of the shipped
/// DLL (docs/middleware.md).
/// </summary>
internal static unsafe partial class FfxShim
{
    [Shim(FfxLayout.Module, "ffxCreateContext")]
    [UnmanagedCallersOnly]
    private static uint FfxCreateContext(nint* context, byte* desc, nint allocator)
    {
        try
        {
            return ShimContext.Runtime.FidelityFx.CreateContext(context, desc, allocator);
        }
        catch (Exception exception)
        {
            UnmanagedGuard.Caught(exception, "FfxShim.FfxCreateContext");

            return default;
        }
    }

    [Shim(FfxLayout.Module, "ffxDestroyContext")]
    [UnmanagedCallersOnly]
    private static uint FfxDestroyContext(nint* context, nint allocator)
    {
        try
        {
            return ShimContext.Runtime.FidelityFx.DestroyContext(context, allocator);
        }
        catch (Exception exception)
        {
            UnmanagedGuard.Caught(exception, "FfxShim.FfxDestroyContext");

            return default;
        }
    }

    [Shim(FfxLayout.Module, "ffxConfigure")]
    [UnmanagedCallersOnly]
    private static uint FfxConfigure(nint* context, byte* desc)
    {
        try
        {
            return ShimContext.Runtime.FidelityFx.Configure(context, desc);
        }
        catch (Exception exception)
        {
            UnmanagedGuard.Caught(exception, "FfxShim.FfxConfigure");

            return default;
        }
    }

    [Shim(FfxLayout.Module, "ffxQuery")]
    [UnmanagedCallersOnly]
    private static uint FfxQuery(nint* context, byte* desc)
    {
        try
        {
            return ShimContext.Runtime.FidelityFx.Query(context, desc);
        }
        catch (Exception exception)
        {
            UnmanagedGuard.Caught(exception, "FfxShim.FfxQuery");

            return default;
        }
    }

    [Shim(FfxLayout.Module, "ffxDispatch")]
    [HotPath]
    [UnmanagedCallersOnly]
    private static uint FfxDispatch(nint* context, byte* desc)
    {
        try
        {
            return ShimContext.Runtime.FidelityFx.Dispatch(context, desc);
        }
        catch (Exception exception)
        {
            UnmanagedGuard.Caught(exception, "FfxShim.FfxDispatch");

            return default;
        }
    }
}
