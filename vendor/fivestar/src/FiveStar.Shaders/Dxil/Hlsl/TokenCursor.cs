// Copyright (c) 2026 Five Star Recomp contributors.
// SPDX-License-Identifier: GPL-3.0-or-later

namespace FiveStar.Shaders.Dxil.Hlsl;

/// <summary>The parser's cursor over preprocessed tokens; past the end it reads an end token.</summary>
public sealed class TokenCursor
{
    private readonly List<Token> _tokens;
    private int _position;

    public TokenCursor(List<Token> tokens)
    {
        _tokens = tokens;
    }

    public bool AtEnd => _position >= _tokens.Count;

    public Token Current => Peek(0);

    public int Line => Current.Line;

    public Token Peek(int offset)
    {
        var index = _position + offset;

        if (index >= _tokens.Count)
        {
            var line = 0;

            if (_tokens.Count > 0)
            {
                line = _tokens[^1].Line;
            }

            return new Token(TokenKind.End, string.Empty, line, false);
        }

        return _tokens[index];
    }

    public Token Next()
    {
        var token = Current;
        _position++;

        return token;
    }

    public bool Accept(string text)
    {
        if (Current.Is(text))
        {
            _position++;

            return true;
        }

        return false;
    }

    public void Expect(string text)
    {
        if (!Accept(text))
        {
            throw Error($"expected '{text}'");
        }
    }

    public string Identifier()
    {
        if (Current.Kind != TokenKind.Identifier)
        {
            throw Error("expected a name");
        }

        return Next().Text;
    }

    public DxilBackendException Error(string message)
    {
        return new DxilBackendException($"line {Line}: {message} at '{Current.Text}'");
    }
}
