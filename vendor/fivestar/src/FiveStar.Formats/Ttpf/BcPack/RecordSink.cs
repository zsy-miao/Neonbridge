// Copyright (c) 2026 Five Star Recomp contributors.
// SPDX-License-Identifier: GPL-3.0-or-later

using System.Runtime.CompilerServices;

namespace FiveStar.Formats.Ttpf.BcPack;

/// <summary>
/// Gathers the symbol vectors of each block into its record, lane by lane (on 16-byte blocks E then O of each lane:
/// <c>record[2 * lane + half]</c>), and hands the record to the assembler.
/// </summary>
internal unsafe struct RecordSink<TAssembler> : ISymbolVectorSink
    where TAssembler : struct, IBlockAssembler
{
    private const int MaxRecord = 32;

    private readonly int _lanes;
    private readonly int _perBlock;
    private readonly TAssembler _assembler;
    private fixed byte _record[MaxRecord];

    public RecordSink(LaneLayout layout, TAssembler assembler)
    {
        _lanes = layout.Lanes;
        _perBlock = layout.SymbolsPerBlock;
        _assembler = assembler;
        InvalidBlocks = 0;
    }

    /// <summary>Gets the blocks whose record names no valid block (left zero).</summary>
    public int InvalidBlocks { get; private set; }

    [HotPath]
    [MethodImpl(MethodImplOptions.AggressiveInlining)]
    public void Emit(int vector, uint* symbols)
    {
        var half = vector % _perBlock;

        for (var lane = 0; lane < _lanes; lane++)
        {
            _record[(lane * _perBlock) + half] = (byte)symbols[lane];
        }

        if (half != _perBlock - 1)
        {
            return;
        }

        fixed (byte* record = _record)
        {
            if (!_assembler.Assemble(vector / _perBlock, record))
            {
                InvalidBlocks++;
            }
        }
    }
}
