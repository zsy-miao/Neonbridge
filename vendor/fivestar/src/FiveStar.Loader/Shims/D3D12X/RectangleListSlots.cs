// Copyright (c) 2026 Five Star Recomp contributors.
// SPDX-License-Identifier: GPL-3.0-or-later

using System.Runtime.InteropServices;
using FiveStar.Loader.Diagnostics;
using FiveStar.Loader.Native;
using FiveStar.Loader.Shims.D3D12X.Binding;

namespace FiveStar.Loader.Shims.D3D12X;

/// <summary>
/// Command-list slot 25, SetPipelineState, as a handler: a stand-in pipeline the driver has not built is not set and
/// marks the list so its draws and dispatches are skipped; the pipeline (unwrapped) is recorded for
/// <see cref="RectangleDraws"/> and forwarded to the desktop slot of the same number.
/// </summary>
internal static unsafe class RectangleListSlots
{
    public const int SetPipelineStateSlot = 25;

    public static void Register(XboxObjects objects)
    {
        delegate* unmanaged<XboxObject*, nint, void> setPipeline = &SetPipelineState;
        objects.SetHandler(XboxInterfaceKind.CommandList, SetPipelineStateSlot, (nint)setPipeline);
    }

    [HotPath]
    [UnmanagedCallersOnly]
    public static void SetPipelineState(XboxObject* self, nint pipeline)
    {
        try
        {
            var runtime = ShimContext.Runtime.D3D12X;
            nint desktop = runtime.Objects.Unwrap(pipeline);
            bool pending = false;
            var standIns = runtime.Pipelines?.StandIns;

            if (standIns is not null)
            {
                desktop = standIns.Desktop(desktop, out pending);
            }

            CommandListBindings.Of(self)->PipelinePending = pending ? 1 : 0;
            var activity = ListActivity.Of(self);
            activity->Pipeline = desktop;
            activity->StandIn = pending ? runtime.Objects.Unwrap(pipeline) : 0;
            runtime.Rectangles.SetPipeline(self, desktop);

            if (pending)
            {
                return;
            }

            nint inner = self->Inner;
            ((delegate* unmanaged<nint, nint, void>)ComObject.Slot(inner, SetPipelineStateSlot))(inner, desktop);
        }
        catch (Exception exception)
        {
            UnmanagedGuard.Caught(exception, "RectangleListSlots.SetPipelineState");
        }
    }
}
