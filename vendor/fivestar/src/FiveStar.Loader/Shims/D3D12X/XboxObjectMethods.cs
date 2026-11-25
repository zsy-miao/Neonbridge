// Copyright (c) 2026 Five Star Recomp contributors.
// SPDX-License-Identifier: GPL-3.0-or-later

using System.Runtime.InteropServices;
using FiveStar.Loader.Diagnostics;
using FiveStar.Loader.Native;

namespace FiveStar.Loader.Shims.D3D12X;

/// <summary>
/// Slots every wrapper shares, and the getters whose D3D12.X form differs from the desktop one. The wrapper's
/// reference count is the desktop object's: Release frees the wrapper when the desktop object goes.
/// </summary>
internal static unsafe class XboxObjectMethods
{
    /// <summary>ID3D12DeviceChild::GetDevice and IUnknown slots in the desktop vtable (d3d12.h, unknwn.h).</summary>
    private const int DesktopGetDeviceSlot = 7;

    /// <summary>ID3D12Resource::GetGPUVirtualAddress (d3d12.h).</summary>
    private const int DesktopGetGpuAddressSlot = 11;

    /// <summary>ID3D12Resource::Map and ::Unmap (d3d12.h); the D3D12.X resource keeps the slots.</summary>
    public const int MapSlot = 8;
    public const int UnmapSlot = 9;
    /// <summary>ID3D12CommandAllocator::Reset, the same slot on both.</summary>
    public const int AllocatorResetSlot = 8;

    private const int DesktopMapSlot = MapSlot;
    private const int DesktopUnmapSlot = UnmapSlot;

    /// <summary>ID3D12DescriptorHeap::GetCPUDescriptorHandleForHeapStart and ::GetGPUDescriptorHandleForHeapStart.</summary>
    private const int DesktopCpuStartSlot = 9;
    private const int DesktopGpuStartSlot = 10;

    /// <summary>A slot <c>--d3d12-skip</c> switched off: does nothing and returns 0 (S_OK), whatever the arguments.</summary>
    [HotPath]
    [UnmanagedCallersOnly]
    public static nint Skipped(XboxObject* self)
    {
        return 0;
    }

    [HotPath]
    [UnmanagedCallersOnly]
    public static uint Release(XboxObject* self)
    {
        try
        {
            nint inner = self->Inner;
            uint left = ((delegate* unmanaged<nint, uint>)ComObject.Slot(inner, ComObject.ReleaseSlot))(inner);

            if (left == 0)
            {
                NativeMemory.Free(self);
            }

            return left;
        }
        catch (Exception exception)
        {
            UnmanagedGuard.Caught(exception, "XboxObjectMethods.Release");

            return default;
        }
    }

    /// <summary>QueryInterface on the desktop object; an answer that is the desktop object itself becomes the wrapper.</summary>
    [UnmanagedCallersOnly]
    public static int QueryInterface(XboxObject* self, Guid* iid, nint* result)
    {
        try
        {
            nint inner = self->Inner;
            int hr = ((delegate* unmanaged<nint, Guid*, nint*, int>)ComObject.Slot(inner, ComObject.QueryInterfaceSlot))(inner, iid, result);

            if (!HResult.Failed(hr) && result != null && *result == inner)
            {
                *result = (nint)self;
            }

            return hr;
        }
        catch (Exception exception)
        {
            UnmanagedGuard.Caught(exception, "XboxObjectMethods.QueryInterface");

            return HResult.Fail;
        }
    }

    /// <summary>GetDevice returns the game's device wrapper, not the desktop device.</summary>
    [UnmanagedCallersOnly]
    public static int GetDevice(XboxObject* self, Guid* iid, nint* device)
    {
        try
        {
            nint inner = self->Inner;
            int hr = ((delegate* unmanaged<nint, Guid*, nint*, int>)ComObject.Slot(inner, DesktopGetDeviceSlot))(inner, iid, device);

            if (!HResult.Failed(hr) && device != null)
            {
                *device = ShimContext.Runtime.D3D12X.Objects.DeviceFor(*device);
            }

            return hr;
        }
        catch (Exception exception)
        {
            UnmanagedGuard.Caught(exception, "XboxObjectMethods.GetDevice");

            return HResult.Fail;
        }
    }

    /// <summary>Resource slot 11: the console address for a resource the game placed (slot 133), else the desktop one.</summary>
    [HotPath]
    [UnmanagedCallersOnly]
    public static ulong GetGpuVirtualAddress(XboxResourceObject* self)
    {
        try
        {
            if (self->GameAddress != 0)
            {
                return self->GameAddress;
            }

            nint inner = self->Header.Inner;

            return ((delegate* unmanaged<nint, ulong>)ComObject.Slot(inner, DesktopGetGpuAddressSlot))(inner);
        }
        catch (Exception exception)
        {
            UnmanagedGuard.Caught(exception, "XboxObjectMethods.GetGpuVirtualAddress");

            return default;
        }
    }

    /// <summary>
    /// Resource slot 8, Map(subresource, read range, data): a resource the game placed (slot 133) maps to its own memory,
    /// as on the console, where a texture's bytes are CPU-visible (0x145121891 maps a texture and writes a mip at
    /// the XG offset); its desktop texture is a committed one the CPU cannot map. Others map on the desktop.
    /// </summary>
    [HotPath]
    [UnmanagedCallersOnly]
    public static int Map(XboxResourceObject* self, uint subresource, void* readRange, void** data)
    {
        try
        {
            byte* game = GameBytes(self);

            if (game != null)
            {
                if (data != null)
                {
                    *data = game;
                }

                if (self->Dimension != States.ResourceStates.BufferDimension)
                {
                    ShimContext.Runtime.D3D12X.Textures.Watch(self->GameAddress);
                }

                return HResult.Ok;
            }

            nint inner = self->Header.Inner;
            int hr = ((delegate* unmanaged<nint, uint, void*, void**, int>)ComObject.Slot(inner, DesktopMapSlot))(inner, subresource, readRange, data);

            if (HResult.Failed(hr))
            {
                WarnMapFailed(hr);
            }

            return hr;
        }
        catch (Exception exception)
        {
            UnmanagedGuard.Caught(exception, "XboxObjectMethods.Map");

            return HResult.Fail;
        }
    }

    /// <summary>A failed desktop Map, warned once (a default-heap resource the game expected to reach from the CPU).</summary>
    private static void WarnMapFailed(int hr)
    {
        ShimContext.Runtime.D3D12X.Warnings.Warn("map", $"[d3d12_xs] ID3D12Resource::Map failed on a desktop resource: {HResult.Format(hr)}");
    }

    /// <summary>Allocator slot 8, Reset: the desktop allocator and the spares lent for it (<see cref="SpareAllocators"/>).</summary>
    [UnmanagedCallersOnly]
    public static int ResetAllocator(XboxObject* self)
    {
        try
        {
            nint inner = self->Inner;
            int hr = ((delegate* unmanaged<nint, int>)ComObject.Slot(inner, AllocatorResetSlot))(inner);
            ShimContext.Runtime.D3D12X.SpareAllocators.ResetSpares(inner);

            return hr;
        }
        catch (Exception exception)
        {
            UnmanagedGuard.Caught(exception, "XboxObjectMethods.ResetAllocator");

            return HResult.Fail;
        }
    }

    /// <summary>Resource slot 9, Unmap(subresource, written range): nothing to do for game memory.</summary>
    [HotPath]
    [UnmanagedCallersOnly]
    public static void Unmap(XboxResourceObject* self, uint subresource, void* writtenRange)
    {
        try
        {
            if (GameBytes(self) != null)
            {
                return;
            }

            nint inner = self->Header.Inner;
            ((delegate* unmanaged<nint, uint, void*, void>)ComObject.Slot(inner, DesktopUnmapSlot))(inner, subresource, writtenRange);
        }
        catch (Exception exception)
        {
            UnmanagedGuard.Caught(exception, "XboxObjectMethods.Unmap");
        }
    }

    /// <summary>
    /// Descriptor-heap slot 9: the game takes the handle in rax with only the heap in rcx (call after creation at
    /// 0x141c74559); the desktop method returns it through a hidden pointer. A heap in title memory answers with that memory.
    /// </summary>
    [HotPath]
    [UnmanagedCallersOnly]
    public static nint GetCpuHandleForHeapStart(XboxDescriptorHeapObject* self)
    {
        try
        {
            if (self->GameCpuStart != 0)
            {
                return self->GameCpuStart;
            }

            return HeapStart(&self->Header, DesktopCpuStartSlot);
        }
        catch (Exception exception)
        {
            UnmanagedGuard.Caught(exception, "XboxObjectMethods.GetCpuHandleForHeapStart");

            return default;
        }
    }

    /// <summary>Descriptor-heap slot 10, the GPU handle, returned like slot 9 (*inferred*); a heap in title memory answers with that memory too.</summary>
    [HotPath]
    [UnmanagedCallersOnly]
    public static nint GetGpuHandleForHeapStart(XboxDescriptorHeapObject* self)
    {
        try
        {
            if (self->GameGpuStart != 0)
            {
                return self->GameGpuStart;
            }

            return HeapStart(&self->Header, DesktopGpuStartSlot);
        }
        catch (Exception exception)
        {
            UnmanagedGuard.Caught(exception, "XboxObjectMethods.GetGpuHandleForHeapStart");

            return default;
        }
    }

    /// <summary>The CPU address of a placed resource's game memory, or null when it has none mapped.</summary>
    [HotPath]
    private static byte* GameBytes(XboxResourceObject* self)
    {
        if (self->GameAddress == 0)
        {
            return null;
        }

        return ShimContext.Runtime.D3D12X.MemorySource.Find(self->GameAddress, 1);
    }

    [HotPath]
    private static nint HeapStart(XboxObject* self, int desktopSlot)
    {
        nint inner = self->Inner;
        nint handle = 0;
        _ = ((delegate* unmanaged<nint, nint*, nint*>)ComObject.Slot(inner, desktopSlot))(inner, &handle);

        return handle;
    }
}
