// Copyright (c) 2026 Five Star Recomp contributors.
// SPDX-License-Identifier: GPL-3.0-or-later

using FiveStar.Shaders.Dxil.Ir;

namespace FiveStar.Shaders.Dxil.Lowering;

/// <summary>
/// HLSL conversions: scalar kinds (constants are folded), splats of a scalar to a vector, truncation of a vector, and
/// the usual arithmetic conversions that give two operands one type.
/// </summary>
public sealed class Conversions
{
    private static readonly ScalarKind[] RankOrder =
    [
        ScalarKind.Bool, ScalarKind.Int, ScalarKind.Uint, ScalarKind.Int64, ScalarKind.Uint64, ScalarKind.Float, ScalarKind.Double,
    ];

    private readonly IrBuilder _builder;

    public Conversions(IrBuilder builder)
    {
        _builder = builder;
    }

    private DxilModule Module => _builder.Module;

    private TypeTable Types => _builder.Types;

    public static bool IsFloat(ScalarKind kind)
    {
        return kind is ScalarKind.Float or ScalarKind.Double or ScalarKind.LiteralFloat;
    }

    public static bool IsSigned(ScalarKind kind)
    {
        return kind is ScalarKind.Int or ScalarKind.Int64 or ScalarKind.LiteralInt;
    }

    public static bool IsWide(ScalarKind kind)
    {
        return kind is ScalarKind.Int64 or ScalarKind.Uint64 or ScalarKind.Double;
    }

    /// <summary>The LLVM type of a scalar kind (literals as i64 and double).</summary>
    public IrType IrTypeOf(ScalarKind kind)
    {
        return kind switch
        {
            ScalarKind.Bool => Types.Bool,
            ScalarKind.Int or ScalarKind.Uint => Types.Int32,
            ScalarKind.Float => Types.Float,
            ScalarKind.Double or ScalarKind.LiteralFloat => Types.Double,
            _ => Types.Int64,
        };
    }

    /// <summary>The kind both operands of an arithmetic operator become.</summary>
    public static ScalarKind Common(ScalarKind left, ScalarKind right)
    {
        if (left == right)
        {
            return left;
        }

        if (left is ScalarKind.LiteralInt or ScalarKind.LiteralFloat)
        {
            return LiteralWith(left, right);
        }

        if (right is ScalarKind.LiteralInt or ScalarKind.LiteralFloat)
        {
            return LiteralWith(right, left);
        }

        return RankKind(Math.Max(Rank(left), Rank(right)));
    }

    /// <summary>The shape both operands of a componentwise operator take: the vector size, or the smaller of two.</summary>
    public static HlslType CommonShape(HlslType left, HlslType right, ScalarKind kind)
    {
        if (left.Shape == TypeShape.Matrix || right.Shape == TypeShape.Matrix)
        {
            var matrix = left.Shape == TypeShape.Matrix ? left : right;

            return HlslType.Matrix(kind, matrix.Rows, matrix.Columns);
        }

        var size = Math.Max(left.Count, right.Count);

        if (left.Count > 1 && right.Count > 1)
        {
            size = Math.Min(left.Count, right.Count);
        }

        return HlslType.Vector(kind, size);
    }

    /// <summary>Converts <paramref name="value"/> to <paramref name="target"/>: splat, truncation and scalar conversion.</summary>
    public RValue Convert(RValue value, HlslType target)
    {
        if (value.Type.SameAs(target) || target.Shape == TypeShape.Resource)
        {
            return value;
        }

        if (target.Shape is TypeShape.Struct or TypeShape.Array)
        {
            return ConvertAggregate(value, target);
        }

        var components = new Value[target.Count];

        for (var index = 0; index < target.Count; index++)
        {
            var source = value.Components.Count == 1 ? 0 : index;

            if (source >= value.Components.Count)
            {
                throw new DxilBackendException($"cannot convert {value.Type} to {target}");
            }

            components[index] = Scalar(value.Components[source], value.Type.ComponentKind(source), target.ComponentKind(index));
        }

        return new RValue(target, components);
    }

    /// <summary>A struct or array from a value of the same layout, or from one scalar (<c>(Exp)0</c>).</summary>
    private RValue ConvertAggregate(RValue value, HlslType target)
    {
        if (value.Components.Count != 1 && value.Components.Count != target.Count)
        {
            throw new DxilBackendException($"cannot convert {value.Type} to {target}");
        }

        var components = new Value[target.Count];

        for (var index = 0; index < target.Count; index++)
        {
            var source = value.Components.Count == 1 ? 0 : index;
            components[index] = Scalar(value.Components[source], value.Type.ComponentKind(source), target.ComponentKind(index));
        }

        return new RValue(target, components);
    }

    public Value Scalar(Value value, ScalarKind from, ScalarKind to)
    {
        var result = Fold(value, from, to);

        if (result is not null)
        {
            return result;
        }

        if (from == to || (IrTypeOf(from) == IrTypeOf(to) && !IsLiteral(from)))
        {
            return value;
        }

        if (to == ScalarKind.Bool)
        {
            return ToBool(value, from);
        }

        if (from == ScalarKind.Bool)
        {
            return FromBool(value, to);
        }

        return Numeric(value, from, to);
    }

    /// <summary>A constant of <paramref name="kind"/> from an exact value.</summary>
    public Value Constant(ScalarKind kind, double value)
    {
        if (IsFloat(kind))
        {
            return FloatConstant(kind, value);
        }

        if (kind == ScalarKind.Bool)
        {
            return Module.Bool(value != 0);
        }

        return Module.Int(IrTypeOf(kind), (long)value);
    }

    public Value IntegerConstant(ScalarKind kind, long value)
    {
        if (IsFloat(kind))
        {
            return FloatConstant(kind, value);
        }

        if (kind == ScalarKind.Bool)
        {
            return Module.Bool(value != 0);
        }

        return Module.Int(IrTypeOf(kind), value);
    }

    public Value FloatConstant(ScalarKind kind, double value)
    {
        if (kind == ScalarKind.Float)
        {
            return Module.Float((float)value);
        }

        return Module.FloatBits(Types.Double, BitConverter.DoubleToInt64Bits(value));
    }

    private static bool IsLiteral(ScalarKind kind)
    {
        return kind is ScalarKind.LiteralInt or ScalarKind.LiteralFloat;
    }

    private static ScalarKind LiteralWith(ScalarKind literal, ScalarKind other)
    {
        if (literal == ScalarKind.LiteralFloat && !IsFloat(other))
        {
            return ScalarKind.Float;
        }

        if (other == ScalarKind.Bool && literal == ScalarKind.LiteralInt)
        {
            return ScalarKind.Int;
        }

        if (other == ScalarKind.Bool)
        {
            return ScalarKind.Float;
        }

        return other;
    }

    private static int Rank(ScalarKind kind)
    {
        var rank = Array.IndexOf(RankOrder, kind);

        if (rank < 0)
        {
            return RankOrder.Length - 1;
        }

        return rank;
    }

    private static ScalarKind RankKind(int rank)
    {
        return RankOrder[rank];
    }

    /// <summary>Converts a constant at compile time; null when <paramref name="value"/> is not a constant.</summary>
    private Value? Fold(Value value, ScalarKind from, ScalarKind to)
    {
        if (value is not Constant constant || constant.Kind is not (ConstantKind.Integer or ConstantKind.FloatingPoint))
        {
            return null;
        }

        if (IsFloat(from))
        {
            var real = FloatValue(constant);

            if (IsFloat(to))
            {
                return FloatConstant(to, real);
            }

            if (to == ScalarKind.Bool)
            {
                return Module.Bool(real != 0);
            }

            return IntegerConstant(to, Truncate(real, to));
        }

        var integer = IntegerValue(constant, from);

        if (IsFloat(to))
        {
            return FloatConstant(to, ToDouble(integer, from));
        }

        return IntegerConstant(to, integer);
    }

    private static long Truncate(double real, ScalarKind to)
    {
        if (double.IsNaN(real))
        {
            return 0;
        }

        if (IsSigned(to))
        {
            return (long)Math.Clamp(real, long.MinValue, long.MaxValue);
        }

        return (long)(ulong)Math.Clamp(real, 0, ulong.MaxValue);
    }

    private static double ToDouble(long integer, ScalarKind from)
    {
        if (IsSigned(from) || from == ScalarKind.Bool)
        {
            return integer;
        }

        if (from == ScalarKind.Uint)
        {
            return (uint)integer;
        }

        return (ulong)integer;
    }

    private static double FloatValue(Constant constant)
    {
        if (constant.Type.Kind == IrTypeKind.Float)
        {
            return BitConverter.Int32BitsToSingle((int)constant.Bits);
        }

        return BitConverter.Int64BitsToDouble(constant.Bits);
    }

    /// <summary>An integer constant's value as its kind reads it (unsigned 32-bit values zero-extended).</summary>
    private static long IntegerValue(Constant constant, ScalarKind from)
    {
        if (from == ScalarKind.Uint || from == ScalarKind.Bool)
        {
            return (long)constant.Unsigned;
        }

        return constant.Bits;
    }

    private Value ToBool(Value value, ScalarKind from)
    {
        if (IsFloat(from))
        {
            return _builder.Compare(Predicate.FloatUnorderedNotEqual, value, FloatConstant(from, 0));
        }

        return _builder.Compare(Predicate.NotEqual, value, Module.Int(value.Type, 0));
    }

    private Value FromBool(Value value, ScalarKind to)
    {
        if (IsFloat(to))
        {
            return _builder.Cast(CastOp.UiToFp, value, IrTypeOf(to));
        }

        return _builder.Cast(CastOp.ZExt, value, IrTypeOf(to));
    }

    private Value Numeric(Value value, ScalarKind from, ScalarKind to)
    {
        var target = IrTypeOf(to);

        if (IsFloat(from) && IsFloat(to))
        {
            var resize = IsWide(to) ? CastOp.FpExt : CastOp.FpTrunc;

            return _builder.Cast(resize, value, target);
        }

        if (IsFloat(from))
        {
            var toInteger = IsSigned(to) ? CastOp.FpToSi : CastOp.FpToUi;

            return _builder.Cast(toInteger, value, target);
        }

        if (IsFloat(to))
        {
            var toFloat = IsSigned(from) ? CastOp.SiToFp : CastOp.UiToFp;

            return _builder.Cast(toFloat, value, target);
        }

        var fromBits = value.Type.Bits;
        var toBits = target.Bits;

        if (fromBits == toBits)
        {
            return value;
        }

        if (fromBits > toBits)
        {
            return _builder.Cast(CastOp.Trunc, value, target);
        }

        var extend = IsSigned(from) ? CastOp.SExt : CastOp.ZExt;

        return _builder.Cast(extend, value, target);
    }
}
