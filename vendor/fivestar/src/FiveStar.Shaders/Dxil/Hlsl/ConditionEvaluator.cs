// Copyright (c) 2026 Five Star Recomp contributors.
// SPDX-License-Identifier: GPL-3.0-or-later

namespace FiveStar.Shaders.Dxil.Hlsl;

/// <summary>Evaluates a preprocessor condition: integers, unknown names as 0, C operators by precedence.</summary>
public sealed class ConditionEvaluator
{
    private static readonly string[][] Levels =
    [
        ["||"], ["&&"], ["|"], ["^"], ["&"], ["==", "!="], ["<", ">", "<=", ">="], ["<<", ">>"], ["+", "-"], ["*", "/", "%"],
    ];

    private readonly List<Token> _tokens;
    private readonly int _line;
    private int _position;

    public ConditionEvaluator(List<Token> tokens, int line)
    {
        _tokens = tokens;
        _line = line;
    }

    public long Evaluate()
    {
        var value = Conditional();

        if (_position != _tokens.Count)
        {
            throw new DxilBackendException($"line {_line}: cannot evaluate #if condition at '{_tokens[_position].Text}'");
        }

        return value;
    }

    private long Conditional()
    {
        var condition = Binary(0);

        if (!Accept("?"))
        {
            return condition;
        }

        var whenTrue = Conditional();
        Expect(":");
        var whenFalse = Conditional();

        if (condition != 0)
        {
            return whenTrue;
        }

        return whenFalse;
    }

    private long Binary(int level)
    {
        if (level == Levels.Length)
        {
            return Unary();
        }

        var left = Binary(level + 1);

        while (_position < _tokens.Count && Levels[level].Contains(_tokens[_position].Text) && _tokens[_position].Kind == TokenKind.Punctuator)
        {
            var op = _tokens[_position].Text;
            _position++;
            left = Apply(op, left, Binary(level + 1));
        }

        return left;
    }

    private static long Apply(string op, long left, long right)
    {
        return op switch
        {
            "||" => Truth(left != 0 || right != 0),
            "&&" => Truth(left != 0 && right != 0),
            "|" => left | right,
            "^" => left ^ right,
            "&" => left & right,
            "==" => Truth(left == right),
            "!=" => Truth(left != right),
            "<" => Truth(left < right),
            ">" => Truth(left > right),
            "<=" => Truth(left <= right),
            ">=" => Truth(left >= right),
            "<<" => left << (int)right,
            ">>" => left >> (int)right,
            "+" => left + right,
            "-" => left - right,
            "*" => left * right,
            "/" => left / right,
            _ => left % right,
        };
    }

    private long Unary()
    {
        if (Accept("!"))
        {
            return Truth(Unary() == 0);
        }

        if (Accept("-"))
        {
            return -Unary();
        }

        if (Accept("~"))
        {
            return ~Unary();
        }

        if (Accept("("))
        {
            var value = Conditional();
            Expect(")");

            return value;
        }

        if (_position >= _tokens.Count)
        {
            throw new DxilBackendException($"line {_line}: #if condition ends early");
        }

        var token = _tokens[_position];
        _position++;

        return (long)token.Integer;
    }

    private static long Truth(bool value)
    {
        if (value)
        {
            return 1;
        }

        return 0;
    }

    private bool Accept(string text)
    {
        if (_position < _tokens.Count && _tokens[_position].Is(text))
        {
            _position++;

            return true;
        }

        return false;
    }

    private void Expect(string text)
    {
        if (!Accept(text))
        {
            throw new DxilBackendException($"line {_line}: expected '{text}' in #if condition");
        }
    }
}
