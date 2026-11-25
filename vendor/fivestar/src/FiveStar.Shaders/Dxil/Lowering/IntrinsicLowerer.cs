// Copyright (c) 2026 Five Star Recomp contributors.
// SPDX-License-Identifier: GPL-3.0-or-later

using FiveStar.Shaders.Dxil.Hlsl;
using FiveStar.Shaders.Dxil.Ir;
using FiveStar.Shaders.Dxil.Ops;

namespace FiveStar.Shaders.Dxil.Lowering;

/// <summary>
/// HLSL intrinsic functions as DXIL operations and LLVM instructions, lowered as dxc lowers them (HLOperationLower):
/// math componentwise, wave and quad operations, barriers, shared-memory atomics.
/// </summary>
public sealed class IntrinsicLowerer
{
    private readonly LoweringContext _context;
    private readonly Dictionary<string, Func<SyntaxNode, RValue>> _handlers = new(StringComparer.Ordinal);
    private readonly MathIntrinsics _math;
    private readonly WaveIntrinsics _wave;

    public IntrinsicLowerer(LoweringContext context)
    {
        _context = context;
        _math = new MathIntrinsics(context);
        _wave = new WaveIntrinsics(context);
        _math.Register(_handlers);
        _wave.Register(_handlers);
        _handlers["NonUniformResourceIndex"] = NonUniform;

        _handlers["discard"] = _ => Discard();
        _handlers["GetAttributeAtVertex"] = AttributeAtVertex;
        _handlers["SetMeshOutputCounts"] = call => (_context.Mesh ?? throw SourceError.At(call, "SetMeshOutputCounts outside a mesh shader")).SetOutputCounts(call);
    }

    public RValue Call(SyntaxNode call)
    {
        if (!_handlers.TryGetValue(call.Text, out var handler))
        {
            throw SourceError.At(call, $"intrinsic {call.Text} is not supported");
        }

        return handler(call);
    }

    public RValue Discard()
    {
        var module = _context.Module;
        _context.Ops.Call(DxOpCode.Discard, "discard", AttributeSet.NoUnwind, module.Types.Void, module.Bool(true));

        return RValue.Void;
    }

    private RValue AttributeAtVertex(SyntaxNode call)
    {
        var graphics = _context.Graphics ?? throw SourceError.At(call, "GetAttributeAtVertex outside a pixel shader");
        var vertex = _context.Expressions.Lower(call[1]).Scalar;

        return graphics.AttributeAtVertex(call[0], vertex);
    }

    private RValue NonUniform(SyntaxNode call)
    {
        var value = _context.Expressions.Lower(call[0]);
        _context.Resources.MarkNonUniform(value.Scalar);

        return value;
    }
}
