// Copyright (c) 2026 Five Star Recomp contributors.
// SPDX-License-Identifier: GPL-3.0-or-later

namespace FiveStar.Shaders.Tests.Reference;

/// <summary>A float slot of a BVH node: a number rounded to f32, or raw bits such as a NaN payload.</summary>
public readonly record struct BvhValue(uint Bits)
{
    public static BvhValue Of(double value)
    {
        return new BvhValue(FloatBits.Of(value));
    }

    public static BvhValue Raw(uint bits)
    {
        return new BvhValue(bits);
    }

    public static implicit operator BvhValue(double value)
    {
        return Of(value);
    }
}
