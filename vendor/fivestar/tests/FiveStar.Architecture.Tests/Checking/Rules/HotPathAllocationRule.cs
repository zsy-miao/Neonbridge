// Copyright (c) 2026 Five Star Recomp contributors.
// SPDX-License-Identifier: GPL-3.0-or-later

using System.Collections.Frozen;
using Microsoft.CodeAnalysis;
using Microsoft.CodeAnalysis.CSharp;
using Microsoft.CodeAnalysis.CSharp.Syntax;

namespace FiveStar.Architecture.Tests.Checking.Rules;

/// <summary>
/// A method marked <c>[HotPath]</c> allocates nothing and never blocks: no lambda, LINQ, string building, array,
/// collection expression, <c>new</c> of a non-value type, <c>params</c> call, <c>lock</c> or <c>await</c>.
/// </summary>
public sealed class HotPathAllocationRule : IRule
{
    private static readonly FrozenSet<string> LinqMethods = new[]
    {
        "Select", "SelectMany", "Where", "OrderBy", "OrderByDescending", "ThenBy", "ThenByDescending", "GroupBy", "Join",
        "GroupJoin", "Distinct", "DistinctBy", "Union", "Intersect", "Except", "Concat", "Zip", "Skip", "SkipWhile",
        "Take", "TakeWhile", "ToList", "ToArray", "ToDictionary", "ToHashSet", "ToLookup", "Aggregate", "Any", "All",
        "First", "FirstOrDefault", "Last", "LastOrDefault", "Single", "SingleOrDefault", "Count", "LongCount", "Sum",
        "Min", "MinBy", "Max", "MaxBy", "Average", "Cast", "OfType", "AsEnumerable", "DefaultIfEmpty", "ElementAt",
        "Append", "Prepend",
    }.ToFrozenSet(StringComparer.Ordinal);

    private static readonly FrozenSet<string> StringBuilders =
        new[] { "Format", "Concat", "Join" }.ToFrozenSet(StringComparer.Ordinal);

    public string Name => "HotPathAllocation";

    public IReadOnlyList<Violation> Check(CodeBase codeBase)
    {
        var facts = new HotPathFacts(codeBase);
        var violations = new List<Violation>();

        foreach (var source in codeBase.CSharpFiles)
        {
            foreach (var method in source.Root.DescendantNodes().Where(IsHotPath))
            {
                var findings = method
                    .DescendantNodes()
                    .Select(node => (Node: node, Finding: Finding(node, facts)))
                    .Where(found => found.Finding is not null);

                violations.AddRange(findings.Select(found => source.Violation(Name, found.Node, $"[HotPath] method has {found.Finding}")));
            }
        }

        return violations;
    }

    private static bool IsHotPath(SyntaxNode node)
    {
        var lists = node switch
        {
            BaseMethodDeclarationSyntax method => method.AttributeLists,
            LocalFunctionStatementSyntax local => local.AttributeLists,
            _ => default,
        };

        return lists
            .SelectMany(list => list.Attributes)
            .Any(attribute => TypeDeclarations.AttributeName(attribute) == CodePolicy.HotPathAttribute);
    }

    /// <summary>What the node allocates or blocks on, or null when it does neither.</summary>
    private static string? Finding(SyntaxNode node, HotPathFacts facts) => node switch
    {
        AnonymousFunctionExpressionSyntax => "a lambda or anonymous method",
        QueryExpressionSyntax => "a LINQ query",
        InterpolatedStringExpressionSyntax => "string interpolation",
        ArrayCreationExpressionSyntax or ImplicitArrayCreationExpressionSyntax => "an array creation",
        CollectionExpressionSyntax => "a collection expression",
        AnonymousObjectCreationExpressionSyntax => "an anonymous object",
        LockStatementSyntax => "a lock",
        AwaitExpressionSyntax => "an await",
        ObjectCreationExpressionSyntax creation => NewOf(creation.Type, facts),
        ImplicitObjectCreationExpressionSyntax creation => NewOfTarget(creation, facts),
        BinaryExpressionSyntax binary => Concatenation(binary),
        AssignmentExpressionSyntax assignment => Concatenation(assignment),
        InvocationExpressionSyntax invocation => Call(invocation, facts),
        _ => null,
    };

    private static string? NewOf(TypeSyntax type, HotPathFacts facts)
    {
        if (type is NullableTypeSyntax)
        {
            return null;
        }

        if (type is PredefinedTypeSyntax predefined)
        {
            if (predefined.Keyword.IsKind(SyntaxKind.StringKeyword) || predefined.Keyword.IsKind(SyntaxKind.ObjectKeyword))
            {
                return $"`new {type}` of a non-value type";
            }

            return null;
        }

        if (facts.ValueTypes.Contains(TypeDeclarations.SimpleName(type)))
        {
            return null;
        }

        return $"`new {type}` of a non-value type";
    }

    /// <summary>A target-typed <c>new()</c> is checked against the declared type of its local, when it has one.</summary>
    private static string? NewOfTarget(ImplicitObjectCreationExpressionSyntax creation, HotPathFacts facts)
    {
        var declaration = creation.Parent?.Parent?.Parent as VariableDeclarationSyntax;

        if (declaration is null || declaration.Type.IsVar)
        {
            return "a target-typed `new()` whose type the code does not name";
        }

        return NewOf(declaration.Type, facts);
    }

    private static string? Concatenation(BinaryExpressionSyntax binary)
    {
        if (!binary.IsKind(SyntaxKind.AddExpression) || binary.Parent.IsKind(SyntaxKind.AddExpression))
        {
            return null;
        }

        var operands = binary.DescendantNodesAndSelf(node => node.IsKind(SyntaxKind.AddExpression));

        if (!operands.Any(IsString))
        {
            return null;
        }

        return "string concatenation";
    }

    private static string? Concatenation(AssignmentExpressionSyntax assignment)
    {
        if (!assignment.IsKind(SyntaxKind.AddAssignmentExpression) || !IsString(assignment.Right))
        {
            return null;
        }

        return "string concatenation";
    }

    private static bool IsString(SyntaxNode node) =>
        node.IsKind(SyntaxKind.StringLiteralExpression) || node is InterpolatedStringExpressionSyntax;

    private static string? Call(InvocationExpressionSyntax invocation, HotPathFacts facts)
    {
        var access = invocation.Expression as MemberAccessExpressionSyntax;
        var name = MethodName(invocation.Expression);

        if (access is not null && IsStringType(access.Expression) && StringBuilders.Contains(name))
        {
            return $"string building `string.{name}`";
        }

        if (access is not null && LinqMethods.Contains(name))
        {
            return $"a LINQ call `{name}`";
        }

        if (facts.ParamsMethods.TryGetValue(name, out var index) && FillsParams(invocation.ArgumentList, index))
        {
            return $"a params call `{name}`";
        }

        return null;
    }

    private static string MethodName(ExpressionSyntax expression)
    {
        if (expression is MemberAccessExpressionSyntax access)
        {
            return access.Name.Identifier.Text;
        }

        if (expression is SimpleNameSyntax simple)
        {
            return simple.Identifier.Text;
        }

        return string.Empty;
    }

    private static bool IsStringType(ExpressionSyntax expression)
    {
        var text = expression.ToString();

        return text == "string" || text == "String" || text == "System.String";
    }

    /// <summary>Whether the call puts its own values into the params array rather than passing an array it holds.</summary>
    private static bool FillsParams(ArgumentListSyntax arguments, int index)
    {
        var count = arguments.Arguments.Count;

        if (count <= index)
        {
            return false;
        }

        var passed = arguments.Arguments[index].Expression;

        return count > index + 1 || !(passed is IdentifierNameSyntax || passed is MemberAccessExpressionSyntax);
    }
}
