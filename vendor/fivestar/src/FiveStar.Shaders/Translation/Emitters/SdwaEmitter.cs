// Copyright (c) 2026 Five Star Recomp contributors.
// SPDX-License-Identifier: GPL-3.0-or-later

using FiveStar.Shaders.Translation.Core;
using static FiveStar.Shaders.Translation.Emitters.Operands;

namespace FiveStar.Shaders.Translation.Emitters;

/// <summary>
/// SDWA (sub-dword addressing): each source is a byte or word of its register (zero- or sign-extended), and the result
/// goes to a byte or word of the destination; dst_unused says whether the rest is zeroed, sign-extended or kept.
/// </summary>
internal sealed class SdwaEmitter
{
    private const int DwordBits = 32;

    // The operation runs into a scratch VGPR beyond the hardware range, then is merged into the destination.
    private const int ScratchVgpr = 1023;

    private static readonly string[] SdwaKeys = ["dst_sel", "dst_unused", "src0_sel", "src1_sel"];
    private static readonly string[] OutputKeys = ["clamp", "mul", "div"];

    // Selects: bit offset and width.
    private static readonly IReadOnlyDictionary<string, (int Shift, int Width)> Selects = new Dictionary<string, (int, int)>
    {
        ["BYTE_0"] = (0, 8),
        ["BYTE_1"] = (8, 8),
        ["BYTE_2"] = (16, 8),
        ["BYTE_3"] = (24, 8),
        ["WORD_0"] = (0, 16),
        ["WORD_1"] = (16, 16),
        ["DWORD"] = (0, 32),
    };

    private readonly TranslationState _state;
    private readonly OperandAccess _operands;
    private readonly ValuEmitter _valu;

    public SdwaEmitter(TranslationState state, OperandAccess operands, ValuEmitter valu)
    {
        _state = state;
        _operands = operands;
        _valu = valu;
    }

    public void Translate(string op, IReadOnlyList<Operand> a, InstructionModifiers modifiers)
    {
        var sources = a.ToList();

        if (sources.Count > Source0)
        {
            sources[Source0] = Select(a, Source0, modifiers.Value("src0_sel"), modifiers);
        }

        if (sources.Count > Source1)
        {
            sources[Source1] = Select(a, Source1, modifiers.Value("src1_sel"), modifiers);
        }

        var destinationSelect = modifiers.Value("dst_sel") ?? "DWORD";

        if (op.StartsWith("v_cmp", StringComparison.Ordinal) || destinationSelect == "DWORD")
        {
            _valu.Translate(op, sources, modifiers.Filter(SdwaKeys, keep: false));

            return;
        }

        sources[Destination] = Operand.Register(RegisterKey.Vgpr(ScratchVgpr));
        _valu.Translate(op, sources, modifiers.Filter(OutputKeys, keep: true));
        MergeResult(a[Destination], Selects[destinationSelect], modifiers.Value("dst_unused") ?? "UNUSED_PAD");
    }

    private Operand Select(IReadOnlyList<Operand> a, int position, string? select, InstructionModifiers modifiers)
    {
        var operand = a[position];

        if (select is null || select == "DWORD")
        {
            return operand;
        }

        var (shift, width) = Selects[select];
        var value = _operands.Read(operand);
        string extracted;

        if (modifiers.SignExtended.Contains(position))
        {
            extracted = $"asuint(asint({value} << {DwordBits - shift - width}) >> {DwordBits - width})";
        }
        else
        {
            extracted = $"(({value} >> {shift}) & {Hlsl.HexUint((1L << width) - 1)})";
        }

        return Operand.FromExpression(_operands.SnapOne(extracted)).WithModifiersOf(operand);
    }

    private void MergeResult(Operand destination, (int Shift, int Width) select, string unused)
    {
        const long DwordMask = 0xffffffff;

        var (shift, width) = select;
        var mask = ((1L << width) - 1) << shift;
        var value = $"((v1023 << {shift}) & {Hlsl.HexUint(mask)})";

        switch (unused)
        {
            case "UNUSED_PRESERVE":
                var current = "0u";

                if (_state.Written.Contains(destination.At(0)))
                {
                    current = _operands.Read(destination);
                }

                _operands.Write(destination, $"({current} & {Hlsl.HexUint(~mask & DwordMask)}) | {value}");
                break;
            case "UNUSED_SEXT":
                _operands.Write(destination, $"asuint(asint(v1023 << {DwordBits - width}) >> {DwordBits - width - shift})");
                break;
            default:
                _operands.Write(destination, value);
                break;
        }
    }
}
