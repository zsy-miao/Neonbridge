// Copyright (c) 2026 Five Star Recomp contributors.
// SPDX-License-Identifier: GPL-3.0-or-later

using FiveStar.Loader.Native;
using FiveStar.Loader.Shims.D3D12X.Present;

namespace FiveStar.Loader.Shims.D3D12X.States;

/// <summary>
/// Writes legacy barriers (ResourceBarrier) on a desktop list. Enhanced barriers (ID3D12GraphicsCommandList7::Barrier)
/// are not used: every desktop resource is created with a legacy state, and the debug layer refuses enhanced barriers
/// on a texture that is not in COMMON at the list boundary (error #1350), which render targets resting in
/// RENDER_TARGET never are.
/// </summary>
internal static unsafe class BarrierWriter
{
    /// <summary>ID3D12GraphicsCommandList::ResourceBarrier (d3d12.h).</summary>
    private const int ResourceBarrierSlot = 26;

    [HotPath]
    public static void Transition(nint list, nint resource, int before, int after)
    {
        var barrier = TransitionBarrier.Of(resource, before, after);
        Write(list, &barrier);
    }

    /// <summary>One D3D12_RESOURCE_BARRIER on <paramref name="list"/>.</summary>
    [HotPath]
    public static void Write(nint list, TransitionBarrier* barrier)
    {
        ((delegate* unmanaged<nint, uint, TransitionBarrier*, void>)ComObject.Slot(list, ResourceBarrierSlot))(list, 1, barrier);
    }
}
