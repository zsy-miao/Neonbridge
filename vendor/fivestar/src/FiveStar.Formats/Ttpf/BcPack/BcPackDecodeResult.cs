// Copyright (c) 2026 Five Star Recomp contributors.
// SPDX-License-Identifier: GPL-3.0-or-later

using System.Globalization;

namespace FiveStar.Formats.Ttpf.BcPack;

/// <summary>The result of a BCPack decode.</summary>
/// <param name="Status">Convergence status; the output is valid only for <see cref="BcPackDecodeStatus.Ok"/>.</param>
/// <param name="Consumed">Stream bytes the lanes read.</param>
/// <param name="ChunkLength">Length of the input chunk.</param>
/// <param name="Cycles">Decoder clocks simulated.</param>
/// <param name="UnfittedBlocks">Blocks whose reference chain reaches an unfitted preset window slot.</param>
/// <param name="Detail">The lane (<see cref="BcPackDecodeStatus.BadHeader"/>) or lane bit mask
/// (<see cref="BcPackDecodeStatus.IllegalEndState"/>), or the invalid blocks (<see cref="BcPackDecodeStatus.InvalidBlock"/>).</param>
public readonly record struct BcPackDecodeResult(
    BcPackDecodeStatus Status,
    int Consumed,
    int ChunkLength,
    long Cycles,
    int UnfittedBlocks,
    int Detail)
{
    public bool Success => Status == BcPackDecodeStatus.Ok;

    /// <summary>The status in words, for reports.</summary>
    public string Describe()
    {
        switch (Status)
        {
            case BcPackDecodeStatus.Ok:
                return $"ok ({Consumed} of {ChunkLength} bytes consumed)";
            case BcPackDecodeStatus.InvalidArgument:
                return "invalid block count or output size";
            case BcPackDecodeStatus.BadHeader:
                return $"lane {Detail}: header selects an alphabet index above 8";
            case BcPackDecodeStatus.NoProgress:
                return $"no progress (stream truncated or not this BCPack codec) at byte {Consumed}";
            case BcPackDecodeStatus.CycleLimit:
                return "cycle limit reached";
            case BcPackDecodeStatus.IllegalEndState:
                return $"lanes [{LaneList(Detail)}] ended in an illegal state";
            case BcPackDecodeStatus.TrailingData:
                return $"non-zero bytes after the end of the stream ({Consumed} of {ChunkLength} consumed)";
            case BcPackDecodeStatus.InvalidBlock:
                return $"{Detail} block records name no valid block";
            default:
                return Status.ToString();
        }
    }

    private static string LaneList(int mask)
    {
        var lanes = new List<string>();

        for (var lane = 0; lane < LaneRegisters.Width; lane++)
        {
            if ((mask & (1 << lane)) != 0)
            {
                lanes.Add(lane.ToString(CultureInfo.InvariantCulture));
            }
        }

        return string.Join(", ", lanes);
    }
}
