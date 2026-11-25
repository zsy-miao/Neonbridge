// Copyright (c) 2026 Five Star Recomp contributors.
// SPDX-License-Identifier: GPL-3.0-or-later

namespace FiveStar.Shaders.Hardware;

/// <summary>The hardware stage byte of an FSSM program header (u8 at 0x100).</summary>
public enum HardwareStage
{
    /// <summary>Legacy geometry shader (merged ES+GS) with a VS copy shader.</summary>
    Geometry = 3,

    /// <summary>Legacy hardware vertex shader.</summary>
    Vertex = 4,

    /// <summary>Pixel shader.</summary>
    Pixel = 5,

    /// <summary>Compute shader.</summary>
    Compute = 6,

    /// <summary>NGG primitive shader, the hardware form of a mesh shader.</summary>
    Ngg = 7,
}
