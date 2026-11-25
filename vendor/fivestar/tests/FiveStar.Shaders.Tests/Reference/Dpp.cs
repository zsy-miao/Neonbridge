// Copyright (c) 2026 Five Star Recomp contributors.
// SPDX-License-Identifier: GPL-3.0-or-later

namespace FiveStar.Shaders.Tests.Reference;

/// <summary>
/// V_MOV_B32 with DPP16 / DPP8 under a full EXEC (RDNA ISA): a source outside the 16-lane row gives 0 with bound_ctrl,
/// else the lane keeps its old value; lanes whose row or bank is masked off keep their old value.
/// </summary>
public sealed record Dpp
{
    public string? RowControl { get; init; }

    public int Shift { get; init; }

    public bool BoundControl { get; init; }

    public int RowMask { get; init; } = 0xF;

    public int BankMask { get; init; } = 0xF;

    public int[]? QuadPattern { get; init; }

    public int[]? Dpp8Pattern { get; init; }

    /// <summary>The per-lane result; null where the source lane lies outside the modelled wave.</summary>
    public uint?[] Move(uint[] data, uint old)
    {
        var result = new uint?[data.Length];

        for (var lane = 0; lane < data.Length; lane++)
        {
            if (((RowMask >> ((lane >> 4) & 3)) & 1) == 0 || ((BankMask >> ((lane >> 2) & 3)) & 1) == 0)
            {
                result[lane] = old;

                continue;
            }

            var source = Source(lane);

            if (source is null)
            {
                result[lane] = BoundControl ? 0 : old;
            }
            else if (source.Value < data.Length)
            {
                result[lane] = data[source.Value];
            }
        }

        return result;
    }

    private int? Source(int lane)
    {
        if (QuadPattern is not null)
        {
            return (lane & ~3) | QuadPattern[lane & 3];
        }

        if (Dpp8Pattern is not null)
        {
            return (lane & ~7) | Dpp8Pattern[lane & 7];
        }

        return RowSource(lane);
    }

    private int? RowSource(int lane)
    {
        var row = lane & ~15;
        var inRow = lane & 15;

        switch (RowControl)
        {
            case "row_shr":
                return InRow(inRow - Shift, lane - Shift);
            case "row_shl":
                return InRow(inRow + Shift, lane + Shift);
            case "row_ror":
                return row | ((inRow - Shift) & 15);
            case "row_mirror":
                return row | (15 - inRow);
            case "row_half_mirror":
                return (lane & ~7) | (7 - (lane & 7));
            default:
                throw new InvalidOperationException($"unknown DPP control {RowControl}");
        }
    }

    /// <summary>The shifted source lane, or null when its position falls outside the 16-lane row.</summary>
    private static int? InRow(int position, int lane)
    {
        if (position < 0 || position >= 16)
        {
            return null;
        }

        return lane;
    }
}
