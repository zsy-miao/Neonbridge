// Copyright (c) 2026 Five Star Recomp contributors.
// SPDX-License-Identifier: GPL-3.0-or-later

namespace FiveStar.Shaders.Compilation;

/// <summary>What the DXIL backend made of a request.</summary>
public enum BackendVerdict
{
    /// <summary>Lowered, validated and written.</summary>
    Compiled,

    /// <summary>Not lowered: outside the backend's dialect, or the compile ended abnormally.</summary>
    NotLowered,

    /// <summary>Lowered, but rejected by the validator.</summary>
    Rejected,
}
