// Copyright (c) 2026 Five Star Recomp contributors.
// SPDX-License-Identifier: GPL-3.0-or-later

using System.Runtime.InteropServices;

namespace FiveStar.Loader.Shims.D3D12X;

/// <summary>
/// What a command list recorded since its last Reset, kept in its wrapper for <see cref="QueueProgress"/>: the last
/// desktop pipeline it set and how many draws, dispatches and indirect executions it forwarded.
/// </summary>
[StructLayout(LayoutKind.Sequential)]
internal unsafe struct ListActivity
{
    /// <summary>Offset in the command-list wrapper, after <see cref="ListSubmission"/> (0xE00 to 0xE10).</summary>
    public const int Offset = 0xE40;

    public nint Pipeline;
    public int Draws;
    public int Dispatches;
    public int Indirect;

    /// <summary>With <c>--d3d12-breadcrumbs</c> (<see cref="DebugLayer.GpuBreadcrumbs"/>): the recording's number, 0 for none.</summary>
    public uint Serial;

    /// <summary>The stand-in the list set while its pipeline is not built yet, else 0.</summary>
    public nint StandIn;

    /// <summary>The last stand-in whose work this recording skipped and no later work ran after, else 0.</summary>
    public nint Skipped;

    public static ListActivity* Of(XboxObject* list)
    {
        return (ListActivity*)((byte*)list + Offset);
    }

    /// <summary>A new recording: the counts start over.</summary>
    [HotPath]
    public static void Reset(XboxObject* list)
    {
        var activity = Of(list);

        activity->Pipeline = 0;
        activity->Draws = 0;
        activity->Dispatches = 0;
        activity->Indirect = 0;

        activity->StandIn = 0;
        activity->Skipped = 0;
        activity->Serial = DebugLayer.GpuBreadcrumbs.Begin(list);
    }
}
