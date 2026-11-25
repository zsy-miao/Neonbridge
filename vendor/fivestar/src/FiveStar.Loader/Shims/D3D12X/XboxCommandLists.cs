// Copyright (c) 2026 Five Star Recomp contributors.
// SPDX-License-Identifier: GPL-3.0-or-later

using FiveStar.Loader.Native;

namespace FiveStar.Loader.Shims.D3D12X;

/// <summary>
/// Command lists for device slots 12 and 73: a desktop ID3D12GraphicsCommandList in a
/// <see cref="XboxInterfaces.CommandListObjectSize"/>-byte wrapper. The game records most commands inline: it compares
/// the list's write pointer (+0x18) with its limits (+0x28 for draws, +0x30) and writes packets itself when there is
/// room, else calls the vtable slot. The wrapper keeps all three at 0, so every command reaches a slot.
/// </summary>
internal sealed unsafe class XboxCommandLists
{
    /// <summary>ID3D12Device::CreateCommandList in the desktop vtable (d3d12.h).</summary>
    private const int DesktopCreateCommandListSlot = 12;

    private readonly XboxObjects _objects;

    public XboxCommandLists(XboxObjects objects)
    {
        _objects = objects;
    }

    /// <summary>Creates a desktop list of <paramref name="type"/> on <paramref name="device"/> and returns it wrapped.</summary>
    public int Create(nint device, int type, nint allocator, nint pipelineState, nint* list)
    {
        if (list == null)
        {
            return HResult.Pointer;
        }

        *list = 0;
        Guid iid = D3D12XLayout.GraphicsCommandListIid;
        var create = (delegate* unmanaged<nint, uint, int, nint, nint, Guid*, nint*, int>)ComObject.Slot(device, DesktopCreateCommandListSlot);
        nint desktopAllocator = _objects.Unwrap(allocator);
        int hr = create(device, 0, type, desktopAllocator, _objects.Unwrap(pipelineState), &iid, list);
        hr = _objects.WrapResult(hr, XboxInterfaceKind.CommandList, list);

        if (!HResult.Failed(hr) && *list != 0)
        {
            ListSegments.Of((XboxObject*)*list)->Allocator = desktopAllocator;
            CommandObjectCounts.ListMade();
        }

        return hr;
    }
}
