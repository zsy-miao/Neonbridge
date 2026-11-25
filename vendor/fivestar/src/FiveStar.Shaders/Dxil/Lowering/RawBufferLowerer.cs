// Copyright (c) 2026 Five Star Recomp contributors.
// SPDX-License-Identifier: GPL-3.0-or-later

using FiveStar.Shaders.Dxil.Hlsl;
using FiveStar.Shaders.Dxil.Ir;
using FiveStar.Shaders.Dxil.Ops;

namespace FiveStar.Shaders.Dxil.Lowering;

/// <summary>
/// (RW)ByteAddressBuffer methods: Load/Load2/3/4 and Store/Store2/3/4 as rawBufferLoad/Store of i32, the Interlocked
/// methods as atomicBinOp / atomicCompareExchange, GetDimensions as getDimensions.
/// </summary>
public sealed class RawBufferLowerer
{
    private const int MaxDwords = 4;
    private const int ValueArgument = 2;
    private const string DimensionsClass = "getDimensions";

    private static readonly Dictionary<string, AtomicCode> Atomics = new(StringComparer.Ordinal)
    {
        ["InterlockedAdd"] = AtomicCode.Add,
        ["InterlockedAnd"] = AtomicCode.And,
        ["InterlockedOr"] = AtomicCode.Or,
        ["InterlockedXor"] = AtomicCode.Xor,
        ["InterlockedMin"] = AtomicCode.UMin,
        ["InterlockedMax"] = AtomicCode.UMax,
        ["InterlockedExchange"] = AtomicCode.Exchange,
    };

    private readonly LoweringContext _context;

    public RawBufferLowerer(LoweringContext context)
    {
        _context = context;
    }

    /// <summary>DXIL atomic operation codes (DXIL::AtomicBinOpCode).</summary>
    public enum AtomicCode
    {
        Add = 0,
        And = 1,
        Or = 2,
        Xor = 3,
        IMin = 4,
        IMax = 5,
        UMin = 6,
        UMax = 7,
        Exchange = 8,
    }

    private DxilModule Module => _context.Module;

    private ExpressionLowerer Expressions => _context.Expressions;

    public RValue Method(SyntaxNode call, Value handle)
    {
        var name = call.Text;

        if (name.StartsWith("Load", StringComparison.Ordinal))
        {
            return Load(call, handle, Width(name, "Load"));
        }

        if (name.StartsWith("Store", StringComparison.Ordinal))
        {
            Store(call, handle, Width(name, "Store"));

            return RValue.Void;
        }

        if (name == "InterlockedCompareExchange")
        {
            CompareExchange(call, handle);

            return RValue.Void;
        }

        if (Atomics.TryGetValue(name, out var code))
        {
            Atomic(call, handle, code);

            return RValue.Void;
        }

        if (name == "GetDimensions")
        {
            var dimensions = _context.Ops.Call(DxOpCode.GetDimensions, DimensionsClass, AttributeSet.ReadOnly, _context.Ops.Dimensions, handle, Module.Undefined(Module.Types.Int32));
            _context.Variables.Write(Expressions.Place(call[1]), RValue.Of(HlslType.Uint, _context.Builder.Extract(dimensions, 0)));

            return RValue.Void;
        }

        throw SourceError.At(call, $"ByteAddressBuffer.{name} is not supported");
    }

    private static int Width(string name, string prefix)
    {
        if (name.Length == prefix.Length)
        {
            return 1;
        }

        var digit = name[prefix.Length] - '0';

        if (digit is < 1 or > MaxDwords)
        {
            throw new DxilBackendException($"{name} is not supported");
        }

        return digit;
    }

    private RValue Load(SyntaxNode call, Value handle, int count)
    {
        var offset = Expressions.Index(call[1]);
        var values = _context.Ops.RawLoad(handle, offset, count);

        return new RValue(HlslType.Vector(ScalarKind.Uint, count), values);
    }

    private void Store(SyntaxNode call, Value handle, int count)
    {
        var offset = Expressions.Index(call[1]);
        var value = Expressions.Lower(call[ValueArgument], HlslType.Vector(ScalarKind.Uint, count));
        _context.Ops.RawStore(handle, offset, value.Components);
    }

    private void Atomic(SyntaxNode call, Value handle, AtomicCode code)
    {
        var offset = Expressions.Index(call[1]);
        var argument = Expressions.Lower(call[ValueArgument]);

        if (argument.Type.Kind == ScalarKind.Int && code is AtomicCode.UMin or AtomicCode.UMax)
        {
            code = Choice.Of(code == AtomicCode.UMin, AtomicCode.IMin, AtomicCode.IMax);
        }

        var value = _context.Convert.Convert(argument, HlslType.Uint).Scalar;
        var undefined = Module.Undefined(Module.Types.Int32);
        var original = _context.Ops.Overloaded(
            DxOpCode.AtomicBinOp, "atomicBinOp", AttributeSet.NoUnwind, Module.Types.Int32, Module.Types.Int32,
            handle, Module.Int32((uint)code), offset, undefined, undefined, value);

        if (call.Children.Count > MaxDwords - 1)
        {
            _context.Variables.Write(Expressions.Place(call[MaxDwords - 1]), RValue.Of(HlslType.Uint, original));
        }
    }

    private void CompareExchange(SyntaxNode call, Value handle)
    {
        var offset = Expressions.Index(call[1]);
        var compare = Expressions.Lower(call[ValueArgument], HlslType.Uint).Scalar;
        var value = Expressions.Lower(call[MaxDwords - 1], HlslType.Uint).Scalar;
        var undefined = Module.Undefined(Module.Types.Int32);
        var original = _context.Ops.Overloaded(
            DxOpCode.AtomicCompareExchange, "atomicCompareExchange", AttributeSet.NoUnwind, Module.Types.Int32, Module.Types.Int32,
            handle, offset, undefined, undefined, compare, value);
        _context.Variables.Write(Expressions.Place(call[MaxDwords]), RValue.Of(HlslType.Uint, original));
    }
}
