// Copyright (c) 2026 Five Star Recomp contributors.
// SPDX-License-Identifier: GPL-3.0-or-later

namespace FiveStar.Shaders.Disassembly.Rdna;

/// <summary>
/// The fields of a VOP3 instruction (AMD "RDNA 2 Instruction Set Architecture", VOP3A/VOP3B formats): VDST [7:0], ABS
/// [10:8] or SDST [14:8], OP_SEL [14:11], CLAMP [15], OP [25:16]; SRC0 [40:32], SRC1 [49:41], SRC2 [58:50], OMOD
/// [60:59], NEG [63:61].
/// </summary>
internal sealed class Vop3Fields
{
    public const int Sources = 3;

    private const int ByteWidth = 8;
    private const int AbsLow = 8;
    private const int AbsWidth = 3;
    private const int ScalarLow = 8;
    private const int ScalarWidth = 7;
    private const int OpSelLow = 11;
    private const int OpSelWidth = 4;
    private const int ClampBit = 15;
    private const int OpcodeLow = 16;
    private const int OpcodeWidth = 10;
    private const int SourceWidth = 9;
    private const int OmodLow = 27;
    private const int OmodWidth = 2;
    private const int NegLow = 29;
    private const int NegWidth = 3;

    public Vop3Fields(uint first, uint second)
    {
        Destination = Bits.Field(first, 0, ByteWidth);
        Abs = Bits.Field(first, AbsLow, AbsWidth);
        ScalarDestination = Bits.Field(first, ScalarLow, ScalarWidth);
        OpSel = Bits.Field(first, OpSelLow, OpSelWidth);
        Clamp = Bits.Flag(first, ClampBit);

        Opcode = Bits.Field(first, OpcodeLow, OpcodeWidth);
        SourceCodes = [.. Enumerable.Range(0, Sources).Select(index => Bits.Field(second, index * SourceWidth, SourceWidth))];
        Omod = Bits.Field(second, OmodLow, OmodWidth);
        Neg = Bits.Field(second, NegLow, NegWidth);
    }

    public int Destination { get; }

    public int Abs { get; }

    public int ScalarDestination { get; }

    public int OpSel { get; }

    public bool Clamp { get; }

    public int Opcode { get; }

    public IReadOnlyList<int> SourceCodes { get; }

    public int Omod { get; }

    public int Neg { get; }
}
