// Copyright (c) 2026 Five Star Recomp contributors.
// SPDX-License-Identifier: GPL-3.0-or-later

using System.Runtime.InteropServices;
using FiveStar.Loader.Diagnostics;
using FiveStar.Loader.Shims.D3D12X.DebugLayer;

namespace FiveStar.Loader.Shims.D3D12X.States;

/// <summary>
/// Command-list slot 146: Barrier(UINT32 NumBarrierGroups, const D3D12_BARRIER_GROUP*), the enhanced barriers of
/// ID3D12GraphicsCommandList7 (d3d12.h). The game queues groups at its context +0xA0 (0x1400095e0: buffer groups of
/// 40-byte D3D12_BUFFER_BARRIERs, 0x1400097f0: texture groups of 64-byte D3D12_TEXTURE_BARRIERs, all subresources)
/// and flushes them through this slot (0x14003210d, 0x144da61d1). Desktop resources rest in legacy states
/// (<see cref="ResourceStates"/>), on which the debug layer refuses enhanced barriers (#1350), so each barrier becomes
/// what slot 26 would write: a texture layout change a transition to the layout's state, a buffer barrier a UAV
/// barrier, a group of global barriers one UAV barrier on every resource. A texture barrier that keeps its layout
/// orders unordered access only.
/// </summary>
internal static unsafe class EnhancedBarrierSlots
{
    public const int BarrierSlot = 146;

    /// <summary>D3D12_BARRIER_TYPE: GLOBAL, TEXTURE, BUFFER.</summary>
    private const uint GlobalType = 0;
    private const uint TextureType = 1;
    private const uint BufferType = 2;

    /// <summary>D3D12_BARRIER_ACCESS_UNORDERED_ACCESS and D3D12_BARRIER_LAYOUT_UNDEFINED.</summary>
    private const uint UnorderedAccess = 0x10;
    private const uint UndefinedLayout = 0xFFFFFFFF;

    // D3D12_BARRIER_GROUP: Type, NumBarriers, pointer to the barriers.
    private const int GroupSize = 16;
    private const int GroupCountOffset = 4;
    private const int GroupBarriersOffset = 8;

    // D3D12_BUFFER_BARRIER: SyncBefore, SyncAfter, AccessBefore, AccessAfter, pResource, Offset, Size.
    private const int BufferBarrierSize = 40;
    private const int BufferResourceOffset = 16;

    // D3D12_TEXTURE_BARRIER: Sync, Access, LayoutBefore, LayoutAfter, pResource, Subresources, Flags.
    private const int TextureBarrierSize = 64;
    private const int AccessBeforeOffset = 8;
    private const int AccessAfterOffset = 12;
    private const int LayoutBeforeOffset = 16;
    private const int LayoutAfterOffset = 20;
    private const int TextureResourceOffset = 24;
    private const int SubresourceOffset = 32;

    private static XboxDeviceFactory D3D12X => ShimContext.Runtime.D3D12X;

    public static void Bind(XboxObjects objects)
    {
        delegate* unmanaged<XboxObject*, uint, byte*, void> barrier = &Barrier;
        objects.SetHandler(XboxInterfaceKind.CommandList, BarrierSlot, (nint)barrier);
    }

    [HotPath]
    [UnmanagedCallersOnly]
    public static void Barrier(XboxObject* self, uint count, byte* groups)
    {
        try
        {
            ListSegments.Of(self)->Commands++;

            if (groups == null || count == 0)
            {
                return;
            }

            GpuBreadcrumbs.Before(self, RecordedCommand.EnhancedBarrier);

            var d3d12x = D3D12X;
            var states = d3d12x.ListStates.For(self);

            for (uint i = 0; i < count; i++)
            {
                Group(d3d12x.Objects, self->Inner, states, groups + ((nint)i * GroupSize));
            }
        }
        catch (Exception exception)
        {
            UnmanagedGuard.Caught(exception, "EnhancedBarrierSlots.Barrier");
        }
    }

    [HotPath]
    private static void Group(XboxObjects objects, nint list, ListResourceStates states, byte* group)
    {
        uint type = *(uint*)group;
        uint count = *(uint*)(group + GroupCountOffset);
        byte* barriers = *(byte**)(group + GroupBarriersOffset);

        if (barriers == null || count == 0)
        {
            return;
        }

        if (type == GlobalType)
        {
            ResourceBarrierSlots.Uav(list, 0);

            return;
        }

        for (uint i = 0; i < count; i++)
        {
            if (type == BufferType)
            {
                byte* buffer = barriers + ((nint)i * BufferBarrierSize);
                ResourceBarrierSlots.Uav(list, objects.Unwrap(*(nint*)(buffer + BufferResourceOffset)));
            }
            else if (type == TextureType)
            {
                Texture(objects, list, states, barriers + ((nint)i * TextureBarrierSize));
            }
        }
    }

    [HotPath]
    private static void Texture(XboxObjects objects, nint list, ListResourceStates states, byte* barrier)
    {
        uint before = *(uint*)(barrier + LayoutBeforeOffset);
        uint after = *(uint*)(barrier + LayoutAfterOffset);
        nint resource = *(nint*)(barrier + TextureResourceOffset);

        if (after == UndefinedLayout)
        {
            return;
        }

        if (before == after)
        {
            uint access = *(uint*)(barrier + AccessBeforeOffset) | *(uint*)(barrier + AccessAfterOffset);

            if ((access & UnorderedAccess) != 0)
            {
                ResourceBarrierSlots.Uav(list, objects.Unwrap(resource));
            }

            return;
        }

        // UNDEFINED is past the table, so it reads as COMMON.
        int beforeState = BarrierLayouts.ToState((int)before);
        uint subresource = *(uint*)(barrier + SubresourceOffset);
        ResourceBarrierSlots.Move(objects, list, states, resource, beforeState, BarrierLayouts.ToState((int)after), subresource);
    }
}
