// Copyright (c) 2026 Five Star Recomp contributors.
// SPDX-License-Identifier: GPL-3.0-or-later

namespace FiveStar.Shaders.Cache;

/// <summary>The seven shader slots of a graphics pipeline record, by the stage of the programs found in them.</summary>
public enum PipelineSlot
{
    /// <summary>Slot 0: pixel shader.</summary>
    Pixel = 0,

    /// <summary>Slot 1: legacy vertex shader.</summary>
    Vertex = 1,

    /// <summary>Slot 2: mesh (NGG) shader.</summary>
    Mesh = 2,

    /// <summary>Slot 3: never used.</summary>
    Slot3 = 3,

    /// <summary>Slot 4: never used.</summary>
    Slot4 = 4,

    /// <summary>Slot 5: never used.</summary>
    Slot5 = 5,

    /// <summary>Slot 6: used by two pipelines whose programs are not in the SGD5 tables.</summary>
    Slot6 = 6,
}
