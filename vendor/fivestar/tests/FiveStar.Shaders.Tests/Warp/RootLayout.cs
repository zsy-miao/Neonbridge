// Copyright (c) 2026 Five Star Recomp contributors.
// SPDX-License-Identifier: GPL-3.0-or-later

namespace FiveStar.Shaders.Tests.Warp;

/// <summary>What the root signature holds besides the user-SGPR root constants at b0.</summary>
public enum RootLayout
{
    /// <summary>Only the root constants; every resource is reached through the heaps.</summary>
    Bindless,

    /// <summary>Mesh fallback: u0..u2 and t0 (space 9) through a descriptor table.</summary>
    FallbackTable,

    /// <summary>Mesh fallback: u0..u2 and t0 (space 9) as root descriptors.</summary>
    FallbackRoot,
}
