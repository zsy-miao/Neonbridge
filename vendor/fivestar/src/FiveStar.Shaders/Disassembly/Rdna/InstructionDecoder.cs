// Copyright (c) 2026 Five Star Recomp contributors.
// SPDX-License-Identifier: GPL-3.0-or-later

using System.Globalization;

namespace FiveStar.Shaders.Disassembly.Rdna;

/// <summary>
/// Picks the encoding of the dword at an address from its top bits (AMD "RDNA 2 Instruction Set Architecture",
/// "Microcode Formats"): bit 31 clear for VOP1/VOP2/VOPC, bits [31:30] = 10 for the scalar ALU formats, else bits
/// [31:26]. A dword that does not decode prints as <c>.long</c>, as llvm-objdump does, and decoding resumes after it.
/// </summary>
internal static class InstructionDecoder
{
    private const int VectorShift = 31;
    private const int ScalarShift = 30;
    private const uint ScalarPrefix = 0b10;
    private const int EncodingShift = 26;
    private const uint Interpolation = 0x32;
    private const uint Packed = 0x33;
    private const uint Vop3 = 0x35;
    private const uint Lds = 0x36;
    private const uint Flat = 0x37;
    private const uint Buffer = 0x38;
    private const uint TypedBuffer = 0x3A;
    private const uint Image = 0x3C;
    private const uint ScalarMemory = 0x3D;
    private const uint Export = 0x3E;

    public static Decoded Decode(CodeWords code, int at)
    {
        var decoded = TryDecode(code, at);

        if (decoded is null || decoded.Value.Text.Contains(OperandText.Undecodable, StringComparison.Ordinal))
        {
            return new Decoded(string.Create(CultureInfo.InvariantCulture, $".long 0x{code[at]:x8}"), 1);
        }

        return decoded.Value;
    }

    private static Decoded? TryDecode(CodeWords code, int at)
    {
        var word = code[at];

        if (word == 0)
        {
            return new Decoded("v_illegal", 1);
        }

        if (word >> VectorShift == 0)
        {
            return VectorDecoder.Decode(code, at);
        }

        if (word >> ScalarShift == ScalarPrefix)
        {
            return ScalarDecoder.Decode(code, at);
        }

        switch (word >> EncodingShift)
        {
            case Interpolation:
                return InterpolationDecoder.Decode(code, at);
            case Packed:
                return PackedDecoder.Decode(code, at);
            case Vop3:
                return Vop3Decoder.Decode(code, at);
            case Lds:
                return LdsDecoder.Decode(code, at);
            case Flat:
                return FlatDecoder.Decode(code, at);
            case Buffer:
                return BufferDecoder.DecodeBuffer(code, at);
            case TypedBuffer:
                return BufferDecoder.DecodeTyped(code, at);
            case Image:
                return ImageDecoder.Decode(code, at);
            case ScalarMemory:
                return ScalarMemoryDecoder.Decode(code, at);
            case Export:
                return ExportDecoder.Decode(code, at);
            default:
                return null;
        }
    }
}
