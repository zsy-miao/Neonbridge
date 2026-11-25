// Copyright (c) 2026 Five Star Recomp contributors.
// SPDX-License-Identifier: GPL-3.0-or-later

namespace FiveStar.Cli.Texture;

/// <summary>One scan worker's buffers for a page's stored bytes and its blocks, grown to the largest page seen.</summary>
internal sealed class BcPackScanBuffers
{
    public byte[] Stored { get; private set; } = [];

    public byte[] Blocks { get; private set; } = [];

    public ushort[] Symbols { get; private set; } = [];

    public void Reserve(int stored, int blocks)
    {
        if (Stored.Length < stored)
        {
            Stored = new byte[stored];
        }

        if (Blocks.Length < blocks)
        {
            Blocks = new byte[blocks];
        }
    }

    public void ReserveSymbols(long symbols)
    {
        if (Symbols.Length < symbols)
        {
            Symbols = new ushort[symbols];
        }
    }
}
