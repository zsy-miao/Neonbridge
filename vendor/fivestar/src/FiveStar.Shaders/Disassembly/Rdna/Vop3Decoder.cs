// Copyright (c) 2026 Five Star Recomp contributors.
// SPDX-License-Identifier: GPL-3.0-or-later

using System.Text;

namespace FiveStar.Shaders.Disassembly.Rdna;

/// <summary>
/// VOP3 (see <see cref="Vop3Fields"/>): VOPC, VOP2 and VOP1 instructions in their 64-bit form (suffix <c>_e64</c>)
/// and the instructions that exist only here. A literal may follow in a third dword.
/// </summary>
internal static class Vop3Decoder
{
    private const int Words = 2;
    private const int CarryOpcodeFirst = 0x128;
    private const int CarryOpcodeLast = 0x12A;
    private const int TiedSources = 2;
    private const int PermlaneSelects = 3;

    /// <summary>The source code of the inline constant 0, which current llvm-mc writes in the sources an instruction does not read.</summary>
    private const int InlineZero = 0x80;

    public static Decoded? Decode(CodeWords code, int at)
    {
        if (!code.Has(at, Words))
        {
            return null;
        }

        var fields = new Vop3Fields(code[at], code[at + 1]);

        if (InterpolationDecoder.IsVop3(fields.Opcode))
        {
            return InterpolationDecoder.DecodeVop3(fields);
        }

        var (info, suffix) = Lookup(fields.Opcode);

        if (info is null)
        {
            return null;
        }

        if (info.Destination == OperandType.None && fields.Opcode >= VectorOpcodes.Vop1Base)
        {
            if (fields.Destination != 0 || fields.SourceCodes.Any(source => source != 0))
            {
                return null;
            }

            return new Decoded(info.Name, Words);
        }

        var count = SourceCount(info, fields.Opcode);
        var words = Words;

        if (fields.SourceCodes.Skip(count).Any(source => source is not (0 or InlineZero)) || !ModifiersFit(info, fields, count))
        {
            return null;
        }

        if (!Sources.Literal(code, at, fields.SourceCodes.Take(count), ref words, out var literal))
        {
            return null;
        }

        var operands = Destinations(info, fields, code.Wave64);

        for (var index = 0; index < count; index++)
        {
            operands.Add(Source(info, fields, index, code.Wave64, literal));
        }

        if (operands.Contains(null))
        {
            return null;
        }

        return new Decoded(Sources.Join(info.Name + suffix, operands!) + Tail(info, fields, count), words);
    }

    private static (OpcodeInfo? Info, string Suffix) Lookup(int opcode)
    {
        if (opcode < VectorOpcodes.Vop2Base)
        {
            return (CompareOpcodes.Table.GetValueOrDefault(opcode), "_e64");
        }

        if (VectorOpcodes.CarryOut.TryGetValue(opcode, out var carry))
        {
            if (opcode is >= CarryOpcodeFirst and <= CarryOpcodeLast)
            {
                return (carry, "_e64");
            }

            return (carry, string.Empty);
        }

        if (opcode < VectorOpcodes.Vop2End)
        {
            var vop2 = VectorOpcodes.Vop2.GetValueOrDefault(opcode - VectorOpcodes.Vop2Base);

            if (vop2 is null || vop2.Bare || vop2.Name == "v_pk_fmac_f16")
            {
                return (null, string.Empty);
            }

            return (vop2, "_e64");
        }

        if (opcode is >= VectorOpcodes.Vop1Base and < VectorOpcodes.Vop1End)
        {
            var vop1 = VectorOpcodes.Vop1.GetValueOrDefault(opcode - VectorOpcodes.Vop1Base);

            if (vop1 is null || (vop1.Bare && vop1.Destination != OperandType.None))
            {
                return (null, string.Empty);
            }

            return (vop1, "_e64");
        }

        return (VectorOpcodes.Vop3Only.GetValueOrDefault(opcode), string.Empty);
    }

    /// <summary>
    /// Whether the set neg/abs bits belong to sources that take modifiers and OMOD to an instruction with a float result or source;
    /// llvm-objdump does not decode the dword otherwise.
    /// </summary>
    private static bool ModifiersFit(OpcodeInfo info, Vop3Fields fields, int count)
    {
        var carry = VectorOpcodes.CarryOut.ContainsKey(fields.Opcode);
        var floats = Modifiers.IsFloat(info.Destination);

        for (var index = 0; index < Vop3Fields.Sources; index++)
        {
            var modifiable = index < count && Modifiable(info, info.Sources[index]);
            var bits = (fields.Neg >> index) & 1;

            if (!carry)
            {
                bits |= (fields.Abs >> index) & 1;
            }

            if (bits != 0 && !modifiable)
            {
                return false;
            }

            floats |= modifiable && Modifiers.IsFloat(info.Sources[index]);
        }

        return fields.Omod == 0 || (floats && fields.Opcode >= VectorOpcodes.Vop2Base);
    }

    private static bool Modifiable(OpcodeInfo info, OperandType type)
    {
        return Modifiers.IsFloat(type) || (info.Name == "v_cndmask_b32" && type != OperandType.LaneMask);
    }

    /// <summary>Sources printed: v_mac/v_fmac tie the third source to the destination and leave it out.</summary>
    private static int SourceCount(OpcodeInfo info, int opcode)
    {
        var count = info.Sources.Count;

        if (opcode is >= VectorOpcodes.Vop2Base and < VectorOpcodes.Vop2End && count > TiedSources && info.Sources[TiedSources] != OperandType.LaneMask)
        {
            return TiedSources;
        }

        return count;
    }

    private static List<string?> Destinations(OpcodeInfo info, Vop3Fields fields, bool wave64)
    {
        var operands = new List<string?>();
        var mask = OperandText.Dwords(OperandType.LaneMask, wave64);

        if (fields.Opcode < VectorOpcodes.Vop2Base)
        {
            if (info.Destination != OperandType.None)
            {
                operands.Add(OperandText.Scalar(fields.Destination, mask));
            }

            return operands;
        }

        if (info.Name == "v_readlane_b32")
        {
            operands.Add(OperandText.Scalar(fields.Destination, 1));

            return operands;
        }

        operands.Add(OperandText.Vgpr(fields.Destination, OperandText.Dwords(info.Destination, wave64)));

        if (VectorOpcodes.CarryOut.ContainsKey(fields.Opcode))
        {
            operands.Add(OperandText.Scalar(fields.ScalarDestination, mask));
        }

        return operands;
    }

    private static string? Source(OpcodeInfo info, Vop3Fields fields, int index, bool wave64, uint literal)
    {
        var code = fields.SourceCodes[index];
        var type = info.Sources[index];
        var text = OperandText.Source(code, type, wave64, literal);

        if (text is null)
        {
            return null;
        }

        var modifiable = Modifiable(info, type);

        if (type == OperandType.LaneMask && (code >= OperandText.FirstVgpr || code == OperandText.ExecLo))
        {
            return OperandText.Invalid(text, "SReg_1_XEXEC");
        }

        if (!modifiable)
        {
            return LaneOperand(info.Name, index, code, text);
        }

        var absolute = false;

        if (!VectorOpcodes.CarryOut.ContainsKey(fields.Opcode))
        {
            absolute = ((fields.Abs >> index) & 1) != 0;
        }

        return Modifiers.Apply(text, ((fields.Neg >> index) & 1) != 0, absolute, Modifiers.IsConstant(code));
    }

    /// <summary>
    /// v_readlane reads a VGPR at an SGPR lane; v_writelane writes an SGPR (or LDS_DIRECT) value at an SGPR lane.
    /// llvm-objdump flags a register of the wrong kind.
    /// </summary>
    private static string LaneOperand(string name, int index, int code, string text)
    {
        var vgpr = code >= OperandText.FirstVgpr;

        if (name == "v_readlane_b32" && index == 0 && !vgpr)
        {
            return OperandText.Invalid(text, "VRegOrLds_32");
        }

        if (name == "v_writelane_b32" && index == 0 && vgpr)
        {
            return OperandText.Invalid(text, "SRegOrLds_32");
        }

        if ((name is "v_readlane_b32" or "v_writelane_b32" && index == 1 && vgpr) || (name.StartsWith("v_permlane", StringComparison.Ordinal) && index > 0 && vgpr))
        {
            return OperandText.Invalid(text, "SReg_32");
        }

        return text;
    }

    private static string Tail(OpcodeInfo info, Vop3Fields fields, int count)
    {
        var text = new StringBuilder();

        if (info.Name.StartsWith("v_permlane", StringComparison.Ordinal))
        {
            if ((fields.OpSel & PermlaneSelects) != 0)
            {
                text.Append(" op_sel:").Append(Modifiers.BitList(fields.OpSel, TiedSources));
            }
        }
        else if (fields.OpSel != 0 && HasOpSel(info))
        {
            var bits = (fields.OpSel & ((1 << count) - 1)) | (((fields.OpSel >> Vop3Fields.Sources) & 1) << count);
            text.Append(" op_sel:").Append(Modifiers.BitList(bits, count + 1));
        }

        if (fields.Clamp)
        {
            text.Append(" clamp");
        }

        text.Append(Modifiers.Output(fields.Omod));

        return text.ToString();
    }

    private static bool HasOpSel(OpcodeInfo info)
    {
        return info.Destination is OperandType.HalfInteger or OperandType.HalfReal
            || info.Sources.Any(type => type is OperandType.HalfInteger or OperandType.HalfReal);
    }
}
