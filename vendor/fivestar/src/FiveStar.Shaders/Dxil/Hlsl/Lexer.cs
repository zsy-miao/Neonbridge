// Copyright (c) 2026 Five Star Recomp contributors.
// SPDX-License-Identifier: GPL-3.0-or-later

using System.Globalization;

namespace FiveStar.Shaders.Dxil.Hlsl;

/// <summary>Splits HLSL source into tokens: comments and line continuations are dropped, numbers are parsed.</summary>
public sealed class Lexer
{
    private const int HexBase = 16;
    private const string HexPrefix = "0x";
    private const int ExponentDigit = 2;

    // About one token per few characters of translated HLSL: the list is sized once.
    private const int CharactersPerToken = 4;

    private static readonly string[] ThreeCharacter = [">>=", "<<="];

    private static readonly string[] TwoCharacter =
    [
        "&&", "||", "==", "!=", "<=", ">=", "<<", ">>", "++", "--", "+=", "-=", "*=", "/=", "%=", "&=", "|=", "^=", "::",
    ];

    private readonly string _source;
    private readonly List<Token> _tokens;
    private readonly Dictionary<string, string> _interned = new(StringComparer.Ordinal);
    private int _position;
    private int _line = 1;
    private int _tokenStart;
    private bool _startsLine = true;

    private Lexer(string source)
    {
        _tokens = new List<Token>(source.Length / CharactersPerToken);
        _source = source;
    }

    public static List<Token> Tokenize(string source)
    {
        var lexer = new Lexer(source);
        lexer.Run();

        return lexer._tokens;
    }

    private void Run()
    {
        while (SkipSpace())
        {
            var character = _source[_position];
            _tokenStart = _position;

            if (char.IsLetter(character) || character == '_')
            {
                Identifier();
            }
            else if (char.IsDigit(character) || (character == '.' && Peek(1) is >= '0' and <= '9'))
            {
                Number();
            }
            else if (character == '"')
            {
                Text();
            }
            else
            {
                Punctuator();
            }

            _startsLine = false;
        }
    }

    /// <summary>Skips spaces, newlines, comments and line continuations; false at the end.</summary>
    private bool SkipSpace()
    {
        while (_position < _source.Length)
        {
            var character = _source[_position];

            if (character == '\n')
            {
                _line++;
                _startsLine = true;
                _position++;
            }
            else if (character == '\\' && (Peek(1) == '\n' || Peek(1) == '\r'))
            {
                _position++;
                SkipLineBreak();
            }
            else if (char.IsWhiteSpace(character))
            {
                _position++;
            }
            else if (character == '/' && Peek(1) == '/')
            {
                SkipLineComment();
            }
            else if (character == '/' && Peek(1) == '*')
            {
                SkipBlockComment();
            }
            else
            {
                return true;
            }
        }

        return false;
    }

    private void SkipLineBreak()
    {
        if (Peek(0) == '\r')
        {
            _position++;
        }

        if (Peek(0) == '\n')
        {
            _position++;
            _line++;
        }
    }

    private void SkipLineComment()
    {
        while (_position < _source.Length && _source[_position] != '\n')
        {
            _position++;
        }
    }

    private void SkipBlockComment()
    {
        var end = _source.IndexOf("*/", _position + sizeof(char), StringComparison.Ordinal);

        if (end < 0)
        {
            end = _source.Length - sizeof(char);
        }

        for (var index = _position; index < end; index++)
        {
            if (_source[index] == '\n')
            {
                _line++;
            }
        }

        _position = end + sizeof(char);
    }

    private char Peek(int offset)
    {
        var index = _position + offset;

        if (index >= _source.Length)
        {
            return '\0';
        }

        return _source[index];
    }

    private void Identifier()
    {
        var start = _position;

        while (_position < _source.Length && (char.IsLetterOrDigit(_source[_position]) || _source[_position] == '_'))
        {
            _position++;
        }

        Add(new Token(TokenKind.Identifier, Intern(_source.AsSpan(start, _position - start)), _line, _startsLine));
    }

    private void Number()
    {
        var start = _position;
        var hex = _source.AsSpan(_position).StartsWith(HexPrefix, StringComparison.OrdinalIgnoreCase);

        if (hex)
        {
            _position += HexPrefix.Length;

            while (_position < _source.Length && char.IsAsciiHexDigit(_source[_position]))
            {
                _position++;
            }

            var value = ulong.Parse(_source.AsSpan(start + HexPrefix.Length, _position - start - HexPrefix.Length), NumberStyles.HexNumber, CultureInfo.InvariantCulture);
            Add(new Token(TokenKind.Integer, _source[start.._position], _line, _startsLine) { Integer = value, Suffix = IntegerSuffix() });

            return;
        }

        var real = ScanDecimal();
        var digits = _source.AsSpan(start, _position - start);

        if (real)
        {
            var parsed = double.Parse(digits, NumberStyles.Float, CultureInfo.InvariantCulture);
            Add(new Token(TokenKind.Real, digits.ToString(), _line, _startsLine) { Real = parsed, Suffix = RealSuffix() });

            return;
        }

        var integer = ulong.Parse(digits, NumberStyles.None, CultureInfo.InvariantCulture);
        var suffix = IntegerSuffix();

        if (suffix is 'f' or 'h')
        {
            Add(new Token(TokenKind.Real, digits.ToString(), _line, _startsLine) { Real = integer, Suffix = suffix });

            return;
        }

        Add(new Token(TokenKind.Integer, digits.ToString(), _line, _startsLine) { Integer = integer, Suffix = suffix });
    }

    /// <summary>Scans decimal digits, a fraction and an exponent; true when the number is real.</summary>
    private bool ScanDecimal()
    {
        var real = false;

        while (_position < _source.Length)
        {
            var character = _source[_position];

            if (char.IsDigit(character))
            {
                _position++;
            }
            else if (character == '.')
            {
                real = true;
                _position++;
            }
            else if ((character is 'e' or 'E') && (char.IsDigit(Peek(1)) || ((Peek(1) is '+' or '-') && char.IsDigit(Peek(ExponentDigit)))))
            {
                real = true;
                _position += sizeof(char);
            }
            else
            {
                break;
            }
        }

        return real;
    }

    /// <summary>Consumes <c>u</c>, <c>l</c>, <c>ul</c> or <c>f</c>/<c>h</c> after an integer; the first letter, lower case.</summary>
    private char IntegerSuffix()
    {
        var suffix = '\0';

        while (_position < _source.Length && "uUlLfFhH".Contains(_source[_position], StringComparison.Ordinal))
        {
            if (suffix == '\0' || char.ToLowerInvariant(_source[_position]) == 'u')
            {
                suffix = char.ToLowerInvariant(_source[_position]);
            }

            _position++;
        }

        return suffix;
    }

    private char RealSuffix()
    {
        var suffix = '\0';

        if (_position < _source.Length && "fFhHlL".Contains(_source[_position], StringComparison.Ordinal))
        {
            suffix = char.ToLowerInvariant(_source[_position]);
            _position++;
        }

        return suffix;
    }

    private void Text()
    {
        var end = _source.IndexOf('"', _position + 1);

        if (end < 0)
        {
            throw new DxilBackendException($"line {_line}: unterminated string");
        }

        Add(new Token(TokenKind.Text, _source[(_position + 1)..end], _line, _startsLine));
        _position = end + 1;
    }

    private void Punctuator()
    {
        var text = Match(ThreeCharacter) ?? Match(TwoCharacter) ?? Intern(_source.AsSpan(_position, 1));
        _position += text.Length;
        Add(new Token(TokenKind.Punctuator, text, _line, _startsLine));
    }

    private string? Match(string[] candidates)
    {
        foreach (var candidate in candidates)
        {
            if (string.CompareOrdinal(_source, _position, candidate, 0, candidate.Length) == 0)
            {
                return candidate;
            }
        }

        return null;
    }

    private void Add(Token token)
    {
        _tokens.Add(token with { Offset = _tokenStart });
    }

    /// <summary>One string per distinct name, looked up without making a string first.</summary>
    private string Intern(ReadOnlySpan<char> text)
    {
        var lookup = _interned.GetAlternateLookup<ReadOnlySpan<char>>();

        if (!lookup.TryGetValue(text, out var interned))
        {
            interned = text.ToString();
            _interned[interned] = interned;
        }

        return interned;
    }
}
