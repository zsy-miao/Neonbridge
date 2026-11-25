// Copyright (c) 2026 Five Star Recomp contributors.
// SPDX-License-Identifier: GPL-3.0-or-later

namespace FiveStar.Shaders.Dxil.Hlsl;

/// <summary>The kinds of syntax nodes; <see cref="SyntaxNode"/> says what each holds.</summary>
public enum SyntaxKind
{
    // Declarations
    Struct,
    Cbuffer,
    Function,
    Parameter,
    Field,
    Variable,
    Declarator,
    Attribute,

    // Statements
    Block,
    ExpressionStatement,
    If,
    For,
    While,
    Do,
    Switch,
    Case,
    Break,
    Continue,
    Return,
    Discard,
    Empty,

    // Expressions
    IntegerLiteral,
    RealLiteral,
    BoolLiteral,
    Name,
    Unary,
    PostIncrement,
    Binary,
    Assign,
    Conditional,
    Call,
    MethodCall,
    Cast,
    Member,
    Index,
    InitializerList,
    Comma,
}
