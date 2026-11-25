// Copyright (c) 2026 Five Star Recomp contributors.
// SPDX-License-Identifier: GPL-3.0-or-later

using FiveStar.Shaders.Dxil.Ir;

namespace FiveStar.Shaders.Dxil.Ops;

/// <summary>
/// Calls of DXIL operations with their declared names and overloads (<c>dx.op.&lt;class&gt;.&lt;overload&gt;</c>), and the
/// <c>dx.types</c> structs they use.
/// </summary>
public sealed class DxOps
{
    /// <summary>ResourceProperties dword0: the UAV bit.</summary>
    public const uint UavBit = 0x1000;

    /// <summary>DXIL resource kinds.</summary>
    public const uint RawBufferKind = 11;
    public const uint CbufferKind = 13;

    private const byte CbvClass = 2;
    private const byte FullMask = 0xF;
    private const uint DwordAlignment = 4;

    private readonly IrBuilder _builder;

    public DxOps(IrBuilder builder)
    {
        _builder = builder;
        var types = builder.Types;
        Handle = types.Struct("dx.types.Handle", [types.Pointer(types.Int8, 0)]);
        Properties = types.Struct("dx.types.ResourceProperties", [types.Int32, types.Int32]);
        Binding = types.Struct("dx.types.ResBind", [types.Int32, types.Int32, types.Int32, types.Int8]);
        CbufferResult = types.Struct("dx.types.CBufRet.i32", [types.Int32, types.Int32, types.Int32, types.Int32]);

        Int32Result = types.Struct("dx.types.ResRet.i32", [types.Int32, types.Int32, types.Int32, types.Int32, types.Int32]);
        FourInt32 = types.Struct("dx.types.fouri32", [types.Int32, types.Int32, types.Int32, types.Int32]);
        Dimensions = types.Struct("dx.types.Dimensions", [types.Int32, types.Int32, types.Int32, types.Int32]);
        FloatResult = types.Struct("dx.types.ResRet.f32", [types.Float, types.Float, types.Float, types.Float, types.Int32]);
    }

    public IrType Handle { get; }

    public IrType Properties { get; }

    public IrType Binding { get; }

    public IrType CbufferResult { get; }

    public IrType Int32Result { get; }

    public IrType FourInt32 { get; }

    public IrType Dimensions { get; }

    public IrType FloatResult { get; }

    private DxilModule Module => _builder.Module;

    private TypeTable Types => _builder.Types;

    /// <summary>The name suffix DXIL gives an overload type.</summary>
    public static string Overload(IrType type)
    {
        return type.Kind switch
        {
            IrTypeKind.Half => "f16",
            IrTypeKind.Float => "f32",
            IrTypeKind.Double => "f64",
            IrTypeKind.Void => "void",
            _ => $"i{type.Bits}",
        };
    }

    /// <summary>An operation of class <paramref name="className"/> with no overload in its name.</summary>
    public Value Call(DxOpCode opcode, string className, AttributeSet attribute, IrType result, params Value[] arguments)
    {
        return _builder.DxOp(className, (int)opcode, attribute, result, arguments);
    }

    /// <summary>An operation overloaded on <paramref name="overload"/>.</summary>
    public Value Overloaded(DxOpCode opcode, string className, AttributeSet attribute, IrType overload, IrType result, params Value[] arguments)
    {
        return _builder.DxOp($"{className}.{Overload(overload)}", (int)opcode, attribute, result, arguments);
    }

    /// <summary>A pure operation whose result is the overload type.</summary>
    public Value Pure(DxOpCode opcode, string className, IrType type, params Value[] arguments)
    {
        return Overloaded(opcode, className, AttributeSet.ReadNone, type, type, arguments);
    }

    public Value ThreadIdComponent(DxOpCode opcode, string className, int component)
    {
        return Overloaded(opcode, className, AttributeSet.ReadNone, Types.Int32, Types.Int32, Module.Int32((uint)component));
    }

    /// <summary>A descriptor-heap handle annotated with <paramref name="kind"/> (plus <see cref="UavBit"/>) and <paramref name="detail"/>.</summary>
    public Value HeapHandle(Value index, bool sampler, uint kind, uint detail)
    {
        var handle = Call(DxOpCode.CreateHandleFromHeap, "createHandleFromHeap", AttributeSet.ReadNone, Handle, index, Module.Bool(sampler), Module.Bool(false));

        return Annotate(handle, kind, detail);
    }

    public Value Annotate(Value handle, uint kind, uint detail)
    {
        var properties = Module.Aggregate(Properties, [Module.Int32(kind), Module.Int32(detail)]);

        return Call(DxOpCode.AnnotateHandle, "annotateHandle", AttributeSet.ReadNone, Handle, handle, properties);
    }

    /// <summary>The handle of the constant buffer at b0, space 0, of <paramref name="bytes"/> bytes.</summary>
    public Value UserDataHandle(uint bytes)
    {
        var zero = Module.Int32(0);
        var binding = Module.Aggregate(Binding, [zero, zero, zero, Module.Int8(CbvClass)]);
        var handle = Call(DxOpCode.CreateHandleFromBinding, "createHandleFromBinding", AttributeSet.ReadNone, Handle, binding, zero, Module.Bool(false));

        return Annotate(handle, CbufferKind, bytes);
    }

    public Value CbufferRow(Value handle, Value row)
    {
        return Overloaded(DxOpCode.CBufferLoadLegacy, "cbufferLoadLegacy", AttributeSet.ReadOnly, Types.Int32, CbufferResult, handle, row);
    }

    /// <summary>Loads <paramref name="count"/> dwords of a raw buffer at byte <paramref name="offset"/>.</summary>
    public IReadOnlyList<Value> RawLoad(Value handle, Value offset, int count)
    {
        var mask = (byte)((1 << count) - 1);
        var result = Overloaded(
            DxOpCode.RawBufferLoad, "rawBufferLoad", AttributeSet.ReadOnly, Types.Int32, Int32Result,
            handle, offset, Module.Undefined(Types.Int32), Module.Int8(mask), Module.Int32(DwordAlignment));

        return [.. Enumerable.Range(0, count).Select(component => _builder.Extract(result, component))];
    }

    /// <summary>Stores up to four dwords to a raw buffer at byte <paramref name="offset"/>.</summary>
    public void RawStore(Value handle, Value offset, IReadOnlyList<Value> values)
    {
        var undefined = Module.Undefined(Types.Int32);
        var data = new Value[sizeof(uint)];

        for (var component = 0; component < data.Length; component++)
        {
            data[component] = component < values.Count ? values[component] : undefined;
        }

        var mask = (byte)((1 << values.Count) - 1 & FullMask);
        Overloaded(
            DxOpCode.RawBufferStore, "rawBufferStore", AttributeSet.NoUnwind, Types.Int32, Types.Void,
            [handle, offset, undefined, .. data, Module.Int8(mask), Module.Int32(DwordAlignment)]);
    }
}
