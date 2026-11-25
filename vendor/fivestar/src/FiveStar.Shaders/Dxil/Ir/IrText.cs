// Copyright (c) 2026 Five Star Recomp contributors.
// SPDX-License-Identifier: GPL-3.0-or-later

using System.Globalization;
using System.Text;

namespace FiveStar.Shaders.Dxil.Ir;

/// <summary>A readable listing of a function's blocks and instructions, for debugging the backend.</summary>
public static class IrText
{
    public static string Print(IrFunction function)
    {
        var names = new Dictionary<Value, string>();
        var text = new StringBuilder();

        foreach (var block in function.Blocks)
        {
            text.Append(CultureInfo.InvariantCulture, $"{block}:\n");

            foreach (var instruction in block.Phis.Concat(block.Body).Append(block.Terminator).OfType<Instruction>())
            {
                text.Append(CultureInfo.InvariantCulture, $"  {Name(instruction, names)} = {Describe(instruction, names)}\n");
            }
        }

        return text.ToString();
    }

    private static string Describe(Instruction instruction, Dictionary<Value, string> names)
    {
        var operands = string.Join(", ", instruction.Operands.Select(operand => Name(operand, names)));
        var targets = string.Join(", ", instruction.Targets);

        return $"{instruction.Opcode}.{instruction.Code} {instruction.Callee?.Name} {instruction.Type} ({operands}) [{targets}]";
    }

    private static string Name(Value value, Dictionary<Value, string> names)
    {
        if (value is Constant constant)
        {
            return $"{constant.Type} {constant.Kind} {constant.Bits}";
        }

        if (!names.TryGetValue(value, out var name))
        {
            name = $"%{names.Count}";
            names[value] = name;
        }

        return name;
    }
}
