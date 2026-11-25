// Copyright (c) 2026 Five Star Recomp contributors.
// SPDX-License-Identifier: GPL-3.0-or-later

using FiveStar.Shaders.Dxil.Hlsl;
using FiveStar.Shaders.Dxil.Ir;
using FiveStar.Shaders.Dxil.Ops;

namespace FiveStar.Shaders.Dxil.Lowering;

/// <summary>A compute entry point's parameters: the thread and group ids its semantics name.</summary>
public sealed class ComputeEntry
{
    private const int Axes = 3;
    private const int Z = 2;

    private static readonly Dictionary<string, (DxOpCode Code, string Name)> Ids = new(StringComparer.OrdinalIgnoreCase)
    {
        ["SV_GroupID"] = (DxOpCode.GroupId, "groupId"),
        ["SV_GroupThreadID"] = (DxOpCode.ThreadIdInGroup, "threadIdInGroup"),
        ["SV_DispatchThreadID"] = (DxOpCode.ThreadId, "threadId"),
    };

    private readonly LoweringContext _context;

    public ComputeEntry(LoweringContext context)
    {
        _context = context;
    }

    /// <summary>The thread-group size of <c>[numthreads(x, y, z)]</c>.</summary>
    public (int X, int Y, int Z) Threads(SyntaxNode main)
    {
        var attribute = main.Attributes.FirstOrDefault(candidate => candidate.Text == "numthreads")
            ?? throw SourceError.At(main, "a compute entry needs [numthreads]");
        var sizes = attribute.Children.Select(size => (int)_context.Expressions.ConstantInteger(size)).ToArray();

        if (sizes.Length != Axes)
        {
            throw SourceError.At(attribute, "numthreads takes three sizes");
        }

        return (sizes[0], sizes[1], sizes[Z]);
    }

    /// <summary>The required wave size of <c>[WaveSize(n)]</c>, if any.</summary>
    public int? WaveSize(SyntaxNode main)
    {
        var attribute = main.Attributes.FirstOrDefault(candidate => candidate.Text == "WaveSize");

        if (attribute is null)
        {
            return null;
        }

        return (int)_context.Expressions.ConstantInteger(attribute[0]);
    }

    /// <summary>Declares each parameter and stores its system value.</summary>
    public void BindParameters(SyntaxNode main)
    {
        foreach (var parameter in main.Children.Where(child => child.Kind == SyntaxKind.Parameter))
        {
            var type = _context.Types.Resolve(parameter.Type!, _ => throw SourceError.At(parameter, "array parameter"));
            var variable = _context.Statements.DeclareLocal(parameter.Text, type);
            var value = SystemValue(parameter, type);
            _context.Variables.Write(_context.Variables.Whole(variable), value);
        }
    }

    /// <summary>The thread or group id a parameter's semantic names, as the parameter's type.</summary>
    public RValue SystemValue(SyntaxNode parameter, HlslType type)
    {
        var semantic = parameter.Semantic ?? throw SourceError.At(parameter, $"entry parameter {parameter.Text} has no semantic");

        if (string.Equals(semantic, "SV_GroupIndex", StringComparison.OrdinalIgnoreCase))
        {
            var index = _context.Ops.Overloaded(DxOpCode.FlattenedThreadIdInGroup, "flattenedThreadIdInGroup", AttributeSet.ReadNone, _context.Module.Types.Int32, _context.Module.Types.Int32);

            return _context.Convert.Convert(RValue.Of(HlslType.Uint, index), type);
        }

        if (!Ids.TryGetValue(semantic, out var id))
        {
            throw SourceError.At(parameter, $"semantic {semantic} is not supported in compute shaders");
        }

        var components = Enumerable.Range(0, type.Count).Select(axis => _context.Ops.ThreadIdComponent(id.Code, id.Name, axis)).ToArray();

        return _context.Convert.Convert(new RValue(HlslType.Vector(ScalarKind.Uint, type.Count), components), type);
    }
}
