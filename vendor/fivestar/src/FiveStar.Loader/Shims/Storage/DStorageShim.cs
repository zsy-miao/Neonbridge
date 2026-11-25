// Copyright (c) 2026 Five Star Recomp contributors.
// SPDX-License-Identifier: GPL-3.0-or-later

using System.Runtime.InteropServices;
using FiveStar.Loader.Diagnostics;
using FiveStar.Loader.Native;

namespace FiveStar.Loader.Shims.Storage;

/// <summary>
/// XCRDAPI.dll, the console DirectStorage runtime: <c>HRESULT DStorageGetFactory(REFIID riid, void** ppv)</c>
/// (dstorage.h). The PC DirectStorage runtime is not a system DLL and its interfaces differ from the console ones,
/// so the shim hands out <see cref="DStorageFactory"/>, a CPU implementation: the game stops at an <c>int3</c> when
/// this call fails or yields null.
/// </summary>
[ShimModule(Module, UnresolvedReturn = HResult.NotImplemented)]
internal static unsafe partial class DStorageShim
{
    public const string Module = "xcrdapi.dll";

    [Shim(Module, "DStorageGetFactory")]
    [UnmanagedCallersOnly]
    private static int DStorageGetFactory(Guid* iid, nint* factory)
    {
        try
        {
            return ShimContext.Runtime.Storage.GetFactory(iid, factory);
        }
        catch (Exception exception)
        {
            UnmanagedGuard.Caught(exception, "DStorageShim.DStorageGetFactory");

            return HResult.Fail;
        }
    }
}
