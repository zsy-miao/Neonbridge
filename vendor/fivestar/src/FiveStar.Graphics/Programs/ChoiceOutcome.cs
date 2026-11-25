// Copyright (c) 2026 Five Star Recomp contributors.
// SPDX-License-Identifier: GPL-3.0-or-later

namespace FiveStar.Graphics.Programs;

/// <summary>How a program runs on an adapter.</summary>
public enum ChoiceOutcome
{
    /// <summary>The native DXIL, with console results.</summary>
    Native,

    /// <summary>The wave32 variant of a wave64-pinned compute program (the adapter has no 64-lane waves).</summary>
    Variant,

    /// <summary>The compute + vertex-pulling fallback of a mesh program.</summary>
    Fallback,

    /// <summary>No form gives console results on the adapter (or none loads); the files are what would run.</summary>
    Incorrect,
}
