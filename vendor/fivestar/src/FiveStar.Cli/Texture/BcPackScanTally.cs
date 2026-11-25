// Copyright (c) 2026 Five Star Recomp contributors.
// SPDX-License-Identifier: GPL-3.0-or-later

using FiveStar.Formats.Ttpf.BcPack;

namespace FiveStar.Cli.Texture;

/// <summary>The counts of a BCPack scan for one page file or in total.</summary>
internal sealed class BcPackScanTally
{
    public long Pages { get; private set; }

    public long Converged { get; private set; }

    public long Failed { get; private set; }

    /// <summary>Gets the converged pages with a block that reaches an unfitted preset window slot.</summary>
    public long Unfitted { get; private set; }

    public long StoredBytes { get; private set; }

    /// <summary>Gets the output bytes of the converged pages.</summary>
    public long BlockBytes { get; private set; }

    public void Add(BcPackDecodeResult result, long storedBytes, long blockBytes)
    {
        Pages++;
        StoredBytes += storedBytes;

        if (!result.Success)
        {
            Failed++;

            return;
        }

        Converged++;
        BlockBytes += blockBytes;

        if (result.UnfittedBlocks > 0)
        {
            Unfitted++;
        }
    }

    public void Merge(BcPackScanTally other)
    {
        Pages += other.Pages;
        Converged += other.Converged;
        Failed += other.Failed;
        Unfitted += other.Unfitted;
        StoredBytes += other.StoredBytes;
        BlockBytes += other.BlockBytes;
    }
}
