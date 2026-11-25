// Copyright (c) 2026 Five Star Recomp contributors.
// SPDX-License-Identifier: GPL-3.0-or-later

using System.Runtime.InteropServices;
using FiveStar.Loader.Diagnostics;
using FiveStar.Loader.Native;

namespace FiveStar.Loader.Shims.D3D12X;

/// <summary>
/// Command-list slot 20, IASetPrimitiveTopology. The desktop runtime does not validate the value, and a value the
/// desktop does not define hangs the GPU at the next list start (DXGI_ERROR_DEVICE_HUNG with no draw recorded).
/// The game's inline form (0x140032570) writes the value through a table to the hardware primitive type, which shows
/// 17 is the rectangle list (hardware 0x11); 6 and 19 the game turns into a triangle list itself. A rectangle list
/// becomes a triangle list, drawn with a rectangle-list pipeline variant (<see cref="RectangleDraws"/>); any other value the desktop does not define
/// is logged once and not set.
/// </summary>
internal static unsafe class PrimitiveTopologies
{
    public const int SetPrimitiveTopologySlot = 20;

    /// <summary>D3D_PRIMITIVE_TOPOLOGY ranges (d3dcommon.h): point list to triangle strip, the adjacency lists and strips, 1 to 32 control-point patch lists.</summary>
    private const int FirstBasic = 1;
    private const int LastBasic = 5;
    private const int FirstAdjacency = 10;
    private const int LastAdjacency = 13;
    private const int FirstPatchList = 33;
    private const int LastPatchList = 64;

    /// <summary>The console's rectangle list and the desktop triangle list that stands in for it.</summary>
    public const uint RectangleList = 17;
    public const uint TriangleList = 4;

    /// <summary>Values below this are warned about once without formatting a key on every call.</summary>
    private const int TrackedValues = 256;

    private static readonly bool[] Refused = new bool[TrackedValues];

    /// <summary>Whether the desktop defines <paramref name="topology"/>.</summary>
    public static bool IsDesktop(uint topology)
    {
        return topology is >= FirstBasic and <= LastBasic
            or >= FirstAdjacency and <= LastAdjacency
            or >= FirstPatchList and <= LastPatchList;
    }

    /// <summary>The desktop topology for a console one: the rectangle list as a triangle list, any other unchanged.</summary>
    public static uint Desktop(uint topology)
    {
        if (topology == RectangleList)
        {
            return TriangleList;
        }

        return topology;
    }

    [HotPath]
    [UnmanagedCallersOnly]
    public static void SetPrimitiveTopology(XboxObject* self, uint topology)
    {
        try
        {
            ShimContext.Runtime.D3D12X.Rectangles.SetTopology(self, topology);
            topology = Desktop(topology);

            if (!IsDesktop(topology))
            {
                Refuse(topology);

                return;
            }

            var state = ListReplayState.Of(self);
            state->Topology = topology;
            state->Set |= ListReplayState.TopologyBit;
            nint inner = self->Inner;
            ((delegate* unmanaged<nint, uint, void>)ComObject.Slot(inner, SetPrimitiveTopologySlot))(inner, topology);
        }
        catch (Exception exception)
        {
            UnmanagedGuard.Caught(exception, "PrimitiveTopologies.SetPrimitiveTopology");
        }
    }

    private static void Refuse(uint topology)
    {
        if (topology < Refused.Length)
        {
            if (Refused[topology])
            {
                return;
            }

            Refused[topology] = true;
        }

        ShimContext.Runtime.D3D12X.Warnings.Warn($"topology-{topology}", $"[d3d12_xs] IASetPrimitiveTopology({topology}): not a desktop topology, not set");
    }
}
