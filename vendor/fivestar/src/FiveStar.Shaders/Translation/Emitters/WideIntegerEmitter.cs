// Copyright (c) 2026 Five Star Recomp contributors.
// SPDX-License-Identifier: GPL-3.0-or-later

using System.Text.RegularExpressions;
using FiveStar.Shaders.Translation.Core;
using static FiveStar.Shaders.Translation.Emitters.Operands;

namespace FiveStar.Shaders.Translation.Emitters;

/// <summary>
/// VALU arithmetic beyond one 32-bit result: add/sub with carry lanes, 16-bit integers (low half written, high half
/// kept), 64-bit integers and shifts, f64, three-operand adds, swaps and the cube-map helpers.
/// </summary>
internal sealed partial class WideIntegerEmitter
{
    private readonly TranslationState _state;
    private readonly OperandAccess _operands;

    public WideIntegerEmitter(TranslationState state, OperandAccess operands)
    {
        _state = state;
        _operands = operands;
    }

    /// <summary>Translates the instruction when it belongs to this family; false otherwise.</summary>
    public bool TryTranslate(string op, IReadOnlyList<Operand> a)
    {
        var carry = CarryPattern().Match(op);

        if (carry.Success)
        {
            AddWithCarry(carry.Groups[1].Value, carry.Groups["ci"].Success, a);

            return true;
        }

        var short16 = Integer16Pattern().Match(op);

        if (short16.Success)
        {
            Integer16(short16.Groups[1].Value, short16.Groups["type"].Value, a);

            return true;
        }

        var double64 = Double64Pattern().Match(op);

        if (double64.Success)
        {
            Double64(double64.Groups[1].Value, a);

            return true;
        }

        return TrySimple(op, a);
    }

    private bool TrySimple(string op, IReadOnlyList<Operand> a)
    {
        switch (op)
        {
            case "v_mul_hi_i32":
                Write(a, $"(uint)(((int64_t)asint({U(a, Source0)}) * (int64_t)asint({U(a, Source1)})) >> 32)");
                break;
            case "v_add3_u32":
                Write(a, $"{U(a, Source0)} + {U(a, Source1)} + {U(a, Source2)}");
                break;
            case "v_swap_b32":
                Swap(a);
                break;
            case "v_alignbyte_b32":
                Write(a, $"alignbit({U(a, Source0)}, {U(a, Source1)}, ({U(a, Source2)} & 3u) * 8u)");
                break;
            case "v_ffbh_i32":
                Write(a, $"flbit_signed({U(a, Source0)})");
                break;
            case "v_cvt_f64_f32":
                ConvertToDouble(a);
                break;
            case "v_cvt_f32_f64":
                Write(a, $"asuint((float)asdouble({_operands.Read(a[Source0], Low)}, {_operands.Read(a[Source0], High)}))");
                break;
            case "v_xad_u32":
                Write(a, $"({U(a, Source0)} ^ {U(a, Source1)}) + {U(a, Source2)}");
                break;
            default:
                return TryWide(op, a);
        }

        return true;
    }

    private bool TryWide(string op, IReadOnlyList<Operand> a)
    {
        switch (op)
        {
            case "v_mad_u64_u32":
                MultiplyAdd64(a);
                break;
            case "v_lshlrev_b64":
            case "v_lshrrev_b64":
            case "v_ashrrev_i64":
                Shift64(op, a);
                break;
            case "v_cubeid_f32":
            case "v_cubesc_f32":
            case "v_cubetc_f32":
            case "v_cubema_f32":
                Cube(op, a);
                break;
            default:
                return false;
        }

        return true;
    }

    /// <summary>v_add_co / v_sub_co / v_subrev_co (with _ci: plus carry-in); the carry-out is a lane mask.</summary>
    private void AddWithCarry(string kind, bool carryIn, IReadOnlyList<Operand> a)
    {
        var x = U(a, Source1);
        var y = U(a, Source2);
        var carryValue = carryIn ? $"({_operands.Mask(a[Source3])} ? 1u : 0u)" : "0u";
        var snapped = _operands.Snap(x, y, carryValue);
        (x, y) = (snapped[0], snapped[1]);
        var carry = snapped[^1];

        if (kind == "subrev")
        {
            (x, y) = (y, x);
        }

        string result;
        string carryOut;

        if (kind == "add")
        {
            result = $"{x} + {y} + {carry}";
            carryOut = $"((uint64_t){x} + (uint64_t){y} + {carry}) > 0xffffffffull";
        }
        else
        {
            result = $"{x} - {y} - {carry}";
            carryOut = $"((uint64_t){y} + {carry}) > (uint64_t){x}";
        }

        _operands.WriteMask(a[Source0], $"active && ({carryOut})");
        Write(a, result);
    }

    private void Integer16(string kind, string type, IReadOnlyList<Operand> a)
    {
        var x = $"({U(a, Source0)} & 0xffffu)";
        var y = $"({U(a, Source1)} & 0xffffu)";
        var unsigned = type == "u16";
        string value;

        switch (kind)
        {
            case "add":
                value = $"{x} + {y}";
                break;
            case "sub":
                value = $"{x} - {y}";
                break;
            case "subrev":
                value = $"{y} - {x}";
                break;
            case "mul_lo":
                value = $"{x} * {y}";
                break;
            default:
                value = Integer16Shift(kind, unsigned, x, y, a);
                break;
        }

        _operands.WriteHalf(a[Destination], value, high: false);
    }

    private string Integer16Shift(string kind, bool unsigned, string x, string y, IReadOnlyList<Operand> a)
    {
        switch (kind)
        {
            case "lshlrev":
                return $"{y} << ({x} & 15)";
            case "lshrrev":
                return $"{y} >> ({x} & 15)";
            case "ashrrev":
                return $"asuint({SignExtend16(U(a, Source1))} >> ({x} & 15))";
            default:
                return MinMax16(kind, unsigned, x, y, a);
        }
    }

    private string MinMax16(string kind, bool unsigned, string x, string y, IReadOnlyList<Operand> a)
    {
        if (unsigned)
        {
            return $"{kind}({x}, {y})";
        }

        return $"asuint({kind}({SignExtend16(U(a, Source0))}, {SignExtend16(U(a, Source1))}))";
    }

    private static string SignExtend16(string value)
    {
        return $"(asint({value} << 16) >> 16)";
    }

    private void Swap(IReadOnlyList<Operand> a)
    {
        var values = _operands.Snap(U(a, Destination), U(a, Source0));
        _operands.Write(a[Destination], values[1]);
        _operands.Write(a[Source0], values[0]);
    }

    private string DoubleValue(Operand operand)
    {
        string value;

        if (operand.IsImmediate)
        {
            value = $"(double){_operands.ReadFloat(operand)}";
        }
        else
        {
            value = $"asdouble({_operands.Read(operand, Low)}, {_operands.Read(operand, High)})";
        }

        return OperandAccess.ApplyModifiers(operand, value);
    }

    private void Double64(string kind, IReadOnlyList<Operand> a)
    {
        var x = DoubleValue(a[Source0]);
        var y = DoubleValue(a[Source1]);
        string value;

        switch (kind)
        {
            case "add":
                value = $"{x} + {y}";
                break;
            case "mul":
                value = $"{x} * {y}";
                break;
            case "min":
            case "max":
                value = $"{kind}({x}, {y})";
                break;
            default:
                value = $"fma({x}, {y}, {DoubleValue(a[Source2])})";
                break;
        }

        WriteDouble(a, value);
    }

    private void ConvertToDouble(IReadOnlyList<Operand> a)
    {
        var name = $"_d{_state.NextTemporary()}";
        _state.Emit($"  uint {name}lo, {name}hi; asuint((double){_operands.ReadFloat(a[Source0])}, {name}lo, {name}hi);");
        _operands.Write(a[Destination], $"{name}lo", Low);
        _operands.Write(a[Destination], $"{name}hi", High);
    }

    private void WriteDouble(IReadOnlyList<Operand> a, string value)
    {
        var name = $"_d{_state.NextTemporary()}";
        _state.Emit($"  uint {name}lo, {name}hi; asuint({value}, {name}lo, {name}hi);");
        _operands.Write(a[Destination], $"{name}lo", Low);
        _operands.Write(a[Destination], $"{name}hi", High);
    }

    /// <summary>v_mad_u64_u32: D.u64 = S0.u32 * S1.u32 + S2.u64; the carry-out mask is never set here.</summary>
    private void MultiplyAdd64(IReadOnlyList<Operand> a)
    {
        var destination = a[Destination];
        var addend = a[Source3];
        var values = _operands.Snap(U(a, Source1), U(a, Source2), _operands.Read(addend, Low), _operands.Read(addend, High));
        var (x, y, low, high) = (values[0], values[1], values[^PairDwords], values[^1]);

        _state.Emit($"  {{ uint64_t _p = (uint64_t){x} * (uint64_t){y} + ((uint64_t){high} << 32 | {low});");
        _state.Emit($"    if (active) {{ {destination.At(Low).Name} = (uint)_p; {destination.At(High).Name} = (uint)(_p >> 32); }} }}");
        _state.Touch(destination.At(Low));
        _state.Touch(destination.At(High));
        _operands.WriteMask(a[Source0], "false");
    }

    private void Shift64(string op, IReadOnlyList<Operand> a)
    {
        var destination = a[Destination];
        var values = _operands.Snap(U(a, Source0), _operands.Read(a[Source1], Low), _operands.Read(a[Source1], High));
        var (amount, low, high) = (values[0], values[1], values[^1]);
        var symbol = op.Contains("lshl", StringComparison.Ordinal) ? "<<" : ">>";
        var type = op.Contains("ashr", StringComparison.Ordinal) ? "int64_t" : "uint64_t";

        _state.Emit($"  {{ {type} _v = ({type})((uint64_t){high} << 32 | {low}) {symbol} ({amount} & 63);");
        _state.Emit($"    if (active) {{ {destination.At(Low).Name} = (uint)_v; {destination.At(High).Name} = (uint)((uint64_t)_v >> 32); }} }}");
        _state.Touch(destination.At(Low));
        _state.Touch(destination.At(High));
    }

    private void Cube(string op, IReadOnlyList<Operand> a)
    {
        // v_cubeid_f32 -> cubeid(...): the prelude implements the AMD face conventions.
        var function = op["v_".Length..op.IndexOf("_f32", StringComparison.Ordinal)];
        var x = _operands.ReadFloat(a[Source0]);
        var y = _operands.ReadFloat(a[Source1]);
        Write(a, $"asuint({function}({x}, {y}, {_operands.ReadFloat(a[Source2])}))");
    }

    private string U(IReadOnlyList<Operand> a, int position)
    {
        return _operands.Read(a[position]);
    }

    private void Write(IReadOnlyList<Operand> a, string value)
    {
        _operands.Write(a[Destination], value);
    }

    [GeneratedRegex(@"^v_(add|sub|subrev)_co(?<ci>_ci)?_u32$")]
    private static partial Regex CarryPattern();

    [GeneratedRegex(@"^v_(add|sub|subrev|mul_lo|lshlrev|lshrrev|ashrrev|max|min)_(?:nc_)?(?<type>u16|i16|b16)$")]
    private static partial Regex Integer16Pattern();

    [GeneratedRegex(@"^v_(add|mul|fma|min|max)_f64$")]
    private static partial Regex Double64Pattern();
}
