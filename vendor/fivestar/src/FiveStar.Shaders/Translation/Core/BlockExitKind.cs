// Copyright (c) 2026 Five Star Recomp contributors.
// SPDX-License-Identifier: GPL-3.0-or-later

namespace FiveStar.Shaders.Translation.Core;

/// <summary>How a basic block passes control on.</summary>
internal enum BlockExitKind
{
    /// <summary>An unconditional branch or the fall-through into the next block.</summary>
    Jump,

    /// <summary>A conditional branch: the target when the condition holds, else the next block.</summary>
    Conditional,

    /// <summary>s_endpgm, or the fall-through past the last block.</summary>
    End,

    /// <summary>A mesh-shader barrier in two-phase mode: the block stops the phase and resumes after the barrier.</summary>
    Yield,
}
