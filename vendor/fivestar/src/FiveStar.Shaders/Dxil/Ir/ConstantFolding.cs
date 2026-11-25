// Copyright (c) 2026 Five Star Recomp contributors.
// SPDX-License-Identifier: GPL-3.0-or-later

namespace FiveStar.Shaders.Dxil.Ir;

/// <summary>
/// What <see cref="IrBuilder"/> writes instead of an instruction whose result is known while lowering: integer
/// operations, comparisons and casts of constants, a comparison of a value with itself, a select on a constant
/// condition or between one value, and the integer identities (x + 0, x | 0, x ^ 0, x - 0, x * 1, x &amp; ~0, shifts by
/// 0, x &amp; 0, x * 0). The translator's address and range arithmetic is full of them (a cbuffer row index 5 &gt;&gt; 2,
/// a mask of 0xffffffff): about a quarter of a large program's instructions. Floating-point operations are left alone:
/// x + 0.0 is not x for -0.0.
/// </summary>
internal static class ConstantFolding
{
    private const int LongBits = 64;

    /// <summary>The value of <c>left op right</c> when it is known; null otherwise.</summary>
    public static Value? Binary(DxilModule module, BinaryOp op, Value left, Value right)
    {
        if (!left.Type.IsInteger)
        {
            return null;
        }

        var a = IntegerConstant(left);
        var b = IntegerConstant(right);

        if (a is not null && b is not null)
        {
            return Fold(module, op, a, b);
        }

        if (b is not null)
        {
            return RightIdentity(module, op, left, b);
        }

        if (a is not null && op is BinaryOp.Add or BinaryOp.Or or BinaryOp.Xor or BinaryOp.Mul or BinaryOp.And)
        {
            return RightIdentity(module, op, right, a);
        }

        return null;
    }

    /// <summary>The value of an integer comparison of two constants or of a value with itself; null otherwise.</summary>
    public static Value? Compare(DxilModule module, Predicate predicate, Value left, Value right)
    {
        if (ReferenceEquals(left, right) && left.Type.IsInteger)
        {
            return module.Bool(predicate is Predicate.Equal or Predicate.UnsignedGreaterOrEqual or Predicate.UnsignedLessOrEqual
                or Predicate.SignedGreaterOrEqual or Predicate.SignedLessOrEqual);
        }

        var a = IntegerConstant(left);
        var b = IntegerConstant(right);

        if (a is null || b is null)
        {
            return null;
        }

        bool? result = predicate switch
        {
            Predicate.Equal => a.Bits == b.Bits,
            Predicate.NotEqual => a.Bits != b.Bits,
            Predicate.UnsignedGreater => a.Unsigned > b.Unsigned,
            Predicate.UnsignedGreaterOrEqual => a.Unsigned >= b.Unsigned,
            Predicate.UnsignedLess => a.Unsigned < b.Unsigned,
            Predicate.UnsignedLessOrEqual => a.Unsigned <= b.Unsigned,
            Predicate.SignedGreater => a.Bits > b.Bits,
            Predicate.SignedGreaterOrEqual => a.Bits >= b.Bits,
            Predicate.SignedLess => a.Bits < b.Bits,
            Predicate.SignedLessOrEqual => a.Bits <= b.Bits,
            _ => null,
        };

        if (result is null)
        {
            return null;
        }

        return module.Bool(result.Value);
    }

    /// <summary>An integer-to-integer cast of a constant; null otherwise.</summary>
    public static Value? Cast(DxilModule module, CastOp op, Value value, IrType to)
    {
        var constant = IntegerConstant(value);

        if (constant is null || !to.IsInteger)
        {
            return null;
        }

        return op switch
        {
            CastOp.Trunc or CastOp.SExt => module.Int(to, constant.Bits),
            CastOp.ZExt => module.Int(to, (long)constant.Unsigned),
            CastOp.Bitcast when to.Bits == constant.Type.Bits => constant,
            _ => null,
        };
    }

    /// <summary>The chosen operand of a select whose condition is constant or whose operands are the same.</summary>
    public static Value? Select(Value condition, Value whenTrue, Value whenFalse)
    {
        if (ReferenceEquals(whenTrue, whenFalse))
        {
            return whenTrue;
        }

        var constant = IntegerConstant(condition);

        if (constant is null)
        {
            return null;
        }

        if (constant.Bits != 0)
        {
            return whenTrue;
        }

        return whenFalse;
    }

    private static Constant? IntegerConstant(Value value)
    {
        var constant = value as Constant;

        if (constant is null || !constant.IsInteger)
        {
            return null;
        }

        return constant;
    }

    private static Value? Fold(DxilModule module, BinaryOp op, Constant a, Constant b)
    {
        var bits = a.Type.Bits;
        var shift = (int)b.Unsigned;
        bool shiftable = b.Unsigned < (ulong)bits;

        long? result = op switch
        {
            BinaryOp.Add => a.Bits + b.Bits,
            BinaryOp.Sub => a.Bits - b.Bits,
            BinaryOp.Mul => a.Bits * b.Bits,
            BinaryOp.And => a.Bits & b.Bits,
            BinaryOp.Or => a.Bits | b.Bits,
            BinaryOp.Xor => a.Bits ^ b.Bits,
            BinaryOp.Shl when shiftable => a.Bits << shift,
            BinaryOp.LShr when shiftable => (long)(a.Unsigned >> shift),
            BinaryOp.AShr when shiftable => a.Bits >> shift,
            BinaryOp.UDiv when b.Unsigned != 0 => (long)(a.Unsigned / b.Unsigned),
            BinaryOp.URem when b.Unsigned != 0 => (long)(a.Unsigned % b.Unsigned),
            _ => null,
        };

        if (result is null || bits > LongBits)
        {
            return null;
        }

        return module.Int(a.Type, result.Value);
    }

    /// <summary><c>value op constant</c> when it is <paramref name="value"/> itself or a constant.</summary>
    private static Value? RightIdentity(DxilModule module, BinaryOp op, Value value, Constant constant)
    {
        bool zero = constant.Bits == 0;
        bool one = constant.Bits == 1;
        bool allOnes = constant.Bits == -1;

        return op switch
        {
            BinaryOp.Add or BinaryOp.Sub or BinaryOp.Or or BinaryOp.Xor or BinaryOp.Shl or BinaryOp.LShr or BinaryOp.AShr when zero => value,
            BinaryOp.Mul or BinaryOp.UDiv or BinaryOp.SDiv when one => value,
            BinaryOp.And when allOnes => value,
            BinaryOp.And or BinaryOp.Mul when zero => module.Int(value.Type, 0),
            _ => null,
        };
    }
}
