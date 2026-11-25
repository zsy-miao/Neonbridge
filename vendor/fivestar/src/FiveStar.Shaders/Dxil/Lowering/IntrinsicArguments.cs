// Copyright (c) 2026 Five Star Recomp contributors.
// SPDX-License-Identifier: GPL-3.0-or-later

using FiveStar.Shaders.Dxil.Hlsl;
using FiveStar.Shaders.Dxil.Ir;

namespace FiveStar.Shaders.Dxil.Lowering;

/// <summary>
/// The arguments of an intrinsic call converted to one type (the usual conversions; literals become int or float, and
/// a floating request makes integers float), and componentwise maps over them.
/// </summary>
public sealed class IntrinsicArguments
{
    private readonly LoweringContext _context;

    public IntrinsicArguments(LoweringContext context)
    {
        _context = context;
    }

    public IReadOnlyList<RValue> Lower(SyntaxNode call)
    {
        return [.. call.Children.Select(argument => _context.Expressions.Lower(argument))];
    }

    /// <summary>All arguments in their common shape and kind.</summary>
    public (HlslType Type, IReadOnlyList<RValue> Values) Common(SyntaxNode call, bool floating)
    {
        var values = Lower(call);
        var kind = values.Select(value => value.Type.Kind).Aggregate(Conversions.Common);
        kind = Concrete(kind);

        if (floating && kind is not (ScalarKind.Float or ScalarKind.Double))
        {
            kind = ScalarKind.Float;
        }

        var shape = values.Select(value => value.Type).Aggregate((left, right) => Conversions.CommonShape(left, right, kind));
        shape = shape.WithKind(kind);

        return (shape, [.. values.Select(value => _context.Convert.Convert(value, shape))]);
    }

    /// <summary>A literal kind made concrete: int or float.</summary>
    public static ScalarKind Concrete(ScalarKind kind)
    {
        return kind switch
        {
            ScalarKind.LiteralInt => ScalarKind.Int,
            ScalarKind.LiteralFloat => ScalarKind.Float,
            ScalarKind.Bool => ScalarKind.Int,
            _ => kind,
        };
    }

    /// <summary>Applies <paramref name="operation"/> to each component of the arguments.</summary>
    public static RValue Map(HlslType type, IReadOnlyList<RValue> values, Func<Value[], Value> operation)
    {
        var components = new Value[type.Count];

        for (var index = 0; index < components.Length; index++)
        {
            var operands = new Value[values.Count];

            for (var argument = 0; argument < operands.Length; argument++)
            {
                operands[argument] = values[argument].Components[index];
            }

            components[index] = operation(operands);
        }

        return new RValue(type, components);
    }
}
