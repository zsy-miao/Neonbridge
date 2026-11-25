// Copyright (c) 2026 Five Star Recomp contributors.
// SPDX-License-Identifier: GPL-3.0-or-later

using System.Runtime.InteropServices;
using FiveStar.Loader.Diagnostics;
using FiveStar.Loader.Native;

namespace FiveStar.Loader.Shims.D3D12X;

/// <summary>
/// Command-list slots that set output state the desktop takes unchanged: RSSetViewports (21), RSSetScissorRects (22),
/// OMSetBlendFactor (23), OMSetStencilRef (24), OMSetRenderTargets (50, desktop 46) and OMSetDepthBounds (70, desktop
/// 62). Each is kept in <see cref="ListReplayState"/> for a split list (<see cref="ListSplits"/>), shown to
/// <see cref="DebugLayer.DrawTrace"/> with <c>--d3d12-trace-draws</c>, then forwarded.
/// </summary>
internal static unsafe class ListStateSlots
{
    public const int SetViewportsSlot = 21;
    public const int SetScissorsSlot = 22;
    public const int SetBlendFactorSlot = 23;
    public const int SetStencilReferenceSlot = 24;
    public const int SetRenderTargetsSlot = 50;
    public const int SetDepthBoundsSlot = 70;

    /// <summary>ID3D12GraphicsCommandList::OMSetRenderTargets and ID3D12GraphicsCommandList1::OMSetDepthBounds (d3d12.h).</summary>
    private const int DesktopSetRenderTargets = 46;
    private const int DesktopSetDepthBounds = 62;

    public static void Register(XboxObjects objects)
    {
        delegate* unmanaged<XboxObject*, uint, float*, void> viewports = &SetViewports;
        delegate* unmanaged<XboxObject*, uint, int*, void> scissors = &SetScissors;
        delegate* unmanaged<XboxObject*, float*, void> blend = &SetBlendFactor;
        delegate* unmanaged<XboxObject*, uint, void> stencil = &SetStencilReference;
        delegate* unmanaged<XboxObject*, uint, nint*, int, nint*, void> targets = &SetRenderTargets;
        delegate* unmanaged<XboxObject*, float, float, void> depthBounds = &SetDepthBounds;

        objects.SetHandler(XboxInterfaceKind.CommandList, SetViewportsSlot, (nint)viewports);
        objects.SetHandler(XboxInterfaceKind.CommandList, SetScissorsSlot, (nint)scissors);
        objects.SetHandler(XboxInterfaceKind.CommandList, SetBlendFactorSlot, (nint)blend);
        objects.SetHandler(XboxInterfaceKind.CommandList, SetStencilReferenceSlot, (nint)stencil);
        objects.SetHandler(XboxInterfaceKind.CommandList, SetRenderTargetsSlot, (nint)targets);
        objects.SetHandler(XboxInterfaceKind.CommandList, SetDepthBoundsSlot, (nint)depthBounds);
    }

    [HotPath]
    [UnmanagedCallersOnly]
    public static void SetViewports(XboxObject* self, uint count, float* viewports)
    {
        try
        {
            var state = ListReplayState.Of(self);
            state->Set &= ~ListReplayState.ViewportsBit;

            if (viewports != null && count <= ListReplayState.MaxViewports)
            {
                Buffer.MemoryCopy(viewports, state->Viewports, sizeof(float) * ListReplayState.MaxViewports * ListReplayState.ViewportFloats, sizeof(float) * count * ListReplayState.ViewportFloats);
                state->ViewportCount = count;
                state->Set |= ListReplayState.ViewportsBit;
            }

            ShimContext.Runtime.D3D12X.DrawTrace?.Viewports(self, count, viewports);
            nint inner = self->Inner;
            ((delegate* unmanaged<nint, uint, float*, void>)ComObject.Slot(inner, SetViewportsSlot))(inner, count, viewports);
        }
        catch (Exception exception)
        {
            UnmanagedGuard.Caught(exception, "ListStateSlots.SetViewports");
        }
    }

    [HotPath]
    [UnmanagedCallersOnly]
    public static void SetScissors(XboxObject* self, uint count, int* rects)
    {
        try
        {
            var state = ListReplayState.Of(self);
            state->Set &= ~ListReplayState.ScissorsBit;

            if (rects != null && count <= ListReplayState.MaxViewports)
            {
                Buffer.MemoryCopy(rects, state->Scissors, sizeof(int) * ListReplayState.MaxViewports * ListReplayState.RectInts, sizeof(int) * count * ListReplayState.RectInts);
                state->ScissorCount = count;
                state->Set |= ListReplayState.ScissorsBit;
            }

            ShimContext.Runtime.D3D12X.DrawTrace?.Scissors(self, count, rects);
            nint inner = self->Inner;
            ((delegate* unmanaged<nint, uint, int*, void>)ComObject.Slot(inner, SetScissorsSlot))(inner, count, rects);
        }
        catch (Exception exception)
        {
            UnmanagedGuard.Caught(exception, "ListStateSlots.SetScissors");
        }
    }

    [HotPath]
    [UnmanagedCallersOnly]
    public static void SetBlendFactor(XboxObject* self, float* factor)
    {
        try
        {
            var state = ListReplayState.Of(self);
            state->Set &= ~ListReplayState.BlendBit;

            if (factor != null)
            {
                Buffer.MemoryCopy(factor, state->Blend, sizeof(float) * ListReplayState.ColorFloats, sizeof(float) * ListReplayState.ColorFloats);
                state->Set |= ListReplayState.BlendBit;
            }

            nint inner = self->Inner;
            ((delegate* unmanaged<nint, float*, void>)ComObject.Slot(inner, SetBlendFactorSlot))(inner, factor);
        }
        catch (Exception exception)
        {
            UnmanagedGuard.Caught(exception, "ListStateSlots.SetBlendFactor");
        }
    }

    [HotPath]
    [UnmanagedCallersOnly]
    public static void SetStencilReference(XboxObject* self, uint reference)
    {
        try
        {
            var state = ListReplayState.Of(self);
            state->StencilReference = reference;
            state->Set |= ListReplayState.StencilBit;
            nint inner = self->Inner;
            ((delegate* unmanaged<nint, uint, void>)ComObject.Slot(inner, SetStencilReferenceSlot))(inner, reference);
        }
        catch (Exception exception)
        {
            UnmanagedGuard.Caught(exception, "ListStateSlots.SetStencilReference");
        }
    }

    [HotPath]
    [UnmanagedCallersOnly]
    public static void SetDepthBounds(XboxObject* self, float min, float max)
    {
        try
        {
            var state = ListReplayState.Of(self);
            state->DepthMin = min;
            state->DepthMax = max;
            state->Set |= ListReplayState.DepthBoundsBit;
            nint inner = self->Inner;
            ((delegate* unmanaged<nint, float, float, void>)ComObject.Slot(inner, DesktopSetDepthBounds))(inner, min, max);
        }
        catch (Exception exception)
        {
            UnmanagedGuard.Caught(exception, "ListStateSlots.SetDepthBounds");
        }
    }

    [HotPath]
    [UnmanagedCallersOnly]
    public static void SetRenderTargets(XboxObject* self, uint count, nint* handles, int single, nint* depthStencil)
    {
        try
        {
            Keep(ListReplayState.Of(self), count, handles, single, depthStencil);
            nint depth = 0;

            if (depthStencil != null)
            {
                depth = *depthStencil;
            }

            ShimContext.Runtime.D3D12X.DrawTrace?.RenderTargets(self, count, handles, single != 0, depth);
            nint inner = self->Inner;
            ((delegate* unmanaged<nint, uint, nint*, int, nint*, void>)ComObject.Slot(inner, DesktopSetRenderTargets))(inner, count, handles, single, depthStencil);
        }
        catch (Exception exception)
        {
            UnmanagedGuard.Caught(exception, "ListStateSlots.SetRenderTargets");
        }
    }

    /// <summary>The targets as given: one handle when <paramref name="single"/> is set (a descriptor range), else one per target.</summary>
    [HotPath]
    private static void Keep(ListReplayState* state, uint count, nint* handles, int single, nint* depthStencil)
    {
        state->Set &= ~(ListReplayState.TargetsBit | ListReplayState.DepthStencilBit);

        if (count > ListReplayState.MaxTargets || (count != 0 && handles == null))
        {
            return;
        }

        uint kept = count;

        if (single != 0 && kept > 1)
        {
            kept = 1;
        }

        for (uint i = 0; i < kept; i++)
        {
            state->Targets[i] = handles[i];
        }

        state->TargetCount = count;
        state->SingleHandle = single;
        state->Set |= ListReplayState.TargetsBit;

        if (depthStencil != null)
        {
            state->DepthStencil = *depthStencil;
            state->Set |= ListReplayState.DepthStencilBit;
        }
    }
}
