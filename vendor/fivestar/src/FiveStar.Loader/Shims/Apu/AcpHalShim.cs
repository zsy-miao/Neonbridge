// Copyright (c) 2026 Five Star Recomp contributors.
// SPDX-License-Identifier: GPL-3.0-or-later

using System.Runtime.InteropServices;
using FiveStar.Loader.Diagnostics;
using FiveStar.Loader.Native;

namespace FiveStar.Loader.Shims.Apu;

/// <summary>
/// acphal.dll, the console's audio co-processor HAL. Not in the public GDK headers: every signature here is inferred
/// from the executable's call sites (argument registers, how the result is tested) and matches the shape of the
/// older XDK APU API. The APU memory functions run on <see cref="ApuHeap"/>; the SHAPE functions on
/// <see cref="AcpHalDevice"/>. They must succeed: when one fails, the game's audio start-up executes an <c>int3</c>.
/// </summary>
[ShimModule(Module, UnresolvedReturn = HResult.NotImplemented)]
internal static unsafe partial class AcpHalShim
{
    public const string Module = "acphal.dll";

    /// <summary>HRESULT AcpHalCreate(IAcpHal** out): the caller then calls (*out)->vtbl[0](obj, 0x1000, 0x1000).</summary>
    [Shim(Module, "AcpHalCreate")]
    [UnmanagedCallersOnly]
    private static int AcpHalCreate(nint* result)
    {
        try
        {
            return ShimContext.Runtime.AcpHal.Create(result);
        }
        catch (Exception exception)
        {
            UnmanagedGuard.Caught(exception, "AcpHalShim.AcpHalCreate");

            return HResult.Fail;
        }
    }

    /// <summary>HRESULT AcpHalAllocateShapeContexts(SHAPE_CONTEXTS* contexts): the caller zeroes count * 64 bytes at +0x38 on success.</summary>
    [Shim(Module, "AcpHalAllocateShapeContexts")]
    [UnmanagedCallersOnly]
    private static int AcpHalAllocateShapeContexts(byte* contexts)
    {
        try
        {
            return ShimContext.Runtime.AcpHal.AllocateContexts(contexts);
        }
        catch (Exception exception)
        {
            UnmanagedGuard.Caught(exception, "AcpHalShim.AcpHalAllocateShapeContexts");

            return HResult.Fail;
        }
    }

    /// <summary>AcpHalReleaseShapeContexts(): called with no argument at teardown; the result is not read.</summary>
    [Shim(Module, "AcpHalReleaseShapeContexts")]
    [UnmanagedCallersOnly]
    private static int AcpHalReleaseShapeContexts()
    {
        try
        {
            ShimContext.Runtime.AcpHal.ReleaseContexts();

            return HResult.Ok;
        }
        catch (Exception exception)
        {
            UnmanagedGuard.Caught(exception, "AcpHalShim.AcpHalReleaseShapeContexts");

            return HResult.Fail;
        }
    }

    /// <summary>HRESULT ApuCreateHeap(UINT32 cachedSizeInBytes, UINT32 nonCachedSizeInBytes).</summary>
    [Shim(Module, "ApuCreateHeap")]
    [UnmanagedCallersOnly]
    private static int ApuCreateHeap(uint cachedBytes, uint nonCachedBytes)
    {
        try
        {
            return ShimContext.Runtime.Apu.Create(cachedBytes, nonCachedBytes);
        }
        catch (Exception exception)
        {
            UnmanagedGuard.Caught(exception, "AcpHalShim.ApuCreateHeap");

            return HResult.Fail;
        }
    }

    /// <summary>HRESULT ApuAlloc(void** virtualAddress, APU_ADDRESS* apuAddress, UINT32 size, UINT32 alignment, UINT32 flags).</summary>
    [Shim(Module, "ApuAlloc")]
    [UnmanagedCallersOnly]
    private static int ApuAlloc(nint* virtualAddress, uint* apuAddress, uint size, uint alignment, uint flags)
    {
        try
        {
            return ShimContext.Runtime.Apu.Allocate(virtualAddress, apuAddress, size, alignment);
        }
        catch (Exception exception)
        {
            UnmanagedGuard.Caught(exception, "AcpHalShim.ApuAlloc");

            return HResult.Fail;
        }
    }

    /// <summary>HRESULT ApuFree(void* virtualAddress).</summary>
    [Shim(Module, "ApuFree")]
    [UnmanagedCallersOnly]
    private static int ApuFree(nint virtualAddress)
    {
        try
        {
            return ShimContext.Runtime.Apu.Free(virtualAddress);
        }
        catch (Exception exception)
        {
            UnmanagedGuard.Caught(exception, "AcpHalShim.ApuFree");

            return HResult.Fail;
        }
    }

    /// <summary>BOOL ApuIsVirtualAddressValid(const void* virtualAddress, UINT32 requirement); the second argument (0x40, 0x80, 0x800 at the call sites) is not used.</summary>
    [Shim(Module, "ApuIsVirtualAddressValid")]
    [UnmanagedCallersOnly]
    private static int ApuIsVirtualAddressValid(nint virtualAddress, uint requirement)
    {
        try
        {
            int valid = ShimContext.Runtime.Apu.IsValid(virtualAddress) ? 1 : 0;

            return valid;
        }
        catch (Exception exception)
        {
            UnmanagedGuard.Caught(exception, "AcpHalShim.ApuIsVirtualAddressValid");

            return HResult.Fail;
        }
    }

    /// <summary>APU_ADDRESS ApuMapVirtualAddress(const void* virtualAddress).</summary>
    [Shim(Module, "ApuMapVirtualAddress")]
    [UnmanagedCallersOnly]
    private static uint ApuMapVirtualAddress(nint virtualAddress)
    {
        try
        {
            return ShimContext.Runtime.Apu.ToApuAddress(virtualAddress);
        }
        catch (Exception exception)
        {
            UnmanagedGuard.Caught(exception, "AcpHalShim.ApuMapVirtualAddress");

            return default;
        }
    }

    /// <summary>void* ApuMapApuAddress(APU_ADDRESS apuAddress).</summary>
    [Shim(Module, "ApuMapApuAddress")]
    [UnmanagedCallersOnly]
    private static nint ApuMapApuAddress(uint apuAddress)
    {
        try
        {
            return ShimContext.Runtime.Apu.ToVirtualAddress(apuAddress);
        }
        catch (Exception exception)
        {
            UnmanagedGuard.Caught(exception, "AcpHalShim.ApuMapApuAddress");

            return default;
        }
    }
}
