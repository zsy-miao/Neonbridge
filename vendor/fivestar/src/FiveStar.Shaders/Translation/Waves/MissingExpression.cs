// Copyright (c) 2026 Five Star Recomp contributors.
// SPDX-License-Identifier: GPL-3.0-or-later

using System.Globalization;
using System.Numerics;
using System.Text.RegularExpressions;

namespace FiveStar.Shaders.Translation.Waves;

/// <summary>
/// Evaluates a translated HLSL expression on a console lane without a pixel or vertex (<see cref="MissingValue"/>).
/// Integer and bool operations on known values are computed as HLSL does; float values are known only through bit
/// casts, selects, sign changes and exact comparisons, since dxc may contract float arithmetic. Anything not understood
/// is unknown.
/// </summary>
internal sealed partial class MissingExpression
{
    private const int LogicalOr = 1;
    private const int LogicalAnd = 2;
    private const int BitOr = 3;
    private const int BitXor = 4;
    private const int BitAnd = 5;
    private const int Equality = 6;
    private const int Relation = 7;
    private const int Shift = 8;
    private const int Additive = 9;
    private const int Multiplicative = 10;

    private const int PairArguments = 2;
    private const int SelectArguments = 3;
    private const int ShiftMask = 31;
    private const uint SignBit = 0x80000000u;
    private const uint NoBit = uint.MaxValue;

    private static readonly (string Operator, int Precedence)[] Operators =
    [
        ("||", LogicalOr), ("&&", LogicalAnd), ("==", Equality), ("!=", Equality), ("<=", Relation), (">=", Relation),
        ("<<", Shift), (">>", Shift), ("|", BitOr), ("^", BitXor), ("&", BitAnd), ("<", Relation), (">", Relation),
        ("+", Additive), ("-", Additive), ("*", Multiplicative), ("/", Multiplicative), ("%", Multiplicative),
    ];

    private readonly string _text;
    private readonly Func<string, MissingValue> _variable;
    private readonly Func<string, string, MissingValue> _read;
    private int _position;

    private MissingExpression(string text, Func<string, MissingValue> variable, Func<string, string, MissingValue> read)
    {
        _text = text;
        _variable = variable;
        _read = read;
    }

    /// <summary>
    /// The value of <paramref name="text"/>: <paramref name="variable"/> gives each variable's, <paramref name="read"/> the
    /// result of <c>vw_read(x, lane)</c> from the text of x and lane.
    /// </summary>
    public static MissingValue Evaluate(string text, Func<string, MissingValue> variable, Func<string, string, MissingValue> read)
    {
        var expression = new MissingExpression(text, variable, read);

        try
        {
            var value = expression.Ternary();
            expression.SkipSpaces();

            if (expression._position != text.Length)
            {
                return MissingValue.Unknown;
            }

            return value;
        }
        catch (Exception exception) when (exception is FormatException or OverflowException)
        {
            return MissingValue.Unknown;
        }
    }

    /// <summary>The value converted to an HLSL scalar type, as an assignment or cast does; unknown where that is not exact.</summary>
    public static MissingValue Convert(MissingValue value, MissingType type)
    {
        if (!value.IsKnown || value.Type == type)
        {
            return value;
        }

        if (type == MissingType.Bool)
        {
            return Truth(value) switch
            {
                bool truth => MissingValue.Bool(truth),
                _ => MissingValue.Unknown,
            };
        }

        if (value.Type == MissingType.Float)
        {
            return FromFloat(BitConverter.UInt32BitsToSingle(value.Bits), type);
        }

        if (type == MissingType.Float)
        {
            return MissingValue.Of(BitConverter.SingleToUInt32Bits((float)Number(value)), MissingType.Float);
        }

        return MissingValue.Of(value.Bits, type);
    }

    private MissingValue Ternary()
    {
        var condition = Binary(LogicalOr);

        if (!Take("?"))
        {
            return condition;
        }

        var whenTrue = Ternary();
        Expect(":");
        var whenFalse = Ternary();

        return Choose(condition, whenTrue, whenFalse);
    }

    private MissingValue Binary(int precedence)
    {
        var left = Unary();

        for (;;)
        {
            var next = PeekOperator();

            if (next is null || next.Value.Precedence < precedence)
            {
                return left;
            }

            _position += next.Value.Operator.Length;
            var right = Binary(next.Value.Precedence + 1);
            left = Apply(next.Value.Operator, left, right);
        }
    }

    private MissingValue Unary()
    {
        SkipSpaces();

        if (Take("!"))
        {
            return Not(Unary());
        }

        if (Take("~"))
        {
            return Integer(Unary(), bits => ~bits);
        }

        if (Take("-"))
        {
            return Negate(Unary());
        }

        if (Take("+"))
        {
            return Unary();
        }

        var cast = CastPattern().Match(_text, _position);

        if (cast.Success)
        {
            _position += cast.Length;

            return Cast(cast.Groups["type"].Value, Unary());
        }

        if (Take("("))
        {
            var inner = Ternary();
            Expect(")");

            return Postfix(inner);
        }

        return Postfix(Primary());
    }

    private MissingValue Primary()
    {
        var number = NumberPattern().Match(_text, _position);

        if (number.Success)
        {
            _position += number.Length;

            return Literal(number.Value);
        }

        var name = NamePattern().Match(_text, _position);

        if (!name.Success)
        {
            throw new FormatException(_text);
        }

        _position += name.Length;
        SkipSpaces();

        if (!Take("("))
        {
            return Name(name.Value);
        }

        return Call(name.Value, Arguments());
    }

    /// <summary>The arguments of a call after its opening parenthesis: each value with its text.</summary>
    private List<(MissingValue Value, string Text)> Arguments()
    {
        var arguments = new List<(MissingValue Value, string Text)>();
        SkipSpaces();

        if (Take(")"))
        {
            return arguments;
        }

        for (;;)
        {
            var start = _position;
            var value = Ternary();
            arguments.Add((value, _text[start.._position].Trim()));

            if (Take(")"))
            {
                return arguments;
            }

            Expect(",");
        }
    }

    private MissingValue Postfix(MissingValue value)
    {
        for (;;)
        {
            SkipSpaces();

            if (Take("."))
            {
                // A member of a vector or a method: not tracked.
                Primary();
                value = MissingValue.Unknown;
            }
            else if (Take("["))
            {
                Ternary();
                Expect("]");
                value = MissingValue.Unknown;
            }
            else
            {
                return value;
            }
        }
    }

    private MissingValue Call(string function, List<(MissingValue Value, string Text)> arguments)
    {
        if (arguments.Count == 0)
        {
            return MissingValue.Unknown;
        }

        var first = arguments[0].Value;
        var last = arguments[^1].Value;

        switch (function)
        {
            case "select" when arguments.Count == SelectArguments:
                return Choose(first, arguments[1].Value, last);
            case "vw_read" when arguments.Count == PairArguments:
                return _read(arguments[0].Text, arguments[1].Text);
            case "quad_perm":
                return first;
            case "asuint":
                return Reinterpret(first, MissingType.UInt);
            case "asint":
                return Reinterpret(first, MissingType.Int);
            case "asfloat":
                return Reinterpret(first, MissingType.Float);
            case "min" when arguments.Count == PairArguments:
                return Extreme(first, last, maximum: false);
            case "max" when arguments.Count == PairArguments:
                return Extreme(first, last, maximum: true);
            case "countbits":
                return Integer(first, bits => (uint)BitOperations.PopCount(bits));
            case "firstbitlow":
                return Integer(first, FirstBitLow);
            default:
                return Zeroes(function, first);
        }
    }

    /// <summary>Functions that keep 0 as 0 (and only that is certain of them).</summary>
    private static MissingValue Zeroes(string function, MissingValue value)
    {
        string[] keeping = ["abs", "reversebits", "f32tof16", "f32tof16_rne", "f16tof32"];

        if (keeping.Contains(function) && value.IsKnown && value.Bits == 0)
        {
            return value;
        }

        return MissingValue.Unknown;
    }

    private MissingValue Name(string name)
    {
        if (name == "true")
        {
            return MissingValue.Bool(true);
        }

        if (name == "false")
        {
            return MissingValue.Bool(false);
        }

        return _variable(name);
    }

    private static MissingValue Literal(string text)
    {
        var unsigned = text.EndsWith('u') || text.EndsWith('U');
        var type = MissingType.Int;

        if (unsigned)
        {
            type = MissingType.UInt;
        }

        const string HexPrefix = "0x";

        if (text.StartsWith(HexPrefix, StringComparison.OrdinalIgnoreCase))
        {
            var hex = text[HexPrefix.Length..].TrimEnd('u', 'U', 'l', 'L');

            return MissingValue.Of(uint.Parse(hex, NumberStyles.HexNumber, CultureInfo.InvariantCulture), type);
        }

        var digits = text.TrimEnd('u', 'U', 'f', 'F', 'l', 'L', 'h', 'H');
        var isFloat = digits.Contains('.', StringComparison.Ordinal) || digits.Contains('e', StringComparison.OrdinalIgnoreCase) || digits.Length < text.TrimEnd('u', 'U', 'l', 'L').Length;

        if (isFloat)
        {
            var value = (float)double.Parse(digits, CultureInfo.InvariantCulture);

            return MissingValue.Of(BitConverter.SingleToUInt32Bits(value), MissingType.Float);
        }

        if (!ulong.TryParse(digits, CultureInfo.InvariantCulture, out var whole) || whole > uint.MaxValue)
        {
            return MissingValue.Unknown;
        }

        return MissingValue.Of((uint)whole, type);
    }

    private static MissingValue Apply(string operation, MissingValue left, MissingValue right)
    {
        switch (operation)
        {
            case "||":
                return Not(Logical(Not(left), Not(right)));
            case "&&":
                return Logical(left, right);
            case "==":
            case "!=":
            case "<":
            case ">":
            case "<=":
            case ">=":
                return Compare(operation, left, right);
            default:
                return Arithmetic(operation, left, right);
        }
    }

    /// <summary><c>a &amp;&amp; b</c>: false as soon as either side is known false.</summary>
    private static MissingValue Logical(MissingValue left, MissingValue right)
    {
        var (first, second) = (Truth(left), Truth(right));

        if (first == false || second == false)
        {
            return MissingValue.Bool(false);
        }

        if (first == true && second == true)
        {
            return MissingValue.Bool(true);
        }

        return MissingValue.Unknown;
    }

    private static MissingValue Not(MissingValue value)
    {
        return Truth(value) switch
        {
            bool truth => MissingValue.Bool(!truth),
            _ => MissingValue.Unknown,
        };
    }

    private static MissingValue Compare(string operation, MissingValue left, MissingValue right)
    {
        if (operation is "==" or "!=" && (OutOfRange(left, right) || OutOfRange(right, left)))
        {
            return MissingValue.Bool(operation == "!=");
        }

        if (!left.IsKnown || !right.IsKnown)
        {
            return MissingValue.Unknown;
        }

        int order;

        if (left.Type == MissingType.Float || right.Type == MissingType.Float)
        {
            var (a, b) = (Number(left), Number(right));

            if (double.IsNaN(a) || double.IsNaN(b))
            {
                return MissingValue.Bool(operation == "!=");
            }

            order = a.CompareTo(b);
        }
        else if (left.Type == MissingType.UInt || right.Type == MissingType.UInt)
        {
            order = left.Bits.CompareTo(right.Bits);
        }
        else
        {
            order = ((int)left.Bits).CompareTo((int)right.Bits);
        }

        var result = operation switch
        {
            "==" => order == 0,
            "!=" => order != 0,
            "<" => order < 0,
            ">" => order > 0,
            "<=" => order <= 0,
            _ => order >= 0,
        };

        return MissingValue.Bool(result);
    }

    /// <summary>Integer operations on known values; float arithmetic is not followed (dxc may contract it).</summary>
    private static MissingValue Arithmetic(string operation, MissingValue left, MissingValue right)
    {
        if (operation == "&" && (IsIntegerZero(left) || IsIntegerZero(right)))
        {
            return MissingValue.Zero;
        }

        if (!left.IsKnown || !right.IsKnown || left.Type == MissingType.Float || right.Type == MissingType.Float)
        {
            return MissingValue.Unknown;
        }

        var type = MissingType.Int;

        if (left.Type == MissingType.UInt || right.Type == MissingType.UInt)
        {
            type = MissingType.UInt;
        }

        var (a, b) = (left.Bits, right.Bits);
        uint? bits = operation switch
        {
            "|" => a | b,
            "&" => a & b,
            "^" => a ^ b,
            "+" => a + b,
            "-" => a - b,
            "*" => a * b,
            "<<" => a << (int)(b & ShiftMask),
            ">>" when type == MissingType.UInt => a >> (int)(b & ShiftMask),
            ">>" => (uint)((int)a >> (int)(b & ShiftMask)),
            "/" when b != 0 && type == MissingType.UInt => a / b,
            "%" when b != 0 && type == MissingType.UInt => a % b,
            _ => null,
        };

        if (bits is null)
        {
            return MissingValue.Unknown;
        }

        return MissingValue.Of(bits.Value, type);
    }

    private static MissingValue Choose(MissingValue condition, MissingValue whenTrue, MissingValue whenFalse)
    {
        return Truth(condition) switch
        {
            true => whenTrue,
            false => whenFalse,
            _ => MissingValue.Merge(whenTrue, whenFalse),
        };
    }

    private static MissingValue Cast(string type, MissingValue value)
    {
        return type switch
        {
            "uint" => Convert(value, MissingType.UInt),
            "int" => Convert(value, MissingType.Int),
            "float" => Convert(value, MissingType.Float),
            "bool" => Convert(value, MissingType.Bool),
            _ => MissingValue.Unknown,
        };
    }

    private static MissingValue Reinterpret(MissingValue value, MissingType type)
    {
        if (!value.IsKnown || value.Type == MissingType.Bool)
        {
            return MissingValue.Unknown;
        }

        return value with { Type = type };
    }

    private static MissingValue Negate(MissingValue value)
    {
        if (value.IsKnown && value.Type == MissingType.Float)
        {
            return value with { Bits = value.Bits ^ SignBit };
        }

        return Integer(value, bits => 0u - bits);
    }

    private static MissingValue Extreme(MissingValue first, MissingValue second, bool maximum)
    {
        if (first == second)
        {
            return first;
        }

        if (!first.IsKnown || !second.IsKnown || first.Type == MissingType.Float || second.Type == MissingType.Float)
        {
            return MissingValue.Unknown;
        }

        var greater = Compare(">", first, second).Bits != 0;

        if (greater == maximum)
        {
            return Convert(first, CommonType(first, second));
        }

        return Convert(second, CommonType(first, second));
    }

    private static MissingType CommonType(MissingValue first, MissingValue second)
    {
        if (first.Type == MissingType.UInt || second.Type == MissingType.UInt)
        {
            return MissingType.UInt;
        }

        return MissingType.Int;
    }

    /// <summary>An operation on the bits of a known integer.</summary>
    private static MissingValue Integer(MissingValue value, Func<uint, uint> operation)
    {
        if (!value.IsKnown || value.Type == MissingType.Float)
        {
            return MissingValue.Unknown;
        }

        var type = value.Type;

        if (type == MissingType.Bool)
        {
            type = MissingType.Int;
        }

        return MissingValue.Of(operation(value.Bits), type);
    }

    private static uint FirstBitLow(uint bits)
    {
        if (bits == 0)
        {
            return NoBit;
        }

        return (uint)BitOperations.TrailingZeroCount(bits);
    }

    /// <summary>A lane number compared with a known integer it cannot be (the invalid-lane guard of a DPP read).</summary>
    private static bool OutOfRange(MissingValue lane, MissingValue value)
    {
        return lane.Kind == MissingKind.LaneNumber && value.IsKnown && value.Type != MissingType.Float && value.Bits >= lane.Bits;
    }

    private static bool IsIntegerZero(MissingValue value)
    {
        return value.IsKnown && value.Type != MissingType.Float && value.Bits == 0;
    }

    /// <summary>Whether a known value is true (nonzero; for a float, not ±0).</summary>
    private static bool? Truth(MissingValue value)
    {
        if (!value.IsKnown)
        {
            return null;
        }

        if (value.Type == MissingType.Float)
        {
            return (value.Bits & ~SignBit) != 0;
        }

        return value.Bits != 0;
    }

    private static double Number(MissingValue value)
    {
        switch (value.Type)
        {
            case MissingType.Float:
                return BitConverter.UInt32BitsToSingle(value.Bits);
            case MissingType.Int:
                return (int)value.Bits;
            default:
                return value.Bits;
        }
    }

    /// <summary>A float converted to an integer type: truncated when it fits, else unknown (undefined in HLSL).</summary>
    private static MissingValue FromFloat(float value, MissingType type)
    {
        var truncated = Math.Truncate((double)value);

        if (type == MissingType.UInt && truncated >= 0 && truncated <= uint.MaxValue)
        {
            return MissingValue.Of((uint)truncated, type);
        }

        if (type == MissingType.Int && truncated >= int.MinValue && truncated <= int.MaxValue)
        {
            return MissingValue.Of((uint)(int)truncated, type);
        }

        return MissingValue.Unknown;
    }

    private (string Operator, int Precedence)? PeekOperator()
    {
        SkipSpaces();

        foreach (var entry in Operators)
        {
            if (string.CompareOrdinal(_text, _position, entry.Operator, 0, entry.Operator.Length) == 0)
            {
                return entry;
            }
        }

        return null;
    }

    private bool Take(string token)
    {
        SkipSpaces();

        if (string.CompareOrdinal(_text, _position, token, 0, token.Length) != 0)
        {
            return false;
        }

        _position += token.Length;

        return true;
    }

    private void Expect(string token)
    {
        if (!Take(token))
        {
            throw new FormatException(_text);
        }
    }

    private void SkipSpaces()
    {
        while (_position < _text.Length && char.IsWhiteSpace(_text[_position]))
        {
            _position++;
        }
    }

    [GeneratedRegex(@"\G\((?<type>(?:u?int|float|half|bool|min16(?:u?int|float))(?:[1-4])?)\)")]
    private static partial Regex CastPattern();

    [GeneratedRegex(@"\G(?:0[xX][0-9a-fA-F]+[uUlL]*|\d+(?:\.\d*)?(?:[eE][-+]?\d+)?[uUfFlLhH]*)")]
    private static partial Regex NumberPattern();

    [GeneratedRegex(@"\G[A-Za-z_]\w*")]
    private static partial Regex NamePattern();
}
