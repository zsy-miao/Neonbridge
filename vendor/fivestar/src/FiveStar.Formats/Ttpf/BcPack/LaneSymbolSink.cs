// Copyright (c) 2026 Five Star Recomp contributors.
// SPDX-License-Identifier: GPL-3.0-or-later

namespace FiveStar.Formats.Ttpf.BcPack;

/// <summary>Stores the raw lane symbols, lane major: <c>symbols[lane * count + vector]</c>.</summary>
internal readonly unsafe struct LaneSymbolSink : ISymbolVectorSink
{
    private readonly ushort* _symbols;
    private readonly int _count;
    private readonly int _lanes;

    public LaneSymbolSink(ushort* symbols, int count, int lanes)
    {
        _symbols = symbols;
        _count = count;
        _lanes = lanes;
    }

    [HotPath]
    public void Emit(int block, uint* symbols)
    {
        for (var lane = 0; lane < _lanes; lane++)
        {
            _symbols[(lane * (long)_count) + block] = (ushort)symbols[lane];
        }
    }
}
