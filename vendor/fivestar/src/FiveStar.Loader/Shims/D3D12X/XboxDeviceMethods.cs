// Copyright (c) 2026 Five Star Recomp contributors.
// SPDX-License-Identifier: GPL-3.0-or-later

using System.Runtime.InteropServices;
using FiveStar.Loader.Diagnostics;
using FiveStar.Loader.Native;

namespace FiveStar.Loader.Shims.D3D12X;

/// <summary>D3D12.X-only device slots that translate to a desktop call (creation calls, not on the per-frame path).</summary>
internal static unsafe class XboxDeviceMethods
{
    /// <summary>D3D12_COMMAND_QUEUE_FLAG_DISABLE_GPU_TIMEOUT, the only desktop queue flag (d3d12.h).</summary>
    private const int DesktopQueueFlags = 1;

    /// <summary>ID3D12Device::CreateCommandQueue in the desktop vtable (d3d12.h).</summary>
    private const int DesktopCreateCommandQueueSlot = 8;

    /// <summary>
    /// Slot 48: (desc, REFIID, void**) like the desktop CreateCommandQueue. The game asks for compute queues with
    /// flags 1 and 2 and node mask 0; flag bits the desktop does not define are dropped and node 0 means the one GPU.
    /// </summary>
    [UnmanagedCallersOnly]
    public static int CreateCommandQueue(XboxObject* self, CommandQueueDescription* description, Guid* iid, nint* queue)
    {
        try
        {
            if (self == null || description == null)
            {
                return HResult.InvalidArgument;
            }

            var desktop = *description;
            desktop.Flags &= DesktopQueueFlags;
            nint inner = self->Inner;
            var create = (delegate* unmanaged<nint, CommandQueueDescription*, Guid*, nint*, int>)(*(nint**)inner)[DesktopCreateCommandQueueSlot];
            int hr = create(inner, &desktop, iid, queue);

            string desc = $"type {description->Type}, priority {description->Priority}, flags 0x{description->Flags:x}, node 0x{description->NodeMask:x}";
            ShimContext.Runtime.Log.Detail($"  [d3d12_xs] ID3D12Device.X::Slot48 CreateCommandQueue({desc}) -> {HResult.Format(hr)}");

            return ShimContext.Runtime.D3D12X.Objects.WrapResult(hr, XboxInterfaceKind.CommandQueue, queue);
        }
        catch (Exception exception)
        {
            UnmanagedGuard.Caught(exception, "XboxDeviceMethods.CreateCommandQueue");

            return HResult.Fail;
        }
    }

    /// <summary>Slot 73: (desc, allocator, PSO, REFIID, void**). The list is wrapped whatever IID the game asks for.</summary>
    [UnmanagedCallersOnly]
    public static int CreateCommandList(XboxObject* self, CommandListDescription* description, nint allocator, nint pipelineState, Guid* iid, nint* list)
    {
        try
        {
            if (self == null || description == null)
            {
                return HResult.InvalidArgument;
            }

            var lists = ShimContext.Runtime.D3D12X.CommandLists;
            int hr = lists.Create(self->Inner, description->Type, allocator, pipelineState, list);
            ShimContext.Runtime.Log.Detail($"  [d3d12_xs] ID3D12Device.X::Slot73 CreateCommandList(type {description->Type}, dword1 {description->Unknown1}, dword2 {description->Unknown2}, dword3 {description->Unknown3}) -> {HResult.Format(hr)}");

            return hr;
        }
        catch (Exception exception)
        {
            UnmanagedGuard.Caught(exception, "XboxDeviceMethods.CreateCommandList");

            return HResult.Fail;
        }
    }

    /// <summary>
    /// Slot 133, read as CreatePlacedResourceX (*inferred*): (GPU address, desc, state, clear value, REFIID, void**),
    /// a resource over memory the game mapped itself (<see cref="XboxPlacedResources"/>).
    /// </summary>
    [UnmanagedCallersOnly]
    public static int CreatePlacedResource(XboxObject* self, ulong address, ResourceDescription* description, uint state, void* clear, Guid* iid, nint* resource)
    {
        try
        {
            if (self == null || description == null || resource == null)
            {
                return HResult.InvalidArgument;
            }

            return ShimContext.Runtime.D3D12X.PlacedResources.Create(self->Inner, address, description, (int)state, clear, resource);
        }
        catch (Exception exception)
        {
            UnmanagedGuard.Caught(exception, "XboxDeviceMethods.CreatePlacedResource");

            return HResult.Fail;
        }
    }

    /// <summary>
    /// Slot 127, a committed resource with the argument list of desktop CreateCommittedResource3 (*inferred*: heap
    /// properties, heap flags, desc, initial barrier layout, clear value, protected session, castable formats, IID,
    /// out; call at 0x14193e1b4, next to the slot-133 path; a 4K render target comes with layout 3, UNORDERED_ACCESS).
    /// See <see cref="XboxCommittedResources"/>.
    /// </summary>
    [UnmanagedCallersOnly]
    public static int CreateCommittedResource(XboxObject* self, void* properties, int heapFlags, ResourceDescription* description, int initialLayout, void* clear, nint protectedSession, uint castableCount, void* castableFormats, Guid* iid, nint* resource)
    {
        try
        {
            if (self == null || description == null || resource == null)
            {
                return HResult.InvalidArgument;
            }

            return ShimContext.Runtime.D3D12X.CommittedResources.Create(self->Inner, properties, heapFlags, description, initialLayout, clear, iid, resource);
        }
        catch (Exception exception)
        {
            UnmanagedGuard.Caught(exception, "XboxDeviceMethods.CreateCommittedResource");

            return HResult.Fail;
        }
    }

    /// <summary>
    /// Slot 74 (*inferred*: registers a mapped range with the GPU): (address, page count, out). The game calls it once,
    /// at 0x141e6c2bb, for its 0x185c0-page streaming range right after mapping it, and breaks into the debugger on a
    /// failure. Nothing on the desktop needs it yet: logged, the 8-byte output zeroed, S_OK.
    /// </summary>
    [UnmanagedCallersOnly]
    public static int RegisterGpuRange(XboxObject* self, nint address, uint pages, ulong* output)
    {
        try
        {
            if (output != null)
            {
                *output = 0;
            }

            ShimContext.Runtime.Log.Detail($"  [d3d12_xs] ID3D12Device.X::Slot74(0x{address:x}, {pages} pages) -> S_OK (no-op)");

            return HResult.Ok;
        }
        catch (Exception exception)
        {
            UnmanagedGuard.Caught(exception, "XboxDeviceMethods.RegisterGpuRange");

            return HResult.Fail;
        }
    }
}
