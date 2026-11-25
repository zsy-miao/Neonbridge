// Copyright (c) 2026 Five Star Recomp contributors.
// SPDX-License-Identifier: GPL-3.0-or-later

using FiveStar.Shaders.Dxil.Hlsl;
using FiveStar.Shaders.Dxil.Ir;
using FiveStar.Shaders.Dxil.Ops;

namespace FiveStar.Shaders.Dxil.Lowering;

/// <summary>Wave and quad operations, group barriers, and atomics on thread-group shared memory.</summary>
public sealed class WaveIntrinsics
{
    private static readonly Dictionary<string, (byte Operation, bool Prefix)> Reductions = new(StringComparer.Ordinal)
    {
        ["WaveActiveSum"] = (0, false),
        ["WaveActiveProduct"] = (1, false),
        ["WaveActiveMin"] = (2, false),
        ["WaveActiveMax"] = (3, false),
        ["WavePrefixSum"] = (0, true),
        ["WavePrefixProduct"] = (1, true),
    };

    private static readonly Dictionary<string, byte> BitReductions = new(StringComparer.Ordinal)
    {
        ["WaveActiveBitAnd"] = 0,
        ["WaveActiveBitOr"] = 1,
        ["WaveActiveBitXor"] = 2,
    };

    private static readonly Dictionary<string, byte> QuadReads = new(StringComparer.Ordinal)
    {
        ["QuadReadAcrossX"] = 0,
        ["QuadReadAcrossY"] = 1,
        ["QuadReadAcrossDiagonal"] = 2,
    };

    private static readonly Dictionary<string, uint> Barriers = new(StringComparer.Ordinal)
    {
        ["GroupMemoryBarrierWithGroupSync"] = 9,
        ["GroupMemoryBarrier"] = 8,
        ["DeviceMemoryBarrierWithGroupSync"] = 3,
        ["DeviceMemoryBarrier"] = 2,
        ["AllMemoryBarrierWithGroupSync"] = 11,
        ["AllMemoryBarrier"] = 10,
    };

    private static readonly Dictionary<string, AtomicOp> SharedAtomics = new(StringComparer.Ordinal)
    {
        ["InterlockedAdd"] = AtomicOp.Add,
        ["InterlockedAnd"] = AtomicOp.And,
        ["InterlockedOr"] = AtomicOp.Or,
        ["InterlockedXor"] = AtomicOp.Xor,
        ["InterlockedMin"] = AtomicOp.UMin,
        ["InterlockedMax"] = AtomicOp.UMax,
        ["InterlockedExchange"] = AtomicOp.Exchange,
    };

    private const byte Signed = 0;
    private const byte Unsigned = 1;
    private const int OriginalArgument = 2;
    private const int ExchangeOriginal = 3;

    private readonly LoweringContext _context;

    public WaveIntrinsics(LoweringContext context)
    {
        _context = context;
    }

    private IrBuilder Builder => _context.Builder;

    private DxilModule Module => _context.Module;

    private DxOps Ops => _context.Ops;

    public void Register(Dictionary<string, Func<SyntaxNode, RValue>> handlers)
    {
        handlers["WaveGetLaneIndex"] = _ => Lane(DxOpCode.WaveGetLaneIndex, "waveGetLaneIndex");
        handlers["WaveGetLaneCount"] = _ => Lane(DxOpCode.WaveGetLaneCount, "waveGetLaneCount");
        handlers["WaveIsFirstLane"] = _ => Truth(Wave(DxOpCode.WaveIsFirstLane, "waveIsFirstLane", Module.Types.Bool));
        handlers["WaveActiveAnyTrue"] = call => Truth(Wave(DxOpCode.WaveAnyTrue, "waveAnyTrue", Module.Types.Bool, Condition(call)));
        handlers["WaveActiveAllTrue"] = call => Truth(Wave(DxOpCode.WaveAllTrue, "waveAllTrue", Module.Types.Bool, Condition(call)));
        handlers["WaveActiveCountBits"] = call => RValue.Of(HlslType.Uint, Wave(DxOpCode.WaveAllBitCount, "waveAllOp", Module.Types.Int32, Condition(call)));

        handlers["WavePrefixCountBits"] = call => RValue.Of(HlslType.Uint, Wave(DxOpCode.WavePrefixBitCount, "wavePrefixOp", Module.Types.Int32, Condition(call)));
        handlers["WaveActiveBallot"] = Ballot;
        handlers["WaveReadLaneAt"] = ReadLaneAt;
        handlers["WaveReadLaneFirst"] = call => PerComponent(call, value => Overloaded(DxOpCode.WaveReadLaneFirst, "waveReadLaneFirst", value));
        handlers["QuadReadLaneAt"] = QuadReadLaneAt;
        handlers["IsHelperLane"] = _ => Truth(Ops.Overloaded(DxOpCode.IsHelperLane, "isHelperLane", AttributeSet.ReadOnly, Module.Types.Bool, Module.Types.Bool));

        RegisterReductions(handlers);
    }

    private void RegisterReductions(Dictionary<string, Func<SyntaxNode, RValue>> handlers)
    {
        foreach (var (name, (operation, prefix)) in Reductions)
        {
            handlers[name] = call => Reduce(call, operation, prefix);
        }

        foreach (var (name, operation) in BitReductions)
        {
            handlers[name] = call => PerComponent(call, value => Overloaded(DxOpCode.WaveActiveBit, "waveActiveBit", value, Module.Int8(operation)));
        }

        foreach (var (name, operation) in QuadReads)
        {
            handlers[name] = call => PerComponent(call, value => Overloaded(DxOpCode.QuadOp, "quadOp", value, Module.Int8(operation)));
        }

        foreach (var (name, mode) in Barriers)
        {
            handlers[name] = _ => Barrier(mode);
        }

        foreach (var (name, operation) in SharedAtomics)
        {
            handlers[name] = call => SharedAtomic(call, operation);
        }

        handlers["InterlockedCompareExchange"] = call => SharedCompareExchange(call, store: false);
        handlers["InterlockedCompareStore"] = call => SharedCompareExchange(call, store: true);
    }

    private RValue Lane(DxOpCode code, string name)
    {
        return RValue.Of(HlslType.Uint, Wave(code, name, Module.Types.Int32));
    }

    private static RValue Truth(Value value)
    {
        return RValue.Of(HlslType.Bool, value);
    }

    private Value Condition(SyntaxNode call)
    {
        return _context.Expressions.Condition(call[0]);
    }

    private Value Wave(DxOpCode code, string name, IrType result, params Value[] arguments)
    {
        return Ops.Call(code, name, AttributeSet.NoUnwind, result, arguments);
    }

    /// <summary>A wave operation overloaded on the value's type (bools travel as i32).</summary>
    private Value Overloaded(DxOpCode code, string name, Value value, params Value[] rest)
    {
        var boolean = value.Type == Module.Types.Bool;
        var operand = value;

        if (boolean)
        {
            operand = Builder.Cast(CastOp.ZExt, value, Module.Types.Int32);
        }

        var result = Ops.Overloaded(code, name, AttributeSet.NoUnwind, operand.Type, operand.Type, [operand, .. rest]);

        if (boolean)
        {
            return Builder.Compare(Predicate.NotEqual, result, Module.Int32(0));
        }

        return result;
    }

    private RValue PerComponent(SyntaxNode call, Func<Value, Value> operation)
    {
        var value = Concrete(_context.Expressions.Lower(call[0]));

        return new RValue(value.Type, [.. value.Components.Select(operation)]);
    }

    private RValue Concrete(RValue value)
    {
        var kind = IntrinsicArguments.Concrete(value.Type.Kind);

        if (value.Type.Kind == ScalarKind.Bool)
        {
            return value;
        }

        return _context.Convert.Convert(value, value.Type.WithKind(kind));
    }

    private RValue ReadLaneAt(SyntaxNode call)
    {
        var value = Concrete(_context.Expressions.Lower(call[0]));
        var lane = _context.Expressions.Index(call[1]);

        return new RValue(value.Type, [.. value.Components.Select(component => Overloaded(DxOpCode.WaveReadLaneAt, "waveReadLaneAt", component, lane))]);
    }

    private RValue QuadReadLaneAt(SyntaxNode call)
    {
        var value = Concrete(_context.Expressions.Lower(call[0]));
        var lane = _context.Expressions.Index(call[1]);

        return new RValue(value.Type, [.. value.Components.Select(component => Overloaded(DxOpCode.QuadReadLaneAt, "quadReadLaneAt", component, lane))]);
    }

    private RValue Ballot(SyntaxNode call)
    {
        var ballot = Wave(DxOpCode.WaveActiveBallot, "waveActiveBallot", Ops.FourInt32, Condition(call));
        var components = Enumerable.Range(0, sizeof(uint)).Select(index => Builder.Extract(ballot, index)).ToArray();

        return new RValue(HlslType.Vector(ScalarKind.Uint, sizeof(uint)), components);
    }

    private RValue Reduce(SyntaxNode call, byte operation, bool prefix)
    {
        var value = Concrete(_context.Expressions.Lower(call[0]));
        var sign = Choice.Of(value.Type.Kind is ScalarKind.Uint or ScalarKind.Uint64, Unsigned, Signed);
        var code = Choice.Of(prefix, DxOpCode.WavePrefixOp, DxOpCode.WaveActiveOp);
        var name = Choice.Of(prefix, "wavePrefixOp", "waveActiveOp");

        return new RValue(value.Type, [.. value.Components.Select(component => Overloaded(code, name, component, Module.Int8(operation), Module.Int8(sign)))]);
    }

    private RValue Barrier(uint mode)
    {
        Ops.Call(DxOpCode.Barrier, "barrier", AttributeSet.NoDuplicate, Module.Types.Void, Module.Int32(mode));

        return RValue.Void;
    }

    private RValue SharedAtomic(SyntaxNode call, AtomicOp operation)
    {
        var texture = TextureTarget(call);

        if (texture is not null)
        {
            return _context.Resources.Textures.Atomic(call, texture);
        }

        var place = _context.Expressions.Place(call[0]);

        if (place.Variable.Storage != StorageKind.Memory)
        {
            throw SourceError.At(call, $"{call.Text} on {place.Variable.Name}: only shared-memory atomics are supported");
        }

        var argument = _context.Expressions.Lower(call[1]);

        if (argument.Type.Kind == ScalarKind.Int && operation is AtomicOp.UMin or AtomicOp.UMax)
        {
            operation = Choice.Of(operation == AtomicOp.UMin, AtomicOp.Min, AtomicOp.Max);
        }

        var value = _context.Convert.Convert(argument, place.Type).Scalar;
        var original = Builder.AtomicRmw(operation, _context.Variables.Address(place, 0), value);

        if (call.Children.Count > OriginalArgument)
        {
            _context.Variables.Write(_context.Expressions.Place(call[OriginalArgument]), RValue.Of(place.Type, original));
        }

        return RValue.Void;
    }

    /// <summary>The texture of an Interlocked call on a texel (<c>texture[uv]</c>), or null.</summary>
    private RValue? TextureTarget(SyntaxNode call)
    {
        var target = call[0];

        if (target.Kind != SyntaxKind.Index)
        {
            return null;
        }

        var owner = _context.Expressions.Lower(target[0]);

        if (owner.Type.Shape != TypeShape.Resource)
        {
            return null;
        }

        return owner;
    }

    private RValue SharedCompareExchange(SyntaxNode call, bool store)
    {
        var texture = TextureTarget(call);

        if (texture is not null)
        {
            return _context.Resources.Textures.Atomic(call, texture);
        }

        var place = _context.Expressions.Place(call[0]);
        var compare = _context.Expressions.Lower(call[1], place.Type).Scalar;
        var value = _context.Expressions.Lower(call[OriginalArgument], place.Type).Scalar;
        var original = Builder.CompareExchange(_context.Variables.Address(place, 0), compare, value);

        if (!store)
        {
            _context.Variables.Write(_context.Expressions.Place(call[ExchangeOriginal]), RValue.Of(place.Type, original));
        }

        return RValue.Void;
    }
}
