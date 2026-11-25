// Copyright (c) 2026 Five Star Recomp contributors.
// SPDX-License-Identifier: GPL-3.0-or-later

using FiveStar.Shaders.Dxil.Hlsl;
using FiveStar.Shaders.Dxil.Ir;
using FiveStar.Shaders.Dxil.Ops;

namespace FiveStar.Shaders.Dxil.Lowering;

/// <summary>Vector intrinsics: dot, cross, length, normalize, distance, any, all.</summary>
public sealed class VectorIntrinsics
{
    private const int CrossSize = 3;
    private const int Z = 2;

    private readonly LoweringContext _context;
    private readonly IntrinsicArguments _arguments;

    public VectorIntrinsics(LoweringContext context, IntrinsicArguments arguments)
    {
        _context = context;
        _arguments = arguments;
    }

    private IrBuilder Builder => _context.Builder;

    public void Register(Dictionary<string, Func<SyntaxNode, RValue>> handlers)
    {
        handlers["dot"] = Dot;
        handlers["cross"] = Cross;
        handlers["length"] = Length;
        handlers["normalize"] = Normalize;
        handlers["distance"] = Distance;
        handlers["any"] = call => Reduce(call, BinaryOp.Or);

        handlers["all"] = call => Reduce(call, BinaryOp.And);
    }

    private RValue Dot(SyntaxNode call)
    {
        var (type, values) = _arguments.Common(call, floating: false);

        return RValue.Of(HlslType.ScalarOf(type.Kind), DotProduct(type.Kind, values[0], values[1]));
    }

    private Value DotProduct(ScalarKind kind, RValue a, RValue b)
    {
        var operators = _context.Expressions.Operators;
        var sum = operators.Scalar("*", kind, a.Components[0], b.Components[0]);

        for (var index = 1; index < a.Components.Count; index++)
        {
            sum = operators.Scalar("+", kind, sum, operators.Scalar("*", kind, a.Components[index], b.Components[index]));
        }

        return sum;
    }

    private RValue Cross(SyntaxNode call)
    {
        var (type, values) = _arguments.Common(call, floating: true);
        var (a, b) = (values[0].Components, values[1].Components);
        var components = new Value[CrossSize];

        for (var index = 0; index < CrossSize; index++)
        {
            var next = (index + 1) % CrossSize;
            var last = (index + Z) % CrossSize;
            components[index] = Builder.Binary(BinaryOp.Sub, Builder.Binary(BinaryOp.Mul, a[next], b[last]), Builder.Binary(BinaryOp.Mul, a[last], b[next]));
        }

        return new RValue(type, components);
    }

    private RValue Length(SyntaxNode call)
    {
        var (type, values) = _arguments.Common(call, floating: true);

        return RValue.Of(HlslType.ScalarOf(type.Kind), Sqrt(DotProduct(type.Kind, values[0], values[0])));
    }

    private RValue Distance(SyntaxNode call)
    {
        var (type, values) = _arguments.Common(call, floating: true);
        var difference = _context.Expressions.Operators.Binary("-", values[0], values[1]);

        return RValue.Of(HlslType.ScalarOf(type.Kind), Sqrt(DotProduct(type.Kind, difference, difference)));
    }

    private RValue Normalize(SyntaxNode call)
    {
        var (type, values) = _arguments.Common(call, floating: true);
        var scale = _context.Ops.Pure(DxOpCode.Rsqrt, "unary", values[0].Components[0].Type, DotProduct(type.Kind, values[0], values[0]));

        return new RValue(type, [.. values[0].Components.Select(component => Builder.Binary(BinaryOp.Mul, component, scale))]);
    }

    private Value Sqrt(Value value)
    {
        return _context.Ops.Pure(DxOpCode.Sqrt, "unary", value.Type, value);
    }

    private RValue Reduce(SyntaxNode call, BinaryOp combine)
    {
        var value = _context.Expressions.Lower(call[0]);
        var truths = _context.Convert.Convert(value, value.Type.WithKind(ScalarKind.Bool)).Components;
        var result = truths[0];

        for (var index = 1; index < truths.Count; index++)
        {
            result = Builder.Binary(combine, result, truths[index]);
        }

        return RValue.Of(HlslType.Bool, result);
    }
}
