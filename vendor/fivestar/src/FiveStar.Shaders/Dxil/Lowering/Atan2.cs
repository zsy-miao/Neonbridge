// Copyright (c) 2026 Five Star Recomp contributors.
// SPDX-License-Identifier: GPL-3.0-or-later

using FiveStar.Shaders.Dxil.Ir;
using FiveStar.Shaders.Dxil.Ops;

namespace FiveStar.Shaders.Dxil.Lowering;

/// <summary>atan2(y, x) as dxc lowers it: atan(y / x), moved by ±π for x &lt; 0 and set to ±π/2 for x = 0.</summary>
public sealed class Atan2
{
    private const double HalfPi = Math.PI / 2;

    private readonly IrBuilder _builder;
    private readonly DxilModule _module;
    private readonly DxOps _ops;

    public Atan2(IrBuilder builder, DxilModule module, DxOps ops)
    {
        _builder = builder;
        _module = module;
        _ops = ops;
    }

    public Value Lower(Value y, Value x)
    {
        var type = y.Type;
        var zero = Constant(type, 0);
        var atan = _ops.Pure(DxOpCode.Atan, "unary", type, _builder.Binary(BinaryOp.SDiv, y, x));
        var xNegative = _builder.Compare(Predicate.FloatOrderedLess, x, zero);
        var xZero = _builder.Compare(Predicate.FloatOrderedEqual, x, zero);
        var yNonNegative = _builder.Compare(Predicate.FloatOrderedGreaterOrEqual, y, zero);

        var yNegative = _builder.Compare(Predicate.FloatOrderedLess, y, zero);
        var result = atan;
        result = _builder.Select(_builder.Binary(BinaryOp.And, xNegative, yNonNegative), _builder.Binary(BinaryOp.Add, atan, Constant(type, Math.PI)), result);
        result = _builder.Select(_builder.Binary(BinaryOp.And, xNegative, yNegative), _builder.Binary(BinaryOp.Sub, atan, Constant(type, Math.PI)), result);
        result = _builder.Select(_builder.Binary(BinaryOp.And, xZero, yNegative), Constant(type, -HalfPi), result);

        return _builder.Select(_builder.Binary(BinaryOp.And, xZero, yNonNegative), Constant(type, HalfPi), result);
    }

    private Value Constant(IrType type, double value)
    {
        if (type.Kind == IrTypeKind.Double)
        {
            return _module.FloatBits(type, BitConverter.DoubleToInt64Bits(value));
        }

        return _module.Float((float)value);
    }
}
