// Copyright (c) 2026 Five Star Recomp contributors.
// SPDX-License-Identifier: GPL-3.0-or-later

namespace FiveStar.Shaders.Dxil.Hlsl;

/// <summary>Parses expressions with C precedence: comma, assignment, conditional, binary levels, unary and casts, postfix.</summary>
public sealed class ExpressionParser
{
    private const int AfterName = 2;
    private const int AfterTemplate = 3;

    private static readonly string[][] Levels =
    [
        ["||"], ["&&"], ["|"], ["^"], ["&"], ["==", "!="], ["<", ">", "<=", ">="], ["<<", ">>"], ["+", "-"], ["*", "/", "%"],
    ];

    private static readonly HashSet<string> AssignOperators = new(StringComparer.Ordinal)
    {
        "=", "+=", "-=", "*=", "/=", "%=", "&=", "|=", "^=", "<<=", ">>=",
    };

    private static readonly HashSet<string> Prefix = new(StringComparer.Ordinal) { "-", "+", "!", "~", "++", "--" };

    private readonly TokenCursor _tokens;
    private readonly TypeNames _types;

    public ExpressionParser(TokenCursor tokens, TypeNames types)
    {
        _tokens = tokens;
        _types = types;
    }

    /// <summary>A comma expression (for-loop steps).</summary>
    public SyntaxNode Comma()
    {
        var first = Assignment();

        if (!_tokens.Current.Is(","))
        {
            return first;
        }

        var comma = new SyntaxNode(SyntaxKind.Comma, first.Line);
        comma.Children.Add(first);

        while (_tokens.Accept(","))
        {
            comma.Children.Add(Assignment());
        }

        return comma;
    }

    public SyntaxNode Assignment()
    {
        var target = Conditional();
        var op = _tokens.Current;

        if (op.Kind == TokenKind.Punctuator && AssignOperators.Contains(op.Text))
        {
            _tokens.Next();
            var assign = new SyntaxNode(SyntaxKind.Assign, op.Line) { Text = op.Text };
            assign.Children.Add(target);
            assign.Children.Add(Assignment());

            return assign;
        }

        return target;
    }

    /// <summary>An initializer: a braced list (nested lists allowed) or an assignment expression.</summary>
    public SyntaxNode Initializer()
    {
        if (!_tokens.Current.Is("{"))
        {
            return Assignment();
        }

        var list = new SyntaxNode(SyntaxKind.InitializerList, _tokens.Line);
        _tokens.Expect("{");

        while (!_tokens.Accept("}"))
        {
            list.Children.Add(Initializer());

            if (!_tokens.Accept(","))
            {
                _tokens.Expect("}");

                break;
            }
        }

        return list;
    }

    private SyntaxNode Conditional()
    {
        var condition = Binary(0);

        if (!_tokens.Current.Is("?"))
        {
            return condition;
        }

        var line = _tokens.Next().Line;
        var whenTrue = Assignment();
        _tokens.Expect(":");
        var whenFalse = Assignment();
        var node = new SyntaxNode(SyntaxKind.Conditional, line);
        node.Children.Add(condition);

        node.Children.Add(whenTrue);
        node.Children.Add(whenFalse);

        return node;
    }

    private SyntaxNode Binary(int level)
    {
        if (level == Levels.Length)
        {
            return Unary();
        }

        var left = Binary(level + 1);

        while (_tokens.Current.Kind == TokenKind.Punctuator && Levels[level].Contains(_tokens.Current.Text))
        {
            var op = _tokens.Next();
            var node = new SyntaxNode(SyntaxKind.Binary, op.Line) { Text = op.Text };
            node.Children.Add(left);
            node.Children.Add(Binary(level + 1));
            left = node;
        }

        return left;
    }

    private SyntaxNode Unary()
    {
        var token = _tokens.Current;

        if (token.Kind == TokenKind.Punctuator && Prefix.Contains(token.Text))
        {
            _tokens.Next();
            var node = new SyntaxNode(SyntaxKind.Unary, token.Line) { Text = token.Text };
            node.Children.Add(Unary());

            return node;
        }

        if (token.Is("(") && IsCast())
        {
            _tokens.Next();
            var type = ParseType();
            _tokens.Expect(")");
            var cast = new SyntaxNode(SyntaxKind.Cast, token.Line) { Type = type };
            cast.Children.Add(Unary());

            return cast;
        }

        return Postfix(Primary());
    }

    /// <summary>After '(': a type name (and template or array part) then ')'.</summary>
    private bool IsCast()
    {
        var name = _tokens.Peek(1);

        if (name.Kind != TokenKind.Identifier || !_types.IsType(name.Text))
        {
            return false;
        }

        var next = _tokens.Peek(AfterName);

        return next.Is(")") || next.Is("<") || next.Is("[");
    }

    /// <summary>A type: name, optional template arguments, optional array dimensions.</summary>
    public TypeSyntax ParseType()
    {
        var name = _tokens.Identifier();
        TypeSyntax? argument = null;
        var sizes = new List<long>();

        if (_tokens.Accept("<"))
        {
            argument = ParseType();

            while (_tokens.Accept(","))
            {
                sizes.Add((long)_tokens.Next().Integer);
            }

            _tokens.Expect(">");
        }

        var type = new TypeSyntax(name) { Argument = argument, Sizes = sizes };

        return type.WithDimensions(Dimensions());
    }

    /// <summary>Array dimensions after a name or type: each <c>[size]</c> or <c>[]</c>.</summary>
    public List<SyntaxNode?> Dimensions()
    {
        var dimensions = new List<SyntaxNode?>();

        while (_tokens.Current.Is("["))
        {
            _tokens.Next();

            if (_tokens.Accept("]"))
            {
                dimensions.Add(null);

                continue;
            }

            dimensions.Add(Conditional());
            _tokens.Expect("]");
        }

        return dimensions;
    }

    private SyntaxNode Primary()
    {
        var token = _tokens.Next();

        switch (token.Kind)
        {
            case TokenKind.Integer:
                return new SyntaxNode(SyntaxKind.IntegerLiteral, token.Line) { Literal = token };
            case TokenKind.Real:
                return new SyntaxNode(SyntaxKind.RealLiteral, token.Line) { Literal = token };
            case TokenKind.Identifier:
                return Name(token);
            default:
                break;
        }

        if (token.Is("("))
        {
            var inner = Comma();
            _tokens.Expect(")");

            return inner;
        }

        throw new DxilBackendException($"line {token.Line}: unexpected '{token.Text}'");
    }

    private SyntaxNode Name(Token token)
    {
        if (token.Text is "true" or "false")
        {
            return new SyntaxNode(SyntaxKind.BoolLiteral, token.Line) { Text = token.Text };
        }

        if (_tokens.Current.Is("("))
        {
            var call = new SyntaxNode(SyntaxKind.Call, token.Line) { Text = token.Text };
            Arguments(call);

            return call;
        }

        return new SyntaxNode(SyntaxKind.Name, token.Line) { Text = token.Text };
    }

    private void Arguments(SyntaxNode call)
    {
        _tokens.Expect("(");

        if (_tokens.Accept(")"))
        {
            return;
        }

        do
        {
            call.Children.Add(Assignment());
        }
        while (_tokens.Accept(","));

        _tokens.Expect(")");
    }

    private SyntaxNode Postfix(SyntaxNode operand)
    {
        while (true)
        {
            var token = _tokens.Current;

            if (token.Is("."))
            {
                operand = Member(operand);
            }
            else if (token.Is("["))
            {
                _tokens.Next();
                var index = new SyntaxNode(SyntaxKind.Index, token.Line);
                index.Children.Add(operand);
                index.Children.Add(Comma());
                _tokens.Expect("]");
                operand = index;
            }
            else if (token.Is("++") || token.Is("--"))
            {
                _tokens.Next();
                var post = new SyntaxNode(SyntaxKind.PostIncrement, token.Line) { Text = token.Text };
                post.Children.Add(operand);
                operand = post;
            }
            else
            {
                return operand;
            }
        }
    }

    private SyntaxNode Member(SyntaxNode operand)
    {
        var line = _tokens.Next().Line;
        var name = _tokens.Identifier();

        if (_tokens.Current.Is("<") && _tokens.Peek(1).Kind == TokenKind.Identifier && _tokens.Peek(AfterName).Is(">") && _tokens.Peek(AfterTemplate).Is("("))
        {
            _tokens.Next();
            name = $"{name}<{_tokens.Next().Text}>";
            _tokens.Next();
        }

        if (!_tokens.Current.Is("("))
        {
            var member = new SyntaxNode(SyntaxKind.Member, line) { Text = name };
            member.Children.Add(operand);

            return member;
        }

        var call = new SyntaxNode(SyntaxKind.MethodCall, line) { Text = name };
        call.Children.Add(operand);
        Arguments(call);

        return call;
    }
}
