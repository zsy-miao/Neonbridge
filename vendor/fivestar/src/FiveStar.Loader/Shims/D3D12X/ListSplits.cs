// Copyright (c) 2026 Five Star Recomp contributors.
// SPDX-License-Identifier: GPL-3.0-or-later

using FiveStar.Loader.Native;
using FiveStar.Loader.Shims.D3D12X.Binding;

namespace FiveStar.Loader.Shims.D3D12X;

/// <summary>
/// Splits a command list at a memory wait recorded after work (<see cref="MemoryWaits"/>): the desktop list recorded so
/// far is closed as a segment (<see cref="ListSegments"/>), and recording goes on in another desktop list on the same
/// allocator with the state the list had set (<see cref="ListReplayState"/>, the bindless root signatures, the
/// pipeline). At submission the segments run in order with each wait before the segment it precedes, so the work before
/// a wait is never held behind it. Open queries and predication are not carried over. On by default
/// (<c>--no-split-at-waits</c> turns it off). The same mechanism as victory-gta6's list cuts (`list.rs::cut`).
/// </summary>
internal static unsafe class ListSplits
{
    /// <summary>ID3D12CommandList::GetType; ID3D12GraphicsCommandList Close, Reset and the state setters (d3d12.h).</summary>
    private const int GetTypeSlot = 8;
    private const int CloseSlot = 9;
    private const int ResetSlot = 10;
    private const int SetTopologySlot = 20;
    private const int SetViewportsSlot = 21;
    private const int SetScissorsSlot = 22;
    private const int SetBlendFactorSlot = 23;
    private const int SetStencilReferenceSlot = 24;
    private const int SetPipelineStateSlot = 25;
    private const int SetIndexBufferSlot = 43;
    private const int SetVertexBuffersSlot = 44;
    private const int SetRenderTargetsSlot = 46;
    private const int SetDepthBoundsSlot = 62;

    /// <summary>ID3D12Device4::CreateCommandList1 (a list created closed) and ID3D12DeviceChild::GetDevice.</summary>
    private const int CreateClosedListSlot = 51;
    private const int GetDeviceSlot = 7;

    /// <summary>D3D12_COMMAND_LIST_TYPE_DIRECT and _BUNDLE.</summary>
    private const int DirectType = 0;
    private const int BundleType = 1;

    private static readonly Guid Device4Iid = new("e865df17-a9ee-46f9-a463-3098315aa2e5");

    private static long _splits;

    /// <summary>Whether a wait after work splits its list (on by default); otherwise such a wait is not kept.</summary>
    public static bool Enabled { get; set; } = true;

    /// <summary>Lists split at a wait so far.</summary>
    public static long Splits => Interlocked.Read(ref _splits);

    /// <summary>Closes the list's desktop list as a segment and records on in a new one; false when it cannot.</summary>
    public static bool Split(XboxObject* self)
    {
        if (!Enabled)
        {
            return false;
        }

        var segments = ListSegments.Of(self);
        nint inner = self->Inner;
        int type = ((delegate* unmanaged<nint, int>)ComObject.Slot(inner, GetTypeSlot))(inner);

        if (segments->Count == ListSegments.Capacity || segments->Allocator == 0 || type == BundleType)
        {
            return false;
        }

        nint next = TakeSpare(segments);

        if (next == 0)
        {
            next = CreateClosed(inner, type);
        }

        if (next == 0)
        {
            return false;
        }

        int closed = ((delegate* unmanaged<nint, int>)ComObject.Slot(inner, CloseSlot))(inner);
        nint pipeline = Pipeline(self);

        if (HResult.Failed(Reset(next, segments->Allocator, pipeline)))
        {
            // Nothing to record on: the closed list goes on as the only one, and the wait is not kept.
            ShimContext.Runtime.D3D12X.Warnings.Warn("split-reset", "[memory wait] a list split at a wait could not record on: its later commands are lost");

            return false;
        }

        if (!HResult.Failed(closed))
        {
            segments->Closed[segments->Count++] = inner;
        }

        self->Inner = next;
        Restore(self, next, type);
        Interlocked.Increment(ref _splits);

        return true;
    }

    private static nint TakeSpare(ListSegments* segments)
    {
        if (segments->Spares == 0)
        {
            return 0;
        }

        return (nint)segments->Spare[--segments->Spares];
    }

    /// <summary>A new desktop list of <paramref name="type"/>, created closed on the device of <paramref name="inner"/>.</summary>
    private static nint CreateClosed(nint inner, int type)
    {
        Guid device4Iid = Device4Iid;
        nint device = 0;

        if (HResult.Failed(((delegate* unmanaged<nint, Guid*, nint*, int>)ComObject.Slot(inner, GetDeviceSlot))(inner, &device4Iid, &device)))
        {
            return 0;
        }

        Guid listIid = D3D12XLayout.GraphicsCommandListIid;
        nint list = 0;
        int hr = ((delegate* unmanaged<nint, uint, int, int, Guid*, nint*, int>)ComObject.Slot(device, CreateClosedListSlot))(device, 0, type, 0, &listIid, &list);
        ComObject.Release(device);

        if (HResult.Failed(hr))
        {
            return 0;
        }

        CommandObjectCounts.SplitListMade();

        return list;
    }

    /// <summary>Resets <paramref name="list"/> on the game's allocator, or on a spare of it when that is recording elsewhere.</summary>
    private static int Reset(nint list, nint allocator, nint pipeline)
    {
        var reset = (delegate* unmanaged<nint, nint, nint, int>)ComObject.Slot(list, ResetSlot);
        int hr = reset(list, allocator, pipeline);

        if (hr == HResult.InvalidArgument)
        {
            hr = ShimContext.Runtime.D3D12X.SpareAllocators.Reset(list, allocator, pipeline, reset);
        }

        return hr;
    }

    /// <summary>The desktop pipeline the list has set, 0 for none or one not built yet.</summary>
    private static nint Pipeline(XboxObject* self)
    {
        if (CommandListBindings.Of(self)->PipelinePending != 0)
        {
            return 0;
        }

        return RectangleListState.Of(self)->Pipeline;
    }

    /// <summary>Sets on <paramref name="list"/> the state the wrapper's earlier segments had set.</summary>
    private static void Restore(XboxObject* self, nint list, int type)
    {
        CommandListBindingMethods.Rebind(self);
        nint pipeline = Pipeline(self);
        RectangleListState.Of(self)->VariantBound = 0;

        if (pipeline != 0)
        {
            ((delegate* unmanaged<nint, nint, void>)ComObject.Slot(list, SetPipelineStateSlot))(list, pipeline);
        }

        if (type == DirectType)
        {
            RestoreGraphics(ListReplayState.Of(self), list);
        }
    }

    internal static void RestoreGraphics(ListReplayState* state, nint list)
    {
        RestoreInput(state, list);
        RestoreOutput(state, list);

        if ((state->Set & ListReplayState.BlendBit) != 0)
        {
            ((delegate* unmanaged<nint, float*, void>)ComObject.Slot(list, SetBlendFactorSlot))(list, state->Blend);
        }

        if ((state->Set & ListReplayState.StencilBit) != 0)
        {
            ((delegate* unmanaged<nint, uint, void>)ComObject.Slot(list, SetStencilReferenceSlot))(list, state->StencilReference);
        }

        if ((state->Set & ListReplayState.DepthBoundsBit) != 0)
        {
            ((delegate* unmanaged<nint, float, float, void>)ComObject.Slot(list, SetDepthBoundsSlot))(list, state->DepthMin, state->DepthMax);
        }
    }

    private static void RestoreInput(ListReplayState* state, nint list)
    {
        if ((state->Set & ListReplayState.TopologyBit) != 0)
        {
            ((delegate* unmanaged<nint, uint, void>)ComObject.Slot(list, SetTopologySlot))(list, state->Topology);
        }

        if ((state->Set & ListReplayState.IndexBit) != 0)
        {
            ((delegate* unmanaged<nint, VertexBufferView*, void>)ComObject.Slot(list, SetIndexBufferSlot))(list, &state->Index);
        }

        var setVertex = (delegate* unmanaged<nint, uint, uint, VertexBufferView*, void>)ComObject.Slot(list, SetVertexBuffersSlot);

        for (uint slot = 0; slot < ListReplayState.MaxVertexBuffers; slot++)
        {
            if ((state->VertexMask & (1u << (int)slot)) != 0)
            {
                setVertex(list, slot, 1, state->VertexView(slot));
            }
        }
    }

    private static void RestoreOutput(ListReplayState* state, nint list)
    {
        if ((state->Set & ListReplayState.TargetsBit) != 0)
        {
            nint depth = (nint)state->DepthStencil;
            nint* depthHandle = null;

            if ((state->Set & ListReplayState.DepthStencilBit) != 0)
            {
                depthHandle = &depth;
            }

            ((delegate* unmanaged<nint, uint, long*, int, nint*, void>)ComObject.Slot(list, SetRenderTargetsSlot))(list, state->TargetCount, state->Targets, state->SingleHandle, depthHandle);
        }

        if ((state->Set & ListReplayState.ViewportsBit) != 0)
        {
            ((delegate* unmanaged<nint, uint, float*, void>)ComObject.Slot(list, SetViewportsSlot))(list, state->ViewportCount, state->Viewports);
        }

        if ((state->Set & ListReplayState.ScissorsBit) != 0)
        {
            ((delegate* unmanaged<nint, uint, int*, void>)ComObject.Slot(list, SetScissorsSlot))(list, state->ScissorCount, state->Scissors);
        }
    }
}
