// Copyright (c) 2026 Five Star Recomp contributors.
// SPDX-License-Identifier: GPL-3.0-or-later

using System.Collections.Frozen;
using Microsoft.CodeAnalysis;
using Microsoft.CodeAnalysis.CSharp;
using Microsoft.CodeAnalysis.CSharp.Syntax;

namespace FiveStar.Architecture.Tests.Checking;

/// <summary>
/// What the hot path rule needs to know about the whole code base: which type names are value types and which methods
/// take a <c>params</c> array. Read from syntax only, so names stand for types and methods.
/// </summary>
public sealed class HotPathFacts
{
    /// <summary>The runtime's value types a hot path commonly creates.</summary>
    private static readonly FrozenSet<string> RuntimeValueTypes = new[]
    {
        "Boolean", "Byte", "SByte", "Char", "Int16", "UInt16", "Int32", "UInt32", "Int64", "UInt64", "IntPtr", "UIntPtr",
        "Single", "Double", "Decimal", "Half", "Int128", "UInt128", "Guid", "DateTime", "DateTimeOffset", "TimeSpan",
        "Span", "ReadOnlySpan", "Memory", "ReadOnlyMemory", "ArraySegment", "ValueTuple", "KeyValuePair", "Nullable",
        "Range", "Index", "CancellationToken", "ValueTask", "SpinWait", "Vector2", "Vector3", "Vector4", "Vector128",
        "Vector256", "Matrix3x2", "Matrix4x4", "Quaternion", "Plane",
    }.ToFrozenSet(StringComparer.Ordinal);

    /// <summary>Parameter types a <c>params</c> collection can have without allocating.</summary>
    private static readonly FrozenSet<string> StackCollections =
        new[] { "Span", "ReadOnlySpan" }.ToFrozenSet(StringComparer.Ordinal);

    public HotPathFacts(CodeBase codeBase)
    {
        var declarations = codeBase.CSharpFiles.SelectMany(source => source.Root.DescendantNodes()).ToArray();

        ValueTypes = declarations
            .OfType<BaseTypeDeclarationSyntax>()
            .Where(TypeDeclarations.IsValueType)
            .Select(type => type.Identifier.Text)
            .Concat(RuntimeValueTypes)
            .Concat(CodePolicy.HotPathValueTypes)
            .ToFrozenSet(StringComparer.Ordinal);

        ParamsMethods = declarations
            .Select(ParamsMethod)
            .OfType<KeyValuePair<string, int>>()
            .GroupBy(method => method.Key, StringComparer.Ordinal)
            .ToFrozenDictionary(group => group.Key, group => group.Min(method => method.Value), StringComparer.Ordinal);
    }

    /// <summary>The names of the value types: the repository's structs and enums, the runtime's and the policy's.</summary>
    public FrozenSet<string> ValueTypes { get; }

    /// <summary>The methods with a <c>params</c> array, by name, with the index of that parameter.</summary>
    public FrozenDictionary<string, int> ParamsMethods { get; }

    private static KeyValuePair<string, int>? ParamsMethod(SyntaxNode node)
    {
        var name = string.Empty;
        ParameterListSyntax? parameters = null;

        if (node is MethodDeclarationSyntax method)
        {
            name = method.Identifier.Text;
            parameters = method.ParameterList;
        }
        else if (node is LocalFunctionStatementSyntax local)
        {
            name = local.Identifier.Text;
            parameters = local.ParameterList;
        }

        if (parameters is null)
        {
            return null;
        }

        var index = parameters.Parameters.IndexOf(IsParamsArray);

        if (index < 0)
        {
            return null;
        }

        return new KeyValuePair<string, int>(name, index);
    }

    private static bool IsParamsArray(ParameterSyntax parameter)
    {
        if (!parameter.Modifiers.Any(SyntaxKind.ParamsKeyword) || parameter.Type is null)
        {
            return false;
        }

        return !StackCollections.Contains(TypeDeclarations.SimpleName(parameter.Type));
    }
}
