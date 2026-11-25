// Copyright (c) 2026 Five Star Recomp contributors.
// SPDX-License-Identifier: GPL-3.0-or-later

namespace FiveStar.Shaders.Dxil.Lowering;

/// <summary>Picks one of two values by a condition, for tables of signed and unsigned (or float) forms.</summary>
public static class Choice
{
    public static T Of<T>(bool condition, T whenTrue, T whenFalse)
    {
        if (condition)
        {
            return whenTrue;
        }

        return whenFalse;
    }
}
