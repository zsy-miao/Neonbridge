// Copyright (c) 2026 Five Star Recomp contributors.
// SPDX-License-Identifier: GPL-3.0-or-later

using System.Runtime.InteropServices;
using FiveStar.Loader.Diagnostics;
using FiveStar.Loader.Native;

namespace FiveStar.Loader.Shims.D3D12X.Pipelines;

/// <summary>
/// The native entry points of the five pipeline slots. <see cref="Register"/> binds them in the device vtable through
/// <see cref="XboxObjects.SetHandler"/> once the device's <see cref="PipelineSlots"/> exist
/// (<see cref="XboxDeviceFactory.Pipelines"/>); without them the slots stay logged E_NOTIMPL stubs.
/// </summary>
internal static unsafe class PipelineSlotHandlers
{
    /// <summary>Binds the five pipeline slots of the device vtable to these handlers.</summary>
    public static void Register(XboxObjects objects)
    {
        delegate* unmanaged<nint, nint, uint, byte*, Guid*, nint*, int> derive = &Derive;
        delegate* unmanaged<nint, byte*, Guid*, nint*, int> cachedGraphics = &CachedGraphics;
        delegate* unmanaged<nint, byte*, nint, nint, Guid*, nint*, int> compute = &Compute;
        delegate* unmanaged<nint, byte*, Guid*, nint*, int> cachedCompute = &CachedCompute;
        delegate* unmanaged<nint, byte*, Guid*, nint*, int> stream = &Stream;

        objects.SetHandler(XboxInterfaceKind.Device, PipelineLayout.DeriveSlot, (nint)derive);
        objects.SetHandler(XboxInterfaceKind.Device, PipelineLayout.PrecompiledGraphicsSlot, (nint)cachedGraphics);
        objects.SetHandler(XboxInterfaceKind.Device, PipelineLayout.ComputeSlot, (nint)compute);
        objects.SetHandler(XboxInterfaceKind.Device, PipelineLayout.PrecompiledComputeSlot, (nint)cachedCompute);
        objects.SetHandler(XboxInterfaceKind.Device, PipelineLayout.StreamSlot, (nint)stream);
    }

    [UnmanagedCallersOnly]
    private static int Derive(nint self, nint basePipeline, uint count, byte* entries, Guid* iid, nint* result)
    {
        try
        {
            var slots = Owner(self);

            if (slots is null)
            {
                return HResult.NotImplemented;
            }

            return slots.Derive(basePipeline, count, entries, iid, result);
        }
        catch (Exception exception)
        {
            UnmanagedGuard.Caught(exception, "PipelineSlotHandlers.Derive");

            return HResult.Fail;
        }
    }

    [UnmanagedCallersOnly]
    private static int CachedGraphics(nint self, byte* desc, Guid* iid, nint* result)
    {
        try
        {
            var slots = Owner(self);

            if (slots is null)
            {
                return HResult.NotImplemented;
            }

            return slots.CachedGraphics(desc, iid, result);
        }
        catch (Exception exception)
        {
            UnmanagedGuard.Caught(exception, "PipelineSlotHandlers.CachedGraphics");

            return HResult.Fail;
        }
    }

    [UnmanagedCallersOnly]
    private static int Compute(nint self, byte* desc, nint second, nint third, Guid* iid, nint* result)
    {
        try
        {
            var slots = Owner(self);

            if (slots is null)
            {
                return HResult.NotImplemented;
            }

            return slots.Compute(desc, second, third, iid, result);
        }
        catch (Exception exception)
        {
            UnmanagedGuard.Caught(exception, "PipelineSlotHandlers.Compute");

            return HResult.Fail;
        }
    }

    [UnmanagedCallersOnly]
    private static int CachedCompute(nint self, byte* desc, Guid* iid, nint* result)
    {
        try
        {
            var slots = Owner(self);

            if (slots is null)
            {
                return HResult.NotImplemented;
            }

            return slots.CachedCompute(desc, iid, result);
        }
        catch (Exception exception)
        {
            UnmanagedGuard.Caught(exception, "PipelineSlotHandlers.CachedCompute");

            return HResult.Fail;
        }
    }

    [UnmanagedCallersOnly]
    private static int Stream(nint self, byte* desc, Guid* iid, nint* result)
    {
        try
        {
            var slots = Owner(self);

            if (slots is null)
            {
                return HResult.NotImplemented;
            }

            return slots.Stream(desc, iid, result);
        }
        catch (Exception exception)
        {
            UnmanagedGuard.Caught(exception, "PipelineSlotHandlers.Stream");

            return HResult.Fail;
        }
    }

    private static PipelineSlots? Owner(nint self)
    {
        return ShimContext.Runtime.D3D12X.Pipelines;
    }
}
