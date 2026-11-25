// Copyright (c) 2026 Five Star Recomp contributors.
// SPDX-License-Identifier: GPL-3.0-or-later

namespace FiveStar.Formats.Ttpf.BcPack;

/// <summary>Turns the record of each block, in stream order, into the block (<see cref="RecordSink{TAssembler}"/>).</summary>
internal unsafe interface IBlockAssembler
{
    /// <summary>Writes block <paramref name="block"/>; false when its record names no valid block.</summary>
    bool Assemble(int block, byte* record);
}
