// Copyright (c) 2026 Five Star Recomp contributors.
// SPDX-License-Identifier: GPL-3.0-or-later

using FiveStar.Loader.Native;

namespace FiveStar.Loader.Shims.D3D12X.Binding;

/// <summary>
/// The command-list side of <see cref="PassRules"/> (<c>--hold-passes</c>, <c>--stub-passes</c>): every draw, dispatch
/// and ExecuteIndirect notes its pipeline for the producers' state, and recorded work of a held or stubbed pass is
/// skipped (a stubbed draw first clears the list's render targets to the stub's colour).
/// </summary>
internal static unsafe class PassControls
{
    /// <summary>ID3D12GraphicsCommandList::ClearRenderTargetView (d3d12.h).</summary>
    private const int DesktopClearRenderTargetView = 48;

    /// <summary>The rules; none without the options.</summary>
    public static PassRules Rules { get; set; } = PassRules.Parse(null, null);

    /// <summary>Skipped work of a stand-in named <paramref name="name"/>.</summary>
    public static void Skipped(string name)
    {
        Rules.Note(name, skipped: true);
    }

    /// <summary>Recorded work of <paramref name="pipeline"/> on <paramref name="list"/>; true when it is to be skipped.</summary>
    public static bool Intercept(XboxObject* list, nint pipeline, bool draw)
    {
        string name = QueueProgress.PipelineName(pipeline);
        Rules.Note(name, skipped: false);
        var action = Rules.Decide(name, out var colour, out var reason);

        if (action == PassAction.Run)
        {
            return false;
        }

        ShimContext.Runtime.D3D12X.Warnings.Warn($"pass-{name}", $"[d3d12_xs] pipeline {name}: {reason}");

        if (action == PassAction.Stub && draw && colour is not null)
        {
            Clear(list, colour);
        }

        return true;
    }

    /// <summary>Clears the list's render targets as set (a single-handle range: its first target only).</summary>
    private static void Clear(XboxObject* list, float[] colour)
    {
        var state = ListReplayState.Of(list);

        if ((state->Set & ListReplayState.TargetsBit) == 0)
        {
            return;
        }

        uint count = state->SingleHandle != 0 ? Math.Min(state->TargetCount, 1u) : state->TargetCount;
        nint inner = list->Inner;
        var clear = (delegate* unmanaged<nint, long, float*, uint, void*, void>)ComObject.Slot(inner, DesktopClearRenderTargetView);

        fixed (float* rgba = colour)
        {
            for (uint i = 0; i < count; i++)
            {
                clear(inner, state->Targets[i], rgba, 0, null);
            }
        }
    }
}
