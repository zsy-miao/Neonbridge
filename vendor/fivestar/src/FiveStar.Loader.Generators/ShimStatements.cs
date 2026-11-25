// Copyright (c) 2026 Five Star Recomp contributors.
// SPDX-License-Identifier: GPL-3.0-or-later

using System.Collections.Immutable;
using Microsoft.CodeAnalysis;
using Microsoft.CodeAnalysis.CSharp;

namespace FiveStar.Loader.Generators;

/// <summary>
/// Turns one attributed declaration into the ShimTable call that registers it. Attribute arguments are written as the
/// literals the compiler evaluated, so the generated file needs none of the declaring file's usings.
/// </summary>
internal static class ShimStatements
{
    private const string ModuleAttribute = "global::FiveStar.Loader.Shims.ShimModuleAttribute";
    private const string SetsLastError = "SetsLastError";

    /// <summary>One AddModule per [ShimModule] of the type.</summary>
    public static ImmutableArray<ShimRegistration> Modules(GeneratorAttributeSyntaxContext target)
    {
        var type = ShimDeclaringType.Of((INamedTypeSymbol)target.TargetSymbol);
        var registrations = ImmutableArray.CreateBuilder<ShimRegistration>();

        foreach (var attribute in target.Attributes)
        {
            var initializer = string.Empty;

            if (attribute.NamedArguments.Length > 0)
            {
                var assignments = attribute.NamedArguments.Select(named => $"{named.Key} = {named.Value.ToCSharpString()}");
                initializer = $" {{ {string.Join(", ", assignments)} }}";
            }

            var statement = $"table.AddModule(new {ModuleAttribute}({Argument(attribute, 0)}){initializer});";
            registrations.Add(Registration(target, attribute, type, ShimRegistrationKind.Module, statement));
        }

        return registrations.ToImmutable();
    }

    /// <summary>AddNotImplemented for a [NotImplementedTable] field.</summary>
    public static ImmutableArray<ShimRegistration> NotImplementedTable(GeneratorAttributeSyntaxContext target)
    {
        var field = target.TargetSymbol;
        var attribute = target.Attributes[0];
        var statement = $"table.AddNotImplemented({Argument(attribute, 0)}, {field.Name});";
        var type = ShimDeclaringType.Of(field.ContainingType);

        return [Registration(target, attribute, type, ShimRegistrationKind.NotImplementedTable, statement)];
    }

    /// <summary>
    /// AddMethod for a method with [Shim] attributes: its native entry point (the address of an [UnmanagedCallersOnly]
    /// method; any other method fails to compile here), its parameter count and every (module, name) it answers.
    /// </summary>
    public static ImmutableArray<ShimRegistration> Method(GeneratorAttributeSyntaxContext target)
    {
        var method = (IMethodSymbol)target.TargetSymbol;
        var signature = method.Parameters.Select(parameter => TypeName(parameter.Type)).ToList();
        signature.Add(ReturnTypeName(method));

        var bindings = target.Attributes.Select(attribute => $"new({Argument(attribute, 0)}, {Argument(attribute, 1)})");
        var lastError = target.Attributes.Any(SetsLastErrorOf) ? "true" : "false";
        var address = $"(nint)(delegate* unmanaged<{string.Join(", ", signature)}>)&{method.Name}";
        var names = $"\"{method.ContainingType.Name}\", \"{method.Name}\"";
        var statement = $"table.AddMethod({address}, {method.Parameters.Length}, {lastError}, {names}, [{string.Join(", ", bindings)}]);";
        var type = ShimDeclaringType.Of(method.ContainingType);

        return [Registration(target, target.Attributes[0], type, ShimRegistrationKind.Method, statement)];
    }

    private static ShimRegistration Registration(
        GeneratorAttributeSyntaxContext target,
        AttributeData attribute,
        ShimDeclaringType type,
        ShimRegistrationKind kind,
        string statement)
    {
        var position = target.TargetNode.SpanStart;

        if (attribute.ApplicationSyntaxReference is not null)
        {
            position = attribute.ApplicationSyntaxReference.Span.Start;
        }

        return new ShimRegistration(type, kind, target.TargetNode.SyntaxTree.FilePath, position, statement);
    }

    private static string Argument(AttributeData attribute, int index)
    {
        return attribute.ConstructorArguments[index].ToCSharpString();
    }

    private static bool SetsLastErrorOf(AttributeData attribute)
    {
        return attribute.NamedArguments.Any(named => named.Key == SetsLastError && Equals(named.Value.Value, true));
    }

    private static string TypeName(ITypeSymbol type)
    {
        return type.ToDisplayString(SymbolDisplayFormat.FullyQualifiedFormat);
    }

    private static string ReturnTypeName(IMethodSymbol method)
    {
        if (method.ReturnsVoid)
        {
            return "void";
        }

        return TypeName(method.ReturnType);
    }
}
