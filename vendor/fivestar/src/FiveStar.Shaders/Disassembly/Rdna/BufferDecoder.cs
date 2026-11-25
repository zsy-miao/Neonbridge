// Copyright (c) 2026 Five Star Recomp contributors.
// SPDX-License-Identifier: GPL-3.0-or-later

using System.Globalization;
using System.Text;

namespace FiveStar.Shaders.Disassembly.Rdna;

/// <summary>
/// MUBUF and MTBUF: buffer access through a V# (AMD "RDNA 2 Instruction Set Architecture", MUBUF/MTBUF formats).
/// Fields: OFFSET [11:0], OFFEN [12], IDXEN [13], GLC [14], DLC [15]; MUBUF LDS [16], OP [24:18] and OP[7] [25]; MTBUF
/// OP [18:16], FORMAT [25:19] and OP[3] [53]; second dword VADDR [7:0], VDATA [15:8], SRSRC [20:16], SLC [22], TFE
/// [23], SOFFSET [31:24]. llvm-objdump ignores TFE in MTBUF and buffer atomics; a load with LDS names no data VGPR.
/// </summary>
internal static class BufferDecoder
{
    private const int Words = 2;
    private const int OffsetWidth = 12;
    private const int OffenBit = 12;
    private const int IdxenBit = 13;
    private const int GlcBit = 14;
    private const int DlcBit = 15;
    private const int LdsBit = 16;
    private const int BufferOpcodeLow = 18;
    private const int BufferOpcodeWidth = 7;
    private const int BufferOpcodeHighBit = 25;
    private const int TypedOpcodeLow = 16;
    private const int TypedOpcodeWidth = 3;
    private const int TypedOpcodeHighBit = 21;
    private const int FormatLow = 19;
    private const int FormatWidth = 7;
    private const int ByteWidth = 8;
    private const int DataLow = 8;
    private const int ResourceLow = 16;
    private const int ResourceWidth = 5;
    private const int SlcBit = 22;
    private const int TfeBit = 23;
    private const int SoffsetLow = 24;
    private const int ResourceDwords = 4;
    private const int PairDwords = 2;
    private const int CacheFlagBits = 5;

    /// <summary>The loads that can write LDS directly (LDS [16]); they name no data VGPR.</summary>
    private static readonly HashSet<string> LdsLoads =
        ["buffer_load_format_x", "buffer_load_ubyte", "buffer_load_sbyte", "buffer_load_ushort", "buffer_load_sshort", "buffer_load_dword"];

    public static Decoded? DecodeBuffer(CodeWords code, int at)
    {
        if (!code.Has(at, Words))
        {
            return null;
        }

        var first = code[at];
        var opcode = Bits.Field(first, BufferOpcodeLow, BufferOpcodeWidth) | (Bits.Field(first, BufferOpcodeHighBit, 1) << BufferOpcodeWidth);

        if (!MemoryOpcodes.Buffer.TryGetValue(opcode, out var entry))
        {
            return null;
        }

        if (entry.Dwords == 0)
        {
            return CacheInvalidate(entry.Name, first);
        }

        return Format(entry, first, code[at + 1], null);
    }

    public static Decoded? DecodeTyped(CodeWords code, int at)
    {
        if (!code.Has(at, Words))
        {
            return null;
        }

        var first = code[at];
        var second = code[at + 1];
        var opcode = Bits.Field(first, TypedOpcodeLow, TypedOpcodeWidth) | (Bits.Field(second, TypedOpcodeHighBit, 1) << TypedOpcodeWidth);

        if (!MemoryOpcodes.TypedBuffer.TryGetValue(opcode, out var entry))
        {
            return null;
        }

        return Format(entry, first, second, BufferFormats.Text(Bits.Field(first, FormatLow, FormatWidth)));
    }

    /// <summary>buffer_gl0_inv/buffer_gl1_inv: OFFEN, IDXEN, GLC, DLC and LDS must be clear; other fields are ignored.</summary>
    private static Decoded? CacheInvalidate(string name, uint first)
    {
        if (Bits.Field(first, OffenBit, CacheFlagBits) != 0)
        {
            return null;
        }

        return new Decoded(name, Words);
    }

    private static Decoded? Format((string Name, int Dwords) entry, uint first, uint second, string? format)
    {
        var lds = format is null && Bits.Flag(first, LdsBit);
        var atomic = entry.Name.Contains("_atomic_", StringComparison.Ordinal);

        if ((lds && (!LdsLoads.Contains(entry.Name) || Bits.Flag(second, TfeBit))) || (atomic && Bits.Flag(first, DlcBit)))
        {
            return null;
        }

        var offen = Bits.Flag(first, OffenBit);
        var idxen = Bits.Flag(first, IdxenBit);
        var tfe = Bits.Flag(second, TfeBit) && format is null && !atomic;
        var dataDwords = entry.Dwords;

        if (tfe)
        {
            dataDwords++;
        }

        var soffset = OperandText.Source(Bits.Field(second, SoffsetLow, ByteWidth), OperandType.Word, false, 0);

        if (soffset is null || Bits.Field(second, SoffsetLow, ByteWidth) == OperandText.Literal)
        {
            return null;
        }

        var data = OperandText.Vgpr(Bits.Field(second, DataLow, ByteWidth), dataDwords);
        var address = Address(Bits.Field(second, 0, ByteWidth), offen, idxen);
        var resource = OperandText.Scalar(Bits.Field(second, ResourceLow, ResourceWidth) * ResourceDwords, ResourceDwords) ?? OperandText.Undecodable;
        var text = new StringBuilder($"{entry.Name} {data}, {address}, {resource}, {soffset}{format}");

        if (lds)
        {
            text = new StringBuilder($"{entry.Name} {address}, {resource}, {soffset}");
        }

        AppendFlags(text, first, second, (idxen, offen, tfe), format is null);

        return new Decoded(text.ToString(), Words);
    }

    private static string Address(int register, bool offen, bool idxen)
    {
        if (offen && idxen)
        {
            return OperandText.Vgpr(register, PairDwords);
        }

        if (offen || idxen)
        {
            return OperandText.Vgpr(register, 1);
        }

        return "off";
    }

    private static void AppendFlags(StringBuilder text, uint first, uint second, (bool Idxen, bool Offen, bool Tfe) flags, bool untyped)
    {
        if (flags.Idxen)
        {
            text.Append(" idxen");
        }

        if (flags.Offen)
        {
            text.Append(" offen");
        }

        var offset = Bits.Field(first, 0, OffsetWidth);

        if (offset != 0)
        {
            text.Append(CultureInfo.InvariantCulture, $" offset:{offset}");
        }

        AppendFlag(text, first, GlcBit, " glc");
        AppendFlag(text, second, SlcBit, " slc");
        AppendFlag(text, first, DlcBit, " dlc");

        if (untyped)
        {
            AppendFlag(text, first, LdsBit, " lds");
        }

        if (flags.Tfe)
        {
            text.Append(" tfe");
        }
    }

    private static void AppendFlag(StringBuilder text, uint word, int bit, string flag)
    {
        if (Bits.Flag(word, bit))
        {
            text.Append(flag);
        }
    }
}
