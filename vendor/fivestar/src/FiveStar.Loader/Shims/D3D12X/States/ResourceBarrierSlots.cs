// Copyright (c) 2026 Five Star Recomp contributors.
// SPDX-License-Identifier: GPL-3.0-or-later

using System.Runtime.InteropServices;
using FiveStar.Loader.Diagnostics;
using FiveStar.Loader.Native;
using FiveStar.Loader.Shims.D3D12X.DebugLayer;
using FiveStar.Loader.Shims.D3D12X.Present;

namespace FiveStar.Loader.Shims.D3D12X.States;

/// <summary>
/// Command-list slots 26 (ResourceBarrier, *inferred* from the desktop order of slots 0 to 25; not called in boot) and
/// 9 (Close). A transition of a texture the shim created starts from the state the texture is in on the desktop
/// (its home state, or where an earlier barrier of the list moved it), not from the console state the game names;
/// Close moves every texture the list moved back home, so each list starts and ends with its textures at home.
/// Buffers rest in COMMON with implicit promotion: their transitions become UAV barriers (ordering only).
/// Resources the shim did not create keep the game's states. The first half of a split barrier is dropped and the
/// second does the transition; transitions cover all subresources.
/// </summary>
internal static unsafe class ResourceBarrierSlots
{
    public const int ResourceBarrierSlot = 26;
    public const int CloseSlot = 9;

    private static XboxDeviceFactory D3D12X => ShimContext.Runtime.D3D12X;

    public static void Bind(XboxObjects objects)
    {
        delegate* unmanaged<XboxObject*, uint, TransitionBarrier*, void> barrier = &ResourceBarrier;
        delegate* unmanaged<XboxObject*, int> close = &Close;
        objects.SetHandler(XboxInterfaceKind.CommandList, ResourceBarrierSlot, (nint)barrier);
        objects.SetHandler(XboxInterfaceKind.CommandList, CloseSlot, (nint)close);
    }

    [HotPath]
    [UnmanagedCallersOnly]
    public static void ResourceBarrier(XboxObject* self, uint count, TransitionBarrier* barriers)
    {
        try
        {
            ListSegments.Of(self)->Commands++;

            if (barriers == null || count == 0)
            {
                return;
            }

            GpuBreadcrumbs.Before(self, RecordedCommand.ResourceBarrier);

            var d3d12x = D3D12X;
            var states = d3d12x.ListStates.For(self);

            for (uint i = 0; i < count; i++)
            {
                Translate(d3d12x.Objects, self->Inner, states, barriers + i);
            }
        }
        catch (Exception exception)
        {
            UnmanagedGuard.Caught(exception, "ResourceBarrierSlots.ResourceBarrier");
        }
    }

    [HotPath]
    [UnmanagedCallersOnly]
    public static int Close(XboxObject* self)
    {
        try
        {
            nint inner = self->Inner;
            var states = D3D12X.ListStates.Find(self);

            if (states is not null && states.Count > 0)
            {
                Restore(inner, states);
            }

            GpuBreadcrumbs.Close(self);

            int hr = ((delegate* unmanaged<nint, int>)ComObject.Slot(inner, CloseSlot))(inner);

            // An invalid call recorded on the list: its next Reset fails on any allocator (SpareAllocators replaces it).
            if (HResult.Failed(hr))
            {
                WarnCloseFailed(hr);
            }

            return hr;
        }
        catch (Exception exception)
        {
            UnmanagedGuard.Caught(exception, "ResourceBarrierSlots.Close");

            return HResult.Fail;
        }
    }

    private static void WarnCloseFailed(int hr)
    {
        D3D12X.Warnings.Warn("close", $"[d3d12_xs] command list Close failed: {HResult.Format(hr)} (an invalid call was recorded; the list is replaced at its next Reset)");
    }

    [HotPath]
    private static void Translate(XboxObjects objects, nint list, ListResourceStates states, TransitionBarrier* barrier)
    {
        switch (barrier->Type)
        {
            case TransitionBarrier.TransitionType:
                Transition(objects, list, states, barrier);
                break;
            case TransitionBarrier.AliasingType:
                Aliasing(objects, list, barrier);
                break;
            default:
                Uav(list, objects.Unwrap(barrier->Resource));
                break;
        }
    }

    [HotPath]
    private static void Transition(XboxObjects objects, nint list, ListResourceStates states, TransitionBarrier* barrier)
    {
        if ((barrier->Flags & TransitionBarrier.BeginOnlyFlag) != 0)
        {
            return;
        }

        Move(objects, list, states, barrier->Resource, barrier->StateBefore, barrier->StateAfter, barrier->Subresource);
    }

    /// <summary>
    /// Moves the game's <paramref name="gameResource"/> to <paramref name="after"/> on <paramref name="list"/>: a
    /// texture the shim created from the state it is in on the list, a buffer by a UAV barrier, any other resource with
    /// the game's states. Shared by slot 26 and the enhanced barriers of slot 146 (<see cref="EnhancedBarrierSlots"/>).
    /// </summary>
    [HotPath]
    internal static void Move(XboxObjects objects, nint list, ListResourceStates states, nint gameResource, int before, int after, uint subresource)
    {
        var wrapper = objects.AsResource(gameResource);
        nint resource = objects.Unwrap(gameResource);

        if (wrapper == null || wrapper->Dimension == ResourceStates.UnknownDimension)
        {
            if (!IsLegal(states, before, after))
            {
                return;
            }

            var copy = TransitionBarrier.Of(resource, before, after);
            copy.Subresource = subresource;
            BarrierWriter.Write(list, &copy);

            return;
        }

        if (wrapper->Dimension == ResourceStates.BufferDimension)
        {
            Uav(list, resource);

            return;
        }

        int home = wrapper->HomeState;
        int current = states.Current(resource, home);
        after = ResourceStates.Allowed(after, wrapper->Flags);

        if (current == after)
        {
            return;
        }

        // A compute or copy list cannot name a graphics state: such a texture is moved by the direct lists only and
        // keeps its state here.
        if (!IsLegal(states, current, after))
        {
            return;
        }

        BarrierWriter.Transition(list, resource, current, after);
        states.Set(resource, home, after);
    }

    /// <summary>
    /// True when a transition from <paramref name="before"/> to <paramref name="after"/> names only states the list's
    /// type allows; else false, warned once (the desktop Close would fail and the list never reset again).
    /// </summary>
    [HotPath]
    private static bool IsLegal(ListResourceStates states, int before, int after)
    {
        if (((before | after) & ~states.LegalStates) == 0)
        {
            return true;
        }

        WarnIllegal(before, after);

        return false;
    }

    private static void WarnIllegal(int before, int after)
    {
        D3D12X.Warnings.Warn("barrier-list-type", $"[d3d12_xs] ResourceBarrier: a compute or copy list moved a texture from 0x{before:x} to 0x{after:x}, states its list type cannot name; left to the direct lists");
    }

    [HotPath]
    private static void Aliasing(XboxObjects objects, nint list, TransitionBarrier* barrier)
    {
        var copy = new TransitionBarrier
        {
            Type = TransitionBarrier.AliasingType,
            Resource = objects.Unwrap(barrier->Resource),
            AliasingAfter = objects.Unwrap(barrier->AliasingAfter),
        };
        BarrierWriter.Write(list, &copy);
    }

    /// <summary>A UAV barrier on the desktop <paramref name="resource"/>; 0 orders every UAV access.</summary>
    [HotPath]
    internal static void Uav(nint list, nint resource)
    {
        var copy = new TransitionBarrier { Type = TransitionBarrier.UavType, Resource = resource };
        BarrierWriter.Write(list, &copy);
    }

    [HotPath]
    private static void Restore(nint list, ListResourceStates states)
    {
        for (int i = 0; i < states.Count; i++)
        {
            var (resource, home, state) = states.At(i);

            if (state != home)
            {
                BarrierWriter.Transition(list, resource, state, home);
            }
        }

        states.Clear();
    }
}
