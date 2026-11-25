// Copyright (c) 2026 Five Star Recomp contributors.
// SPDX-License-Identifier: GPL-3.0-or-later

namespace FiveStar.Loader.Shims.D3D12X;

/// <summary>The D3D12_COMPARISON_FUNC values a memory wait takes (d3d12.h), as the GPU applies them: (memory &amp; mask) op value.</summary>
internal static class MemoryComparison
{
    public const int Never = 1;
    public const int Less = 2;
    public const int Equal = 3;
    public const int LessEqual = 4;
    public const int Greater = 5;
    public const int NotEqual = 6;
    public const int GreaterEqual = 7;
    public const int Always = 8;

    /// <summary>Whether a wait with <paramref name="comparison"/> can block: NEVER would hang and ALWAYS never waits.</summary>
    public static bool Blocks(int comparison)
    {
        return comparison is > Never and < Always;
    }

    /// <summary>Whether <paramref name="current"/> meets the wait.</summary>
    [HotPath]
    public static bool Holds(int comparison, ulong current, ulong value, ulong mask)
    {
        ulong masked = current & mask;

        return comparison switch
        {
            Never => false,
            Less => masked < value,
            Equal => masked == value,
            LessEqual => masked <= value,
            Greater => masked > value,
            NotEqual => masked != value,
            GreaterEqual => masked >= value,
            _ => true,
        };
    }
}
