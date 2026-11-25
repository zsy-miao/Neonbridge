// Copyright (c) 2026 Five Star Recomp contributors.
// SPDX-License-Identifier: GPL-3.0-or-later

using Microsoft.CodeAnalysis;

namespace FiveStar.Loader.Generators;

/// <summary>A top-level type that declares shims: the part of it the generated partial declaration repeats.</summary>
internal sealed record ShimDeclaringType
{
    public ShimDeclaringType(string fullName, string? containingNamespace, string name, bool isStatic)
    {
        FullName = fullName;
        Namespace = containingNamespace;
        Name = name;
        IsStatic = isStatic;
    }

    /// <summary>The name with <c>global::</c> and namespace, as generated code refers to the type.</summary>
    public string FullName { get; }

    /// <summary>The containing namespace, or null for the global namespace.</summary>
    public string? Namespace { get; }

    public string Name { get; }

    public bool IsStatic { get; }

    public static ShimDeclaringType Of(INamedTypeSymbol type)
    {
        string? containingNamespace = null;

        if (!type.ContainingNamespace.IsGlobalNamespace)
        {
            containingNamespace = type.ContainingNamespace.ToDisplayString();
        }

        return new ShimDeclaringType(type.ToDisplayString(SymbolDisplayFormat.FullyQualifiedFormat), containingNamespace, type.Name, type.IsStatic);
    }
}
