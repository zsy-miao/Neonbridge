// Copyright (c) 2026 Five Star Recomp contributors.
// SPDX-License-Identifier: GPL-3.0-or-later

using System.Runtime.InteropServices;
using FiveStar.Loader.Diagnostics;
using FiveStar.Loader.Native;
using FiveStar.Loader.Shims.D3D12X.Resources;
using FiveStar.Loader.Shims.D3D12X.States;

namespace FiveStar.Loader.Shims.D3D12X;

/// <summary>
/// Device slot 127, the committed resources. On the console D3D12.X takes their memory from the title's XMemAlloc
/// hook, and the game counts on it: around its render targets (0x14247ad10) it pushes a hook that hands out a 64 MiB
/// region for D3D allocations with page-size field 2, then breaks into the debugger unless all but 2 MiB of it went
/// to the resources. The shim makes the same allocation, of the console's size for a texture (a desktop driver adds
/// metadata of its own: 0x2010000 bytes for a 3840x2160 RGBA8 target on an RDNA3 card, and two of those overrun the
/// region) and of the desktop size for a buffer, and frees it with the resource; the desktop resource itself lives in
/// desktop memory.
/// </summary>
internal sealed unsafe class XboxCommittedResources
{
    /// <summary>ID3D12Device::CreateCommittedResource and ::GetResourceAllocationInfo (d3d12.h).</summary>
    private const int DesktopCreateCommittedResourceSlot = 27;
    private const int DesktopAllocationInfoSlot = 25;

    /// <summary>D3D12_HEAP_FLAG_NONE: the game passes 8, on the desktop ALLOW_DISPLAY.</summary>
    private const int NoHeapFlags = 0;

    /// <summary>
    /// XALLOC attributes: allocator id 128 (bits 16..23, D3D's, as for descriptor memory), page-size field 2
    /// (bits 14..15, what the game's render-target hook at 0x1456a2a60 takes) and 64 KiB alignment (code 17, bits 24..28).
    /// </summary>
    private const ulong ResourceAttributes = 0x11808000;

    private readonly XboxObjects _objects;
    private readonly DesktopResourceDescriptions _descriptions;
    private readonly TitleMemoryHooks _hooks;
    private readonly LoaderLog _log;

    public XboxCommittedResources(XboxObjects objects, DesktopResourceDescriptions descriptions, TitleMemoryHooks hooks, LoaderLog log)
    {
        _objects = objects;
        _descriptions = descriptions;
        _hooks = hooks;
        _log = log;
    }

    /// <summary>
    /// A desktop committed resource in its home state (<see cref="ResourceStates.Home"/> of <paramref name="initialLayout"/>,
    /// a D3D12_BARRIER_LAYOUT; buffers start COMMON), with the desc made a desktop one, plus its title-memory allocation; the wrapper is returned.
    /// </summary>
    public int Create(nint device, void* properties, int heapFlags, ResourceDescription* description, int initialLayout, void* clear, Guid* iid, nint* resource)
    {
        var desktop = _descriptions.ToDesktop(device, description);
        int state = desktop.Dimension == ResourceDescription.BufferDimension ? BarrierLayouts.CommonState : BarrierLayouts.ToState(initialLayout);
        state = ResourceStates.Home(desktop.Dimension, desktop.Flags, state);
        byte* clearCopy = stackalloc byte[DepthViewDescriptions.ClearValueSize];
        var create = (delegate* unmanaged<nint, void*, int, ResourceDescription*, int, void*, Guid*, nint*, int>)ComObject.Slot(device, DesktopCreateCommittedResourceSlot);
        int hr = create(device, properties, NoHeapFlags, &desktop, state, DepthViewDescriptions.ClearValue(clear, clearCopy, desktop.Format), iid, resource);

        if (_log.DetailEnabled)
        {
            string desc = $"dimension {description->Dimension}, {description->Width}x{description->Height}x{description->DepthOrArraySize}, format {description->Format}, mips {description->MipLevels}, flags 0x{description->Flags:x}, layout 0x{description->Layout:x}";
            _log.Detail($"  [d3d12_xs] ID3D12Device.X::Slot127 CreateCommittedResource(heap flags 0x{heapFlags:x}, {desc}, initial layout {initialLayout}) -> {HResult.Format(hr)}");
        }

        hr = _objects.WrapResult(hr, XboxInterfaceKind.Resource, resource);

        if (!HResult.Failed(hr) && resource != null && *resource != 0)
        {
            var wrapper = (XboxResourceObject*)*resource;
            wrapper->TitleMemory = _hooks.Allocate((nuint)TitleAllocationSize(device, description, &desktop), ResourceAttributes);
            ResourceStates.Record(wrapper, desktop.Dimension, state, desktop.Flags);
        }

        return hr;
    }

    /// <summary>Frees a resource wrapper's title memory, when it has some.</summary>
    public void Release(XboxResourceObject* resource)
    {
        if (resource->TitleMemory != 0)
        {
            _hooks.Free(resource->TitleMemory, ResourceAttributes);
            resource->TitleMemory = 0;
        }
    }

    /// <summary>Resource slot 2: Release that also frees the title memory with the last reference.</summary>
    [UnmanagedCallersOnly]
    public static uint ReleaseResource(XboxResourceObject* self)
    {
        try
        {
            nint inner = self->Header.Inner;
            uint left = ((delegate* unmanaged<nint, uint>)ComObject.Slot(inner, ComObject.ReleaseSlot))(inner);

            if (left == 0)
            {
                ShimContext.Runtime.D3D12X.CommittedResources.Release(self);
                NativeMemory.Free(self);
            }

            return left;
        }
        catch (Exception exception)
        {
            UnmanagedGuard.Caught(exception, "XboxCommittedResources.ReleaseResource");

            return default;
        }
    }

    private static ulong TitleAllocationSize(nint device, ResourceDescription* console, ResourceDescription* desktop)
    {
        if (ConsoleAllocationSizes.TryCommittedTextureSize(console, out ulong size))
        {
            return size;
        }

        return AllocationSize(device, desktop);
    }

    private static ulong AllocationSize(nint device, ResourceDescription* description)
    {
        ResourceAllocationInfo info;
        var query = (delegate* unmanaged<nint, ResourceAllocationInfo*, uint, uint, ResourceDescription*, ResourceAllocationInfo*>)ComObject.Slot(device, DesktopAllocationInfoSlot);
        query(device, &info, 0, 1, description);

        return info.Size;
    }
}
