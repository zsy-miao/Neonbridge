// Copyright (c) 2026 Five Star Recomp contributors.
// SPDX-License-Identifier: GPL-3.0-or-later

using FiveStar.Shaders.Dxil.Hlsl;
using FiveStar.Shaders.Dxil.Ir;
using FiveStar.Shaders.Dxil.Ops;

namespace FiveStar.Shaders.Dxil.Lowering;

/// <summary>
/// Arithmetic intrinsics, componentwise: bit casts, select, min/max/clamp/abs/saturate/sign, rounding, exp/log/pow,
/// trigonometry, bit counts, f16 conversions, mad/fma, float classes and the vector helpers.
/// </summary>
public sealed class MathIntrinsics
{
    private const string UnaryClass = "unary";
    private const int Third = 2;
    private const string BinaryClass = "binary";
    private const string TertiaryClass = "tertiary";
    private const string BitsClass = "unaryBits";
    private const string SpecialClass = "isSpecialFloat";
    private const double Log2E = 1.4426950408889634;
    private const double Ln2 = 0.69314718055994531;
    private const double Log10Of2 = 0.30102999566398120;
    private const int TopBit = 31;
    private const double SmoothstepThree = 3.0;
    private const double SmoothstepTwo = 2.0;

    private static readonly Dictionary<string, DxOpCode> FloatUnary = new(StringComparer.Ordinal)
    {
        ["sqrt"] = DxOpCode.Sqrt,
        ["rsqrt"] = DxOpCode.Rsqrt,
        ["floor"] = DxOpCode.RoundNi,
        ["ceil"] = DxOpCode.RoundPi,
        ["round"] = DxOpCode.RoundNe,
        ["trunc"] = DxOpCode.RoundZ,
        ["frac"] = DxOpCode.Frc,
        ["exp2"] = DxOpCode.Exp,
        ["log2"] = DxOpCode.Log,
        ["sin"] = DxOpCode.Sin,
        ["cos"] = DxOpCode.Cos,
        ["tan"] = DxOpCode.Tan,
        ["asin"] = DxOpCode.Asin,
        ["acos"] = DxOpCode.Acos,
        ["atan"] = DxOpCode.Atan,
        ["saturate"] = DxOpCode.Saturate,
        ["ddx"] = DxOpCode.DerivCoarseX,
        ["ddy"] = DxOpCode.DerivCoarseY,
        ["ddx_coarse"] = DxOpCode.DerivCoarseX,
        ["ddy_coarse"] = DxOpCode.DerivCoarseY,
        ["ddx_fine"] = DxOpCode.DerivFineX,
        ["ddy_fine"] = DxOpCode.DerivFineY,
    };

    private static readonly Dictionary<string, DxOpCode> Special = new(StringComparer.Ordinal)
    {
        ["isnan"] = DxOpCode.IsNaN,
        ["isinf"] = DxOpCode.IsInf,
        ["isfinite"] = DxOpCode.IsFinite,
    };

    private readonly LoweringContext _context;
    private readonly IntrinsicArguments _arguments;

    public MathIntrinsics(LoweringContext context)
    {
        _context = context;
        _arguments = new IntrinsicArguments(context);
    }

    private IrBuilder Builder => _context.Builder;

    private DxilModule Module => _context.Module;

    private DxOps Ops => _context.Ops;

    public void Register(Dictionary<string, Func<SyntaxNode, RValue>> handlers)
    {
        foreach (var (name, code) in FloatUnary)
        {
            handlers[name] = call => UnaryFloat(call, code);
        }

        foreach (var (name, code) in Special)
        {
            handlers[name] = call => FloatClass(call, code);
        }

        handlers["asuint"] = call => BitCast(call, ScalarKind.Uint);
        handlers["asint"] = call => BitCast(call, ScalarKind.Int);
        handlers["asfloat"] = call => BitCast(call, ScalarKind.Float);
        handlers["asdouble"] = AsDouble;
        handlers["select"] = Select;
        handlers["min"] = call => MinMax(call, max: false);

        handlers["max"] = call => MinMax(call, max: true);
        handlers["clamp"] = Clamp;
        handlers["abs"] = Abs;
        handlers["sign"] = Sign;
        RegisterMore(handlers);
    }

    private void RegisterMore(Dictionary<string, Func<SyntaxNode, RValue>> handlers)
    {
        handlers["rcp"] = call => FloatMap(call, values => Builder.Binary(BinaryOp.SDiv, Constant(values[0], 1), values[0]));
        handlers["exp"] = call => FloatMap(call, values => Unary(DxOpCode.Exp, Builder.Binary(BinaryOp.Mul, values[0], Constant(values[0], Log2E))));
        handlers["log"] = call => FloatMap(call, values => Builder.Binary(BinaryOp.Mul, Unary(DxOpCode.Log, values[0]), Constant(values[0], Ln2)));
        handlers["log10"] = call => FloatMap(call, values => Builder.Binary(BinaryOp.Mul, Unary(DxOpCode.Log, values[0]), Constant(values[0], Log10Of2)));
        handlers["pow"] = call => FloatMap(call, values => Unary(DxOpCode.Exp, Builder.Binary(BinaryOp.Mul, Unary(DxOpCode.Log, values[0]), values[1])));
        handlers["fmod"] = call => FloatMap(call, values => Builder.Binary(BinaryOp.SRem, values[0], values[1]));

        handlers["ldexp"] = call => FloatMap(call, values => Builder.Binary(BinaryOp.Mul, values[0], Unary(DxOpCode.Exp, values[1])));
        handlers["lerp"] = call => FloatMap(call, values => Add(values[0], Multiply(values[Third], Subtract(values[1], values[0]))));
        handlers["step"] = call => FloatMap(call, values => Step(values[0], values[1]));
        handlers["smoothstep"] = Smoothstep;
        handlers["atan2"] = call => FloatMap(call, values => new Atan2(Builder, Module, Ops).Lower(values[0], values[1]));
        handlers["mad"] = Mad;

        handlers["fma"] = call => Tertiary(call, DxOpCode.Fma, floating: true);
        handlers["countbits"] = call => Bits(call, DxOpCode.Countbits);
        handlers["firstbitlow"] = call => Bits(call, DxOpCode.FirstbitLo);
        handlers["firstbithigh"] = FirstBitHigh;
        handlers["reversebits"] = call => IntegerMap(call, values => Ops.Pure(DxOpCode.Bfrev, UnaryClass, values[0].Type, values[0]));
        handlers["f32tof16"] = F32ToF16;

        handlers["f16tof32"] = F16ToF32;
        new VectorIntrinsics(_context, _arguments).Register(handlers);
    }

    /// <summary>A float operation of the value's type.</summary>
    public Value Unary(DxOpCode code, Value value)
    {
        return Ops.Pure(code, UnaryClass, value.Type, value);
    }

    public Value Add(Value a, Value b)
    {
        return Builder.Binary(BinaryOp.Add, a, b);
    }

    public Value Subtract(Value a, Value b)
    {
        return Builder.Binary(BinaryOp.Sub, a, b);
    }

    public Value Multiply(Value a, Value b)
    {
        return Builder.Binary(BinaryOp.Mul, a, b);
    }

    /// <summary>A float constant of <paramref name="like"/>'s type.</summary>
    public Value Constant(Value like, double value)
    {
        var kind = Choice.Of(like.Type.Kind == IrTypeKind.Double, ScalarKind.Double, ScalarKind.Float);

        return _context.Convert.FloatConstant(kind, value);
    }

    private RValue FloatMap(SyntaxNode call, Func<Value[], Value> operation)
    {
        var (type, values) = _arguments.Common(call, floating: true);

        return IntrinsicArguments.Map(type, values, operation);
    }

    private RValue IntegerMap(SyntaxNode call, Func<Value[], Value> operation)
    {
        var (type, values) = _arguments.Common(call, floating: false);
        type = type.WithKind(Choice.Of(type.Kind == ScalarKind.Int, ScalarKind.Int, ScalarKind.Uint));
        values = [.. values.Select(value => _context.Convert.Convert(value, type))];

        return IntrinsicArguments.Map(type, values, operation);
    }

    private RValue UnaryFloat(SyntaxNode call, DxOpCode code)
    {
        return FloatMap(call, values => Unary(code, values[0]));
    }

    private RValue FloatClass(SyntaxNode call, DxOpCode code)
    {
        var (type, values) = _arguments.Common(call, floating: true);
        var result = IntrinsicArguments.Map(type, values, operands => Ops.Overloaded(code, SpecialClass, AttributeSet.ReadNone, operands[0].Type, Module.Types.Bool, operands[0]));

        return result with { Type = type.WithKind(ScalarKind.Bool) };
    }

    private RValue BitCast(SyntaxNode call, ScalarKind kind)
    {
        var value = _context.Expressions.Lower(call[0]);

        if (value.Type.Kind is ScalarKind.LiteralInt or ScalarKind.LiteralFloat or ScalarKind.Bool)
        {
            value = _context.Convert.Convert(value, value.Type.WithKind(IntrinsicArguments.Concrete(value.Type.Kind)));
        }

        var type = value.Type.WithKind(kind);
        var target = _context.Convert.IrTypeOf(kind);

        return new RValue(type, [.. value.Components.Select(component => Reinterpret(component, target))]);
    }

    private Value Reinterpret(Value value, IrType target)
    {
        if (value.Type == target)
        {
            return value;
        }

        if (value is Constant constant)
        {
            if (target.IsFloatingPoint)
            {
                return Module.FloatBits(target, constant.Bits);
            }

            return Module.Int(target, constant.Bits);
        }

        return Builder.Cast(CastOp.Bitcast, value, target);
    }

    private RValue AsDouble(SyntaxNode call)
    {
        var (type, values) = _arguments.Common(call, floating: false);
        var doubles = type.WithKind(ScalarKind.Double);

        return IntrinsicArguments.Map(doubles, values, operands => Ops.Overloaded(DxOpCode.MakeDouble, "makeDouble", AttributeSet.ReadNone, Module.Types.Double, Module.Types.Double, operands[0], operands[1]));
    }

    private RValue Select(SyntaxNode call)
    {
        var condition = _context.Expressions.Lower(call[0]);

        return _context.Expressions.Select(condition, _context.Expressions.Lower(call[1]), _context.Expressions.Lower(call[Third]));
    }

    private RValue MinMax(SyntaxNode call, bool max)
    {
        var (type, values) = _arguments.Common(call, floating: false);

        return IntrinsicArguments.Map(type, values, operands => MinMaxScalar(type.Kind, max, operands[0], operands[1]));
    }

    public Value MinMaxScalar(ScalarKind kind, bool max, Value a, Value b)
    {
        DxOpCode code;

        if (Conversions.IsFloat(kind))
        {
            code = Choice.Of(max, DxOpCode.FMax, DxOpCode.FMin);
        }
        else if (Conversions.IsSigned(kind))
        {
            code = Choice.Of(max, DxOpCode.IMax, DxOpCode.IMin);
        }
        else
        {
            code = Choice.Of(max, DxOpCode.UMax, DxOpCode.UMin);
        }

        return Ops.Pure(code, BinaryClass, a.Type, a, b);
    }

    private RValue Clamp(SyntaxNode call)
    {
        var (type, values) = _arguments.Common(call, floating: false);

        return IntrinsicArguments.Map(type, values, operands => MinMaxScalar(type.Kind, false, MinMaxScalar(type.Kind, true, operands[0], operands[1]), operands[Third]));
    }

    private RValue Abs(SyntaxNode call)
    {
        var (type, values) = _arguments.Common(call, floating: false);

        if (Conversions.IsFloat(type.Kind))
        {
            return IntrinsicArguments.Map(type, values, operands => Unary(DxOpCode.FAbs, operands[0]));
        }

        if (!Conversions.IsSigned(type.Kind))
        {
            return values[0];
        }

        return IntrinsicArguments.Map(type, values, operands => MinMaxScalar(type.Kind, true, operands[0], Builder.Binary(BinaryOp.Sub, Module.Int(operands[0].Type, 0), operands[0])));
    }

    /// <summary>sign: -1, 0 or 1 as int.</summary>
    private RValue Sign(SyntaxNode call)
    {
        var (type, values) = _arguments.Common(call, floating: false);
        var result = IntrinsicArguments.Map(type, values, operands => SignScalar(type.Kind, operands[0]));

        return result with { Type = type.WithKind(ScalarKind.Int) };
    }

    private Value SignScalar(ScalarKind kind, Value value)
    {
        var zero = _context.Convert.IntegerConstant(kind, 0);
        var operators = _context.Expressions.Operators;
        var positive = operators.Scalar(">", kind, value, zero);
        var negative = operators.Scalar("<", kind, value, zero);
        var up = Builder.Cast(CastOp.ZExt, positive, Module.Types.Int32);
        var down = Builder.Cast(CastOp.ZExt, negative, Module.Types.Int32);

        return Builder.Binary(BinaryOp.Sub, up, down);
    }

    private Value Step(Value edge, Value value)
    {
        var at = Builder.Compare(Predicate.FloatOrderedGreaterOrEqual, value, edge);

        return Builder.Select(at, Constant(value, 1), Constant(value, 0));
    }

    private RValue Smoothstep(SyntaxNode call)
    {
        return FloatMap(call, values =>
        {
            var range = Subtract(values[1], values[0]);
            var t = Unary(DxOpCode.Saturate, Builder.Binary(BinaryOp.SDiv, Subtract(values[Third], values[0]), range));
            var shape = Subtract(Constant(t, SmoothstepThree), Multiply(Constant(t, SmoothstepTwo), t));

            return Multiply(Multiply(t, t), shape);
        });
    }

    private RValue Mad(SyntaxNode call)
    {
        var (type, values) = _arguments.Common(call, floating: false);
        DxOpCode code;

        if (Conversions.IsFloat(type.Kind))
        {
            code = DxOpCode.FMad;
        }
        else
        {
            code = Choice.Of(Conversions.IsSigned(type.Kind), DxOpCode.IMad, DxOpCode.UMad);
        }

        return IntrinsicArguments.Map(type, values, operands => Ops.Pure(code, TertiaryClass, operands[0].Type, operands));
    }

    private RValue Tertiary(SyntaxNode call, DxOpCode code, bool floating)
    {
        var (type, values) = _arguments.Common(call, floating);

        return IntrinsicArguments.Map(type, values, operands => Ops.Pure(code, TertiaryClass, operands[0].Type, operands));
    }

    private RValue Bits(SyntaxNode call, DxOpCode code)
    {
        var result = IntrinsicMapUint(call, value => Ops.Overloaded(code, BitsClass, AttributeSet.ReadNone, value.Type, Module.Types.Int32, value));

        return result;
    }

    private RValue IntrinsicMapUint(SyntaxNode call, Func<Value, Value> operation)
    {
        var (type, values) = _arguments.Common(call, floating: false);
        var source = type.WithKind(Choice.Of(type.Kind == ScalarKind.Int, ScalarKind.Int, ScalarKind.Uint));
        var converted = _context.Convert.Convert(values[0], source);

        return new RValue(type.WithKind(ScalarKind.Uint), [.. converted.Components.Select(operation)]);
    }

    /// <summary>firstbithigh: DXIL counts from the top bit; HLSL numbers bits from the bottom (-1 stays -1).</summary>
    private RValue FirstBitHigh(SyntaxNode call)
    {
        var (type, _) = _arguments.Common(call, floating: false);
        var code = Choice.Of(type.Kind == ScalarKind.Int, DxOpCode.FirstbitSHi, DxOpCode.FirstbitHi);

        return IntrinsicMapUint(call, value =>
        {
            var fromTop = Ops.Overloaded(code, BitsClass, AttributeSet.ReadNone, value.Type, Module.Types.Int32, value);
            var none = Builder.Compare(Predicate.Equal, fromTop, Module.Int32(uint.MaxValue));
            var fromBottom = Builder.Binary(BinaryOp.Sub, Module.Int32(TopBit), fromTop);

            return Builder.Select(none, fromTop, fromBottom);
        });
    }

    private RValue F32ToF16(SyntaxNode call)
    {
        var (type, values) = _arguments.Common(call, floating: true);
        var result = IntrinsicArguments.Map(type, values, operands => Ops.Call(DxOpCode.LegacyF32ToF16, "legacyF32ToF16", AttributeSet.ReadNone, Module.Types.Int32, operands[0]));

        return result with { Type = type.WithKind(ScalarKind.Uint) };
    }

    private RValue F16ToF32(SyntaxNode call)
    {
        var value = _context.Expressions.Lower(call[0]);
        var type = value.Type.WithKind(ScalarKind.Uint);
        var bits = _context.Convert.Convert(value, type);
        var floats = bits.Components.Select(component => Ops.Call(DxOpCode.LegacyF16ToF32, "legacyF16ToF32", AttributeSet.ReadNone, Module.Types.Float, component));

        return new RValue(type.WithKind(ScalarKind.Float), [.. floats]);
    }
}
