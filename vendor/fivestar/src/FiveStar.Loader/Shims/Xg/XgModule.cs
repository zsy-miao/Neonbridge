// Copyright (c) 2026 Five Star Recomp contributors.
// SPDX-License-Identifier: GPL-3.0-or-later

using System.Runtime.InteropServices;
using FiveStar.Loader.Diagnostics;
using FiveStar.Loader.Native;

namespace FiveStar.Loader.Shims.Xg;

/// <summary>
/// xg_xs.dll, the console texture layout library (XG), from its call sites (docs/xg.md):
/// <c>HRESULT XGCreateTextureComputer(const XG_RESOURCE_DESC*, XGTextureAddressComputer**)</c> and
/// <c>HRESULT XGSuggestSwizzleMode(const XG_RESOURCE_DESC*, UINT32 flags, UINT32* layout)</c>. The description has
/// the D3D12_RESOURCE_DESC layout; the computer reports and fills the console tiling of <see cref="XgLayoutBuilder"/>.
/// </summary>
[ShimModule(Module, UnresolvedReturn = HResult.NotImplemented)]
internal static unsafe partial class XgModule
{
    public const string Module = "xg_xs.dll";

    [Shim(Module, "XGCreateTextureComputer")]
    [UnmanagedCallersOnly]
    private static int XGCreateTextureComputer(XgResourceDesc* desc, nint* computer)
    {
        try
        {
            return ShimContext.Runtime.Xg.Create(desc, computer);
        }
        catch (Exception exception)
        {
            UnmanagedGuard.Caught(exception, "XgModule.XGCreateTextureComputer");

            return HResult.Fail;
        }
    }

    /// <summary>The second argument is 0 at both call sites; its meaning is unknown.</summary>
    [Shim(Module, "XGSuggestSwizzleMode")]
    [UnmanagedCallersOnly]
    private static int XGSuggestSwizzleMode(XgResourceDesc* desc, uint flags, uint* layout)
    {
        try
        {
            return XgComputers.Suggest(desc, layout);
        }
        catch (Exception exception)
        {
            UnmanagedGuard.Caught(exception, "XgModule.XGSuggestSwizzleMode");

            return HResult.Fail;
        }
    }
}
