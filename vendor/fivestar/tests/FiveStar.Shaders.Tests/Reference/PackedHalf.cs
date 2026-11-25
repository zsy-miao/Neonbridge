// Copyright (c) 2026 Five Star Recomp contributors.
// SPDX-License-Identifier: GPL-3.0-or-later

namespace FiveStar.Shaders.Tests.Reference;

/// <summary>VOP3P packed f16 and V_FMA_MIX semantics.</summary>
public static class PackedHalf
{
    private static readonly int[] None = [0, 0, 0];
    private static readonly int[] All = [1, 1, 1];

    /// <summary>
    /// V_PK_*_F16: the low lane takes half op_sel[i] of each source, the high lane half op_sel_hi[i]; each lane is one
    /// f16 rounding of the exact result.
    /// </summary>
    public static uint Operate(PackedOperation operation, uint[] sources, int[]? select = null, int[]? selectHigh = null, int[]? negateLow = null)
    {
        var low = Lane(operation, sources, select ?? None, negateLow ?? None);
        var high = Lane(operation, sources, selectHigh ?? All, None);

        return FloatBits.Pack(low, high);
    }

    /// <summary>
    /// V_FMA_MIX*: source i is f16 (half op_sel[i]) when op_sel_hi[i] is set, else f32. The exact a * b + c, unrounded.
    /// </summary>
    public static Rational FmaMix(uint[] sources, int[] select, int[] selectHigh)
    {
        var values = new Rational[3];

        for (var index = 0; index < 3; index++)
        {
            if (selectHigh[index] != 0)
            {
                var half = select[index] != 0 ? sources[index] >> 16 : sources[index];
                values[index] = FloatBits.ExactHalf(half);
            }
            else
            {
                values[index] = FloatBits.Exact(sources[index]);
            }
        }

        return (values[0] * values[1]) + values[2];
    }

    private static uint Lane(PackedOperation operation, uint[] sources, int[] select, int[] negate)
    {
        var values = new Rational[sources.Length];

        for (var index = 0; index < sources.Length; index++)
        {
            var half = select[index] != 0 ? sources[index] >> 16 : sources[index];
            var value = FloatBits.ExactHalf(half);
            values[index] = negate[index] != 0 ? -value : value;
        }

        switch (operation)
        {
            case PackedOperation.Fma:
                return FloatBits.Half((values[0] * values[1]) + values[2]);
            case PackedOperation.Add:
                return FloatBits.Half(values[0] + values[1]);
            default:
                return FloatBits.Half(values[0] * values[1]);
        }
    }
}
