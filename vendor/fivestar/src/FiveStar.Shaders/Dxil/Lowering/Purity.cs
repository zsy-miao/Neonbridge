// Copyright (c) 2026 Five Star Recomp contributors.
// SPDX-License-Identifier: GPL-3.0-or-later

using FiveStar.Shaders.Dxil.Hlsl;

namespace FiveStar.Shaders.Dxil.Lowering;

/// <summary>
/// Whether an expression may be computed even where HLSL would skip it: no assignments, no memory or resource access,
/// no wave, quad or barrier operations, and calls only of arithmetic intrinsics, constructors and user functions that
/// are themselves pure (no loops, and writes only to their own locals).
/// </summary>
public sealed class Purity
{
    private static readonly HashSet<string> PureIntrinsics = new(StringComparer.Ordinal)
    {
        "asuint", "asint", "asfloat", "asdouble", "select", "min", "max", "abs", "clamp", "saturate", "sqrt", "rsqrt", "rcp",
        "floor", "ceil", "round", "trunc", "frac", "exp", "exp2", "log", "log2", "log10", "pow", "sin", "cos", "tan", "asin",
        "acos", "atan", "atan2", "countbits", "firstbitlow", "firstbithigh", "reversebits", "f32tof16", "f16tof32", "mad",
        "fma", "isnan", "isinf", "isfinite", "sign", "lerp", "step", "dot", "cross", "length", "normalize", "any", "all",
        "ldexp", "fmod", "distance", "smoothstep", "NonUniformResourceIndex", "WaveGetLaneIndex", "WaveGetLaneCount",
        "degrees", "radians", "sincos", "msad4", "D3DCOLORtoUBYTE4", "transpose", "mul", "determinant", "reflect",
    };

    private readonly LoweringContext _context;
    private readonly Dictionary<SyntaxNode, bool> _functions = [];

    public Purity(LoweringContext context)
    {
        _context = context;
    }

    public bool IsPure(SyntaxNode node)
    {
        return Check(node, null);
    }

    /// <summary>Whether <paramref name="node"/> is pure; inside a function, writes to <paramref name="locals"/> are allowed.</summary>
    private bool Check(SyntaxNode node, HashSet<string>? locals)
    {
        switch (node.Kind)
        {
            case SyntaxKind.MethodCall:
            case SyntaxKind.For:
            case SyntaxKind.While:
            case SyntaxKind.Do:
            case SyntaxKind.Discard:
                return false;
            case SyntaxKind.Assign:
            case SyntaxKind.PostIncrement:
                return WritesLocal(node[0], locals) && Children(node, locals);
            case SyntaxKind.Unary when node.Text is "++" or "--":
                return WritesLocal(node[0], locals) && Children(node, locals);
            case SyntaxKind.Call:
                return CallIsPure(node) && Children(node, locals);
            case SyntaxKind.Index when node[0].Kind == SyntaxKind.Name && IsMemory(node[0].Text, locals):
                return false;
            default:
                return Children(node, locals);
        }
    }

    private bool Children(SyntaxNode node, HashSet<string>? locals)
    {
        foreach (var child in node.Children.Concat(node.BodyOrEmpty))
        {
            if (!Check(child, locals))
            {
                return false;
            }
        }

        return true;
    }

    /// <summary>A global array in memory (shared memory reads may race with other threads' writes, and cost a load).</summary>
    private bool IsMemory(string name, HashSet<string>? locals)
    {
        if (locals is not null && locals.Contains(name))
        {
            return false;
        }

        var variable = _context.Scope.Find(name);

        return variable is not null && variable.Storage == StorageKind.Memory && !variable.IsConstant;
    }

    private static bool WritesLocal(SyntaxNode target, HashSet<string>? locals)
    {
        var root = target;

        while (root.Kind is SyntaxKind.Member or SyntaxKind.Index)
        {
            root = root[0];
        }

        return locals is not null && root.Kind == SyntaxKind.Name && locals.Contains(root.Text);
    }

    private bool CallIsPure(SyntaxNode call)
    {
        if (_context.Functions.TryGetValue(call.Text, out var overloads))
        {
            return overloads.All(FunctionIsPure);
        }

        return PureIntrinsics.Contains(call.Text) || _context.Types.IsStruct(call.Text) || TypeNames.IsScalar(call.Text)
            || TypeNames.Shape(call.Text) is not null;
    }

    private bool FunctionIsPure(SyntaxNode function)
    {
        if (_functions.TryGetValue(function, out var known))
        {
            return known;
        }

        // A recursive question answers no until it is known.
        _functions[function] = false;
        var locals = new HashSet<string>(StringComparer.Ordinal);
        CollectLocals(function, locals);
        var parameters = function.Children.Where(child => child.Kind == SyntaxKind.Parameter);
        var copiesOut = parameters.Any(parameter => parameter.HasModifier("out") || parameter.HasModifier("inout"));
        var pure = !copiesOut && function.Children.All(child => child.Kind == SyntaxKind.Parameter || Check(child, locals));

        _functions[function] = pure;

        return pure;
    }

    private static void CollectLocals(SyntaxNode node, HashSet<string> locals)
    {
        if (node.Kind is SyntaxKind.Parameter or SyntaxKind.Declarator)
        {
            locals.Add(node.Text);
        }

        foreach (var child in node.Children.Concat(node.BodyOrEmpty))
        {
            CollectLocals(child, locals);
        }
    }
}
