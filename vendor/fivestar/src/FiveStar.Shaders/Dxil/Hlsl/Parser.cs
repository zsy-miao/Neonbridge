// Copyright (c) 2026 Five Star Recomp contributors.
// SPDX-License-Identifier: GPL-3.0-or-later

namespace FiveStar.Shaders.Dxil.Hlsl;

/// <summary>Parses preprocessed HLSL into top-level declarations: structs, constant buffers, globals and functions.</summary>
public sealed class Parser
{
    private readonly TokenCursor _tokens;
    private readonly TypeNames _types = new();
    private readonly ExpressionParser _expressions;
    private readonly StatementParser _statements;

    private Parser(List<Token> tokens)
    {
        _tokens = new TokenCursor(tokens);
        _expressions = new ExpressionParser(_tokens, _types);
        _statements = new StatementParser(_tokens, _types, _expressions);
    }

    public static List<SyntaxNode> Parse(List<Token> tokens)
    {
        var parser = new Parser(tokens);
        var declarations = new List<SyntaxNode>();

        while (!parser._tokens.AtEnd)
        {
            var declaration = parser.TopLevel();

            if (declaration is not null)
            {
                declarations.Add(declaration);
            }
        }

        return declarations;
    }

    private SyntaxNode? TopLevel()
    {
        if (_tokens.Accept(";"))
        {
            return null;
        }

        var attributes = _statements.Attributes();

        if (_tokens.Current.Is("struct"))
        {
            return Struct();
        }

        if (_tokens.Current.Is("cbuffer"))
        {
            return Cbuffer();
        }

        var modifiers = _statements.Modifiers();
        var line = _tokens.Line;
        var type = _expressions.ParseType();

        if (_tokens.Peek(1).Is("("))
        {
            var function = Function(type, line);
            SyntaxNode.AddModifiers(function, modifiers);

            if (attributes.Count > 0)
            {
                function.Attributes.AddRange(attributes);
            }

            return function;
        }

        var declaration = new SyntaxNode(SyntaxKind.Variable, line) { Type = type };
        SyntaxNode.AddModifiers(declaration, modifiers);

        do
        {
            declaration.Children.Add(_statements.Declarator(type));
        }
        while (_tokens.Accept(","));

        _tokens.Expect(";");

        return declaration;
    }

    private SyntaxNode Struct()
    {
        var line = _tokens.Next().Line;
        var name = _tokens.Identifier();
        _types.AddStruct(name);
        var node = new SyntaxNode(SyntaxKind.Struct, line) { Text = name };
        Fields(node);
        _tokens.Accept(";");

        return node;
    }

    private SyntaxNode Cbuffer()
    {
        var line = _tokens.Next().Line;
        var name = _tokens.Identifier();
        var node = new SyntaxNode(SyntaxKind.Cbuffer, line) { Text = name, Semantic = _statements.Semantic() };
        Fields(node);
        _tokens.Accept(";");

        return node;
    }

    /// <summary>Braced field declarations: modifiers, type, then one or more names with dimensions and semantics.</summary>
    private void Fields(SyntaxNode owner)
    {
        _tokens.Expect("{");

        while (!_tokens.Accept("}"))
        {
            var modifiers = _statements.Modifiers();
            var type = _expressions.ParseType();

            do
            {
                var line = _tokens.Line;
                var name = _tokens.Identifier();
                var field = new SyntaxNode(SyntaxKind.Field, line) { Text = name, Type = type.WithDimensions(_expressions.Dimensions()) };
                field.Semantic = _statements.Semantic();
                SyntaxNode.AddModifiers(field, modifiers);
                owner.Children.Add(field);
            }
            while (_tokens.Accept(","));

            _tokens.Expect(";");
        }
    }

    private SyntaxNode Function(TypeSyntax returnType, int line)
    {
        var name = _tokens.Identifier();
        var function = new SyntaxNode(SyntaxKind.Function, line) { Text = name, Type = returnType };
        _tokens.Expect("(");

        while (!_tokens.Accept(")"))
        {
            function.Children.Add(Parameter());
            _tokens.Accept(",");
        }

        function.Semantic = _statements.Semantic();

        if (!_tokens.Accept(";"))
        {
            function.Children.Add(_statements.Block());
        }

        return function;
    }

    private SyntaxNode Parameter()
    {
        var modifiers = _statements.Modifiers();

        if (StatementParser.PrimitiveModifiers.Contains(_tokens.Current.Text))
        {
            modifiers.Add(_tokens.Next().Text);
        }

        var line = _tokens.Line;
        var type = _expressions.ParseType();
        var name = _tokens.Identifier();
        var parameter = new SyntaxNode(SyntaxKind.Parameter, line) { Text = name, Type = type.WithDimensions(_expressions.Dimensions()) };
        SyntaxNode.AddModifiers(parameter, modifiers);

        parameter.Semantic = _statements.Semantic();

        if (_tokens.Accept("="))
        {
            parameter.Children.Add(_expressions.Assignment());
        }

        return parameter;
    }
}
