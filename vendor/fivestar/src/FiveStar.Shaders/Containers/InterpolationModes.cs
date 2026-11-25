// Copyright (c) 2026 Five Star Recomp contributors.
// SPDX-License-Identifier: GPL-3.0-or-later

namespace FiveStar.Shaders.Containers;

/// <summary>The HLSL modifiers of the DXIL interpolation modes of signature elements (PSV0, ISG1).</summary>
public static class InterpolationModes
{
    // Per DXIL::InterpolationMode: 0 undefined, 1 constant, 2 linear, 3 centroid, 4 noperspective, 5 noperspective
    // centroid, 6 sample, 7 noperspective sample.
    private static readonly string[] Modifiers =
    [
        string.Empty, "nointerpolation", string.Empty, "centroid", "noperspective", "noperspective centroid", "sample",
        "noperspective sample",
    ];

    /// <summary>The modifier of a mode followed by a space, or nothing for undefined, linear and unknown modes.</summary>
    public static string Prefix(int mode)
    {
        if (mode < 0 || mode >= Modifiers.Length || Modifiers[mode].Length == 0)
        {
            return string.Empty;
        }

        return Modifiers[mode] + " ";
    }
}
