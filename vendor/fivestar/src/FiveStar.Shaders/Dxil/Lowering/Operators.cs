// Copyright (c) 2026 Five Star Recomp contributors.
// SPDX-License-Identifier: GPL-3.0-or-later

using FiveStar.Shaders.Dxil.Ir;

namespace FiveStar.Shaders.Dxil.Lowering;

/// <summary>
/// HLSL operators on scalar components: arithmetic, bitwise, shifts (the count masked to the width, as HLSL does),
/// comparisons and logic. Operations on two literals fold to a literal.
/// </summary>
public sealed class Operators
{
    private const int WordMask = 31;
    private const int LongMask = 63;

    private readonly LoweringContext _context;

    public Operators(LoweringContext context)
    {
        _context = context;
    }

    private IrBuilder Builder => _context.Builder;

    private DxilModule Module => _context.Module;

    public static bool IsComparison(string op)
    {
        return op is "==" or "!=" or "<" or ">" or "<=" or ">=";
    }

    /// <summary>A binary operator on two values (componentwise, after the usual conversions).</summary>
    public RValue Binary(string op, RValue left, RValue right)
    {
        if (op is "&&" or "||")
        {
            return Logical(op, left, right);
        }

        var kind = Conversions.Common(left.Type.Kind, right.Type.Kind);

        if (op is "<<" or ">>")
        {
            kind = ShiftKind(left.Type.Kind, right.Type.Kind);
        }

        var shape = Conversions.CommonShape(left.Type, right.Type, kind);
        var a = _context.Convert.Convert(left, shape);
        var b = _context.Convert.Convert(right, shape);
        var components = new Value[shape.Count];

        for (var index = 0; index < components.Length; index++)
        {
            components[index] = Scalar(op, kind, a.Components[index], b.Components[index]);
        }

        var result = shape;

        if (IsComparison(op))
        {
            result = shape.WithKind(ScalarKind.Bool);
        }

        return new RValue(result, components);
    }

    public Value Scalar(string op, ScalarKind kind, Value a, Value b)
    {
        var folded = Fold(op, kind, a, b);

        if (folded is not null)
        {
            return folded;
        }

        if (IsComparison(op))
        {
            return Builder.Compare(Predicate(op, kind), a, b);
        }

        if (Conversions.IsFloat(kind))
        {
            return FloatArithmetic(op, a, b);
        }

        return IntegerArithmetic(op, kind, a, b);
    }

    /// <summary>Unary minus, logical not, bitwise not.</summary>
    public RValue Unary(string op, RValue operand)
    {
        var type = operand.Type;

        if (op == "!")
        {
            var truth = _context.Convert.Convert(operand, type.WithKind(ScalarKind.Bool));

            return new RValue(truth.Type, [.. truth.Components.Select(component => Not(component))]);
        }

        if (type.Kind == ScalarKind.Bool)
        {
            type = type.WithKind(ScalarKind.Int);
            operand = _context.Convert.Convert(operand, type);
        }

        var kind = type.Kind;

        if (op == "~")
        {
            return new RValue(type, [.. operand.Components.Select(component => Scalar("^", kind, component, AllOnes(kind)))]);
        }

        var zero = _context.Convert.IntegerConstant(kind, 0);

        return new RValue(type, [.. operand.Components.Select(component => Negate(kind, zero, component))]);
    }

    public Value Not(Value value)
    {
        if (value is Constant constant)
        {
            return Module.Bool(constant.Bits == 0);
        }

        return Builder.Binary(BinaryOp.Xor, value, Module.Bool(true));
    }

    private Value Negate(ScalarKind kind, Value zero, Value value)
    {
        if (Conversions.IsFloat(kind) && value is not Constant)
        {
            // fneg: 0 - x would give +0 for x = +0.
            return Builder.Binary(BinaryOp.Sub, _context.Convert.FloatConstant(kind, -0.0), value);
        }

        return Scalar("-", kind, zero, value);
    }

    private Value AllOnes(ScalarKind kind)
    {
        return _context.Convert.IntegerConstant(kind, -1);
    }

    private RValue Logical(string op, RValue left, RValue right)
    {
        var shape = Conversions.CommonShape(left.Type, right.Type, ScalarKind.Bool);
        var a = _context.Convert.Convert(left, shape);
        var b = _context.Convert.Convert(right, shape);
        var code = op == "&&" ? BinaryOp.And : BinaryOp.Or;
        var components = new Value[shape.Count];

        for (var index = 0; index < components.Length; index++)
        {
            components[index] = Builder.Binary(code, a.Components[index], b.Components[index]);
        }

        return new RValue(shape, components);
    }

    private static ScalarKind ShiftKind(ScalarKind left, ScalarKind right)
    {
        if (left == ScalarKind.Bool)
        {
            return ScalarKind.Int;
        }

        if (left == ScalarKind.LiteralInt && right != ScalarKind.LiteralInt)
        {
            return Conversions.Common(left, right);
        }

        return left;
    }

    private Value FloatArithmetic(string op, Value a, Value b)
    {
        var code = op switch
        {
            "+" => BinaryOp.Add,
            "-" => BinaryOp.Sub,
            "*" => BinaryOp.Mul,
            "/" => BinaryOp.SDiv,
            "%" => BinaryOp.SRem,
            _ => throw new DxilBackendException($"operator {op} on floats"),
        };

        return Builder.Binary(code, a, b);
    }

    private Value IntegerArithmetic(string op, ScalarKind kind, Value a, Value b)
    {
        var signed = Conversions.IsSigned(kind);

        if (op is "<<" or ">>")
        {
            var width = a.Type.Bits == sizeof(long) * BitsPerByte ? LongMask : WordMask;
            var mask = Module.Int(a.Type, width);
            var count = Builder.Binary(BinaryOp.And, b, mask);
            var shift = BinaryOp.Shl;

            if (op == ">>")
            {
                shift = Choice.Of(signed, BinaryOp.AShr, BinaryOp.LShr);
            }

            return Builder.Binary(shift, a, count);
        }

        var code = op switch
        {
            "+" => BinaryOp.Add,
            "-" => BinaryOp.Sub,
            "*" => BinaryOp.Mul,
            "/" => Choice.Of(signed, BinaryOp.SDiv, BinaryOp.UDiv),
            "%" => Choice.Of(signed, BinaryOp.SRem, BinaryOp.URem),
            "&" => BinaryOp.And,
            "|" => BinaryOp.Or,
            "^" => BinaryOp.Xor,
            _ => throw new DxilBackendException($"operator {op} on integers"),
        };

        return Builder.Binary(code, a, b);
    }

    private const int BitsPerByte = 8;

    private static Predicate Predicate(string op, ScalarKind kind)
    {
        if (Conversions.IsFloat(kind))
        {
            return op switch
            {
                "==" => Ir.Predicate.FloatOrderedEqual,
                "!=" => Ir.Predicate.FloatUnorderedNotEqual,
                "<" => Ir.Predicate.FloatOrderedLess,
                ">" => Ir.Predicate.FloatOrderedGreater,
                "<=" => Ir.Predicate.FloatOrderedLessOrEqual,
                _ => Ir.Predicate.FloatOrderedGreaterOrEqual,
            };
        }

        var signed = Conversions.IsSigned(kind);

        return op switch
        {
            "==" => Ir.Predicate.Equal,
            "!=" => Ir.Predicate.NotEqual,
            "<" => Choice.Of(signed, Ir.Predicate.SignedLess, Ir.Predicate.UnsignedLess),
            ">" => Choice.Of(signed, Ir.Predicate.SignedGreater, Ir.Predicate.UnsignedGreater),
            "<=" => Choice.Of(signed, Ir.Predicate.SignedLessOrEqual, Ir.Predicate.UnsignedLessOrEqual),
            _ => Choice.Of(signed, Ir.Predicate.SignedGreaterOrEqual, Ir.Predicate.UnsignedGreaterOrEqual),
        };
    }

    /// <summary>Folds an operator on two literals (or two integer constants); null otherwise.</summary>
    private Value? Fold(string op, ScalarKind kind, Value a, Value b)
    {
        if (kind == ScalarKind.LiteralFloat && a is Constant x0 && b is Constant y0 && x0.Kind == ConstantKind.FloatingPoint && y0.Kind == ConstantKind.FloatingPoint)
        {
            return FoldLiteralFloat(op, BitConverter.Int64BitsToDouble(x0.Bits), BitConverter.Int64BitsToDouble(y0.Bits));
        }

        if (a is not Constant left || b is not Constant right || left.Kind != ConstantKind.Integer || right.Kind != ConstantKind.Integer)
        {
            return null;
        }

        if (kind is not (ScalarKind.LiteralInt or ScalarKind.Int or ScalarKind.Uint or ScalarKind.Int64 or ScalarKind.Uint64))
        {
            return null;
        }

        var x = left.Bits;
        var y = right.Bits;
        var unsigned = !Conversions.IsSigned(kind);

        if (kind == ScalarKind.Uint)
        {
            (x, y) = ((long)left.Unsigned, (long)right.Unsigned);
        }

        long? result = FoldInteger(op, x, y, unsigned, kind);

        if (result is null)
        {
            return null;
        }

        if (IsComparison(op))
        {
            return Module.Bool(result.Value != 0);
        }

        return _context.Convert.IntegerConstant(kind, result.Value);
    }

    /// <summary>Literal floats are exact doubles until they meet a type, as HLSL folds them.</summary>
    private Value? FoldLiteralFloat(string op, double x, double y)
    {
        double? result = op switch
        {
            "+" => x + y,
            "-" => x - y,
            "*" => x * y,
            "/" => x / y,
            _ => null,
        };

        if (result is not null)
        {
            return _context.Convert.FloatConstant(ScalarKind.LiteralFloat, result.Value);
        }

        if (IsComparison(op))
        {
            var truth = op switch
            {
                "==" => x == y,
                "!=" => x != y,
                "<" => x < y,
                ">" => x > y,
                "<=" => x <= y,
                _ => x >= y,
            };

            return Module.Bool(truth);
        }

        return null;
    }

    private static long? FoldInteger(string op, long x, long y, bool unsigned, ScalarKind kind)
    {
        var width = kind is ScalarKind.Int or ScalarKind.Uint ? WordMask : LongMask;

        return op switch
        {
            "+" => x + y,
            "-" => x - y,
            "*" => x * y,
            "/" when y == 0 => null,
            "%" when y == 0 => null,
            "/" when unsigned => (long)((ulong)x / (ulong)y),
            "/" => x / y,
            "%" when unsigned => (long)((ulong)x % (ulong)y),
            "%" => x % y,
            "&" => x & y,
            "|" => x | y,
            "^" => x ^ y,
            "<<" => x << (int)(y & width),
            ">>" when unsigned && kind == ScalarKind.Uint => (long)((ulong)x >> (int)(y & width)),
            ">>" when !unsigned && kind == ScalarKind.Int => (int)x >> (int)(y & width),
            ">>" when unsigned => (long)((ulong)x >> (int)(y & width)),
            ">>" => x >> (int)(y & width),
            "==" => Truth(x == y),
            "!=" => Truth(x != y),
            "<" when unsigned => Truth((ulong)x < (ulong)y),
            "<" => Truth(x < y),
            ">" when unsigned => Truth((ulong)x > (ulong)y),
            ">" => Truth(x > y),
            "<=" when unsigned => Truth((ulong)x <= (ulong)y),
            "<=" => Truth(x <= y),
            ">=" when unsigned => Truth((ulong)x >= (ulong)y),
            ">=" => Truth(x >= y),
            _ => null,
        };
    }

    private static long Truth(bool value)
    {
        if (value)
        {
            return 1;
        }

        return 0;
    }
}
