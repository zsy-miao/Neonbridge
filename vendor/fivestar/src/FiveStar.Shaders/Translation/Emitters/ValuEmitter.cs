// Copyright (c) 2026 Five Star Recomp contributors.
// SPDX-License-Identifier: GPL-3.0-or-later

using System.Globalization;
using FiveStar.Shaders.Translation.Core;
using static FiveStar.Shaders.Translation.Emitters.Operands;

namespace FiveStar.Shaders.Translation.Emitters;

/// <summary>
/// Vector ALU instructions (VOP1/VOP2/VOP3/VOPC): every result is written only on active lanes. Float results honour
/// the VOP3 output modifiers (mul:N, div:N, clamp).
/// </summary>
internal sealed class ValuEmitter
{
    private readonly OperandAccess _operands;
    private readonly ValuExtendedEmitter _extended;
    private readonly VectorCompareEmitter _compare;

    public ValuEmitter(OperandAccess operands, ValuExtendedEmitter extended, VectorCompareEmitter compare)
    {
        _operands = operands;
        _extended = extended;
        _compare = compare;
    }

    /// <summary>Applies the VOP3 output modifiers to a float expression.</summary>
    public static string OutputModifiers(string value, InstructionModifiers modifiers)
    {
        var multiply = modifiers.Value("mul");

        if (multiply is not null)
        {
            value = $"({value}) * {Hlsl.Float(double.Parse(multiply, CultureInfo.InvariantCulture))}";
        }

        var divide = modifiers.Value("div");

        if (divide is not null)
        {
            value = $"({value}) / {Hlsl.Float(double.Parse(divide, CultureInfo.InvariantCulture))}";
        }

        if (modifiers.Has("clamp"))
        {
            value = $"saturate({value})";
        }

        return value;
    }

    public void Translate(string op, IReadOnlyList<Operand> a, InstructionModifiers modifiers)
    {
        if (op == "v_mov_b32")
        {
            Move(a);

            return;
        }

        if (_extended.TryTranslate(op, a, modifiers))
        {
            return;
        }

        Func<int, string> f = k => _operands.ReadFloat(a[k]);
        Func<int, string> u = k => _operands.Read(a[k]);
        Func<int, string> i = k => _operands.ReadInt(a[k]);
        var floatValue = ValuExpressions.Float(op, f, u, i);

        if (floatValue is not null)
        {
            _operands.Write(a[Destination], $"asuint({OutputModifiers(floatValue, modifiers)})");

            return;
        }

        var integerValue = ValuExpressions.Integer(op, f, u, i);

        if (integerValue is not null)
        {
            if (modifiers.Has("clamp"))
            {
                throw new UnsupportedProgramException($"{op} clamp");
            }

            _operands.Write(a[Destination], integerValue);

            return;
        }

        TranslateMaskOperation(op, a, modifiers);
    }

    private void TranslateMaskOperation(string op, IReadOnlyList<Operand> a, InstructionModifiers modifiers)
    {
        if (_compare.TryTranslate(op, a, modifiers))
        {
            return;
        }

        if (op != "v_cndmask_b32")
        {
            throw new UnsupportedProgramException(op);
        }

        // v_cndmask: select src1 where the mask bit is set (VCC unless a mask operand is given).
        var select = a.Count > Source2 ? a[Source2] : OperandParser.Parse("vcc_lo");
        var first = ModifiedBits(a[Source0]);
        var second = ModifiedBits(a[Source1]);
        _operands.Write(a[Destination], $"({_operands.Mask(select)} ? {second} : {first})");
    }

    private string ModifiedBits(Operand operand)
    {
        if (operand.Negate || operand.Absolute)
        {
            return $"asuint({_operands.ReadFloat(operand)})";
        }

        return _operands.Read(operand);
    }

    private void Move(IReadOnlyList<Operand> a)
    {
        var source = a[Source0];
        var value = ModifiedBits(source);
        _operands.Write(a[Destination], value);
    }
}
