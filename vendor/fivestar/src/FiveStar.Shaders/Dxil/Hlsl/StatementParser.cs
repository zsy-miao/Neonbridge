// Copyright (c) 2026 Five Star Recomp contributors.
// SPDX-License-Identifier: GPL-3.0-or-later

namespace FiveStar.Shaders.Dxil.Hlsl;

/// <summary>Parses statements: blocks, declarations, control flow and expression statements, with their attributes.</summary>
public sealed class StatementParser
{
    /// <summary>Modifiers a declaration may start with.</summary>
    public static readonly HashSet<string> DeclarationModifiers = new(StringComparer.Ordinal)
    {
        "static", "const", "groupshared", "uniform", "extern", "precise", "volatile", "inline", "nointerpolation", "linear",
        "centroid", "noperspective", "sample", "in", "out", "inout", "row_major", "column_major", "globallycoherent",
        "snorm", "unorm", "export", "shared", "vertices", "indices", "primitives", "payload",
    };

    /// <summary>The input primitive of a geometry shader's vertex array parameter.</summary>
    public static readonly HashSet<string> PrimitiveModifiers = new(StringComparer.Ordinal)
    {
        "point", "line", "triangle", "lineadj", "triangleadj",
    };

    private readonly TokenCursor _tokens;
    private readonly TypeNames _types;
    private readonly ExpressionParser _expressions;

    public StatementParser(TokenCursor tokens, TypeNames types, ExpressionParser expressions)
    {
        _tokens = tokens;
        _types = types;
        _expressions = expressions;
    }

    public SyntaxNode Block()
    {
        var block = new SyntaxNode(SyntaxKind.Block, _tokens.Line);
        _tokens.Expect("{");

        while (!_tokens.Accept("}"))
        {
            if (_tokens.AtEnd)
            {
                throw _tokens.Error("unterminated block");
            }

            block.Children.Add(Statement());
        }

        return block;
    }

    public SyntaxNode Statement()
    {
        var attributes = Attributes();
        var statement = Unattributed();

        if (attributes.Count > 0)
        {
            statement.Attributes.AddRange(attributes);
        }

        return statement;
    }

    /// <summary>Bracketed attributes: <c>[name]</c> or <c>[name(arguments)]</c>.</summary>
    public List<SyntaxNode> Attributes()
    {
        var attributes = new List<SyntaxNode>();

        while (_tokens.Current.Is("["))
        {
            _tokens.Next();
            var attribute = new SyntaxNode(SyntaxKind.Attribute, _tokens.Line) { Text = _tokens.Identifier() };

            if (_tokens.Accept("("))
            {
                do
                {
                    attribute.Children.Add(AttributeArgument());
                }
                while (_tokens.Accept(","));

                _tokens.Expect(")");
            }

            _tokens.Expect("]");
            attributes.Add(attribute);
        }

        return attributes;
    }

    /// <summary>An attribute argument: an expression, or a quoted name (<c>[outputtopology("triangle")]</c>).</summary>
    private SyntaxNode AttributeArgument()
    {
        if (_tokens.Current.Kind != TokenKind.Text)
        {
            return _expressions.Assignment();
        }

        var token = _tokens.Next();

        return new SyntaxNode(SyntaxKind.Name, token.Line) { Text = token.Text };
    }

    /// <summary>Whether a declaration starts here: a modifier, or a type name followed by a name.</summary>
    public bool AtDeclaration()
    {
        var token = _tokens.Current;

        if (token.Kind != TokenKind.Identifier)
        {
            return false;
        }

        if (DeclarationModifiers.Contains(token.Text))
        {
            return true;
        }

        if (!_types.IsType(token.Text))
        {
            return false;
        }

        var next = _tokens.Peek(1);

        return next.Kind == TokenKind.Identifier || next.Is("<");
    }

    /// <summary>A variable declaration after its modifiers: type, then declarators up to ';'.</summary>
    public SyntaxNode Declaration(HashSet<string> modifiers)
    {
        var line = _tokens.Line;
        var type = _expressions.ParseType();
        var declaration = new SyntaxNode(SyntaxKind.Variable, line) { Type = type };
        SyntaxNode.AddModifiers(declaration, modifiers);

        do
        {
            declaration.Children.Add(Declarator(type));
        }
        while (_tokens.Accept(","));

        _tokens.Expect(";");

        return declaration;
    }

    public HashSet<string> Modifiers()
    {
        var modifiers = new HashSet<string>(StringComparer.Ordinal);

        while (_tokens.Current.Kind == TokenKind.Identifier && DeclarationModifiers.Contains(_tokens.Current.Text))
        {
            modifiers.Add(_tokens.Next().Text);
        }

        return modifiers;
    }

    /// <summary>A name with array dimensions, semantic or register, and initializer.</summary>
    public SyntaxNode Declarator(TypeSyntax type)
    {
        var line = _tokens.Line;
        var name = _tokens.Identifier();
        var declarator = new SyntaxNode(SyntaxKind.Declarator, line) { Text = name, Type = type.WithDimensions(_expressions.Dimensions()) };
        declarator.Semantic = Semantic();

        if (_tokens.Accept("="))
        {
            declarator.Children.Add(_expressions.Initializer());
        }

        return declarator;
    }

    /// <summary>A <c>: SEMANTIC</c> or <c>: register(...)</c> annotation; returns the semantic or register text.</summary>
    public string? Semantic()
    {
        if (!_tokens.Accept(":"))
        {
            return null;
        }

        var name = _tokens.Identifier();

        if (!_tokens.Accept("("))
        {
            return name;
        }

        var parts = new List<string>();

        while (!_tokens.Accept(")"))
        {
            parts.Add(_tokens.Next().Text);
        }

        return $"{name}({string.Concat(parts)})";
    }

    private SyntaxNode Unattributed()
    {
        var token = _tokens.Current;

        if (token.Is("{"))
        {
            return Block();
        }

        if (token.Is(";"))
        {
            _tokens.Next();

            return new SyntaxNode(SyntaxKind.Empty, token.Line);
        }

        if (token.Kind == TokenKind.Identifier)
        {
            var keyword = Keyword(token);

            if (keyword is not null)
            {
                return keyword;
            }

            if (AtDeclaration())
            {
                return Declaration(Modifiers());
            }
        }

        var statement = new SyntaxNode(SyntaxKind.ExpressionStatement, token.Line);
        statement.Children.Add(_expressions.Comma());
        _tokens.Expect(";");

        return statement;
    }

    private SyntaxNode? Keyword(Token token)
    {
        switch (token.Text)
        {
            case "if":
                return If();
            case "for":
                return For();
            case "while":
                return While();
            case "do":
                return Do();
            case "switch":
                return Switch();
            case "break":
                return Simple(SyntaxKind.Break);
            case "continue":
                return Simple(SyntaxKind.Continue);
            case "discard":
                return Simple(SyntaxKind.Discard);
            case "return":
                return Return();
            default:
                return null;
        }
    }

    private SyntaxNode Simple(SyntaxKind kind)
    {
        var line = _tokens.Next().Line;
        _tokens.Expect(";");

        return new SyntaxNode(kind, line);
    }

    private SyntaxNode Return()
    {
        var node = new SyntaxNode(SyntaxKind.Return, _tokens.Next().Line);

        if (!_tokens.Accept(";"))
        {
            node.Children.Add(_expressions.Comma());
            _tokens.Expect(";");
        }

        return node;
    }

    private SyntaxNode If()
    {
        var node = new SyntaxNode(SyntaxKind.If, _tokens.Next().Line);
        node.Children.Add(Parenthesized());
        node.Children.Add(Statement());

        if (_tokens.Accept("else"))
        {
            node.Children.Add(Statement());
        }

        return node;
    }

    private SyntaxNode Parenthesized()
    {
        _tokens.Expect("(");
        var expression = _expressions.Comma();
        _tokens.Expect(")");

        return expression;
    }

    private SyntaxNode For()
    {
        var node = new SyntaxNode(SyntaxKind.For, _tokens.Next().Line);
        _tokens.Expect("(");
        node.Children.Add(ForInit());
        node.Children.Add(Optional(";"));
        _tokens.Expect(";");
        node.Children.Add(Optional(")"));

        _tokens.Expect(")");
        node.Children.Add(Statement());

        return node;
    }

    private SyntaxNode ForInit()
    {
        if (AtDeclaration())
        {
            return Declaration(Modifiers());
        }

        var init = Optional(";");
        _tokens.Expect(";");

        return init;
    }

    /// <summary>An expression, or an Empty node when <paramref name="end"/> follows at once.</summary>
    private SyntaxNode Optional(string end)
    {
        if (_tokens.Current.Is(end))
        {
            return new SyntaxNode(SyntaxKind.Empty, _tokens.Line);
        }

        return _expressions.Comma();
    }

    private SyntaxNode While()
    {
        var node = new SyntaxNode(SyntaxKind.While, _tokens.Next().Line);
        node.Children.Add(Parenthesized());
        node.Children.Add(Statement());

        return node;
    }

    private SyntaxNode Do()
    {
        var node = new SyntaxNode(SyntaxKind.Do, _tokens.Next().Line);
        node.Children.Add(Statement());
        _tokens.Expect("while");
        node.Children.Add(Parenthesized());
        _tokens.Expect(";");

        return node;
    }

    private SyntaxNode Switch()
    {
        var node = new SyntaxNode(SyntaxKind.Switch, _tokens.Next().Line);
        node.Children.Add(Parenthesized());
        _tokens.Expect("{");

        while (!_tokens.Accept("}"))
        {
            node.Children.Add(Case());
        }

        return node;
    }

    private SyntaxNode Case()
    {
        var node = new SyntaxNode(SyntaxKind.Case, _tokens.Line);

        while (_tokens.Current.Is("case") || _tokens.Current.Is("default"))
        {
            if (_tokens.Next().Text == "default")
            {
                node.Children.Add(new SyntaxNode(SyntaxKind.Name, _tokens.Line));
            }
            else
            {
                node.Children.Add(_expressions.Assignment());
            }

            _tokens.Expect(":");
        }

        if (node.Children.Count == 0)
        {
            throw _tokens.Error("expected case or default");
        }

        while (!_tokens.Current.Is("case") && !_tokens.Current.Is("default") && !_tokens.Current.Is("}"))
        {
            node.Body.Add(Statement());
        }

        return node;
    }
}
