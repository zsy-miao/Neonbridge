// Copyright (c) 2026 Five Star Recomp contributors.
// SPDX-License-Identifier: GPL-3.0-or-later

namespace FiveStar.Loader.Shims.D3D12X.Pipelines;

/// <summary>
/// The game's primitive topology types as desktop D3D12_PRIMITIVE_TOPOLOGY_TYPE. The stream builder (0x14086f090) maps
/// its topology byte through a table that also yields 19, a console-only type for the topology 0x13; the command-list
/// path (0x140032570) draws topologies 6 and 0x13 as triangle lists on the D3D12 interface, so 19 is a triangle type
/// (rectangle list, *inferred*).
/// </summary>
internal static class TopologyTypes
{
    /// <summary>D3D12_PRIMITIVE_TOPOLOGY_TYPE_POINT, the first defined type.</summary>
    public const int Point = 1;

    /// <summary>D3D12_PRIMITIVE_TOPOLOGY_TYPE_PATCH, the last defined type.</summary>
    public const int Patch = 4;

    /// <summary>The console-only type the game's table gives topology 0x13.</summary>
    public const int ConsoleRectangle = 19;

    /// <summary>
    /// The desktop type: point, line, triangle and patch pass; the console rectangle type, undefined (0) and any
    /// other value become triangle, the class the game's draws use for them.
    /// </summary>
    public static int Desktop(int type)
    {
        if (type >= Point && type <= Patch)
        {
            return type;
        }

        return PipelineLayout.TriangleTopology;
    }
}
