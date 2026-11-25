// Copyright (c) 2026 Five Star Recomp contributors.
// SPDX-License-Identifier: GPL-3.0-or-later

using FiveStar.Shaders.Dxil.Hlsl;
using FiveStar.Shaders.Dxil.Ir;
using FiveStar.Shaders.Dxil.Ops;

namespace FiveStar.Shaders.Dxil.Lowering;

/// <summary>
/// Texture methods as DXIL operations: Load, Sample, SampleLevel, SampleBias, SampleGrad, the comparison samples,
/// Gather and GatherCmp per channel, GetDimensions, and element reads and writes of RW textures. A method call's
/// children are the texture, then the arguments.
/// </summary>
public sealed class TextureLowerer
{
    private const int MaxCoordinates = 4;
    private const int MaxOffsets = 3;
    private const int SamplerArgument = 1;
    private const int LocationArgument = 2;
    private const int AfterLocation = 3;
    private const int LoadOffsetArgument = 2;
    private const int LevelsField = 3;
    private const int CompareAndLevel = 2;
    private const string GatherPrefix = "Gather";
    private const string GatherComparePrefix = "GatherCmp";

    private static readonly Dictionary<string, (int Coordinates, int Offsets)> Shapes = new(StringComparer.Ordinal)
    {
        ["Texture1D"] = (1, 1),
        ["Texture1DArray"] = (2, 1),
        ["Texture2D"] = (2, 2),
        ["Texture2DArray"] = (3, 2),
        ["Texture2DMS"] = (2, 2),
        ["Texture2DMSArray"] = (3, 2),
        ["Texture3D"] = (3, 3),
        ["TextureCube"] = (3, 0),
        ["TextureCubeArray"] = (4, 0),
        ["Buffer"] = (1, 0),
    };

    private static readonly string[] Channels = ["Red", "Green", "Blue", "Alpha"];

    private readonly LoweringContext _context;

    public TextureLowerer(LoweringContext context)
    {
        _context = context;
    }

    private IrBuilder Builder => _context.Builder;

    private DxilModule Module => _context.Module;

    private ExpressionLowerer Expressions => _context.Expressions;

    public RValue Method(SyntaxNode call, RValue target)
    {
        var name = call.Text;
        var texture = new TextureCall(_context, call, target, false);

        return name switch
        {
            "Load" => Load(texture, Expressions.Lower(call[1]), 1),
            "Sample" => Sample(texture, DxOpCode.Sample, "sample", 0),
            "SampleLevel" => Sample(texture, DxOpCode.SampleLevel, "sampleLevel", 1),
            "SampleBias" => Sample(texture, DxOpCode.SampleBias, "sampleBias", 1),
            "SampleGrad" => SampleGrad(texture),
            "SampleCmp" => Sample(texture, DxOpCode.SampleCmp, "sampleCmp", 1),
            "SampleCmpLevelZero" => Sample(texture, DxOpCode.SampleCmpLevelZero, "sampleCmpLevelZero", 1),
            "SampleCmpLevel" => Sample(texture, DxOpCode.SampleCmpLevel, "sampleCmpLevel", CompareAndLevel),
            "CalculateLevelOfDetail" => LevelOfDetail(texture, clamped: true),
            "CalculateLevelOfDetailUnclamped" => LevelOfDetail(texture, clamped: false),
            "GetDimensions" => Dimensions(texture),
            _ when name.StartsWith(GatherPrefix, StringComparison.Ordinal) => Gather(texture, name),
            _ => throw SourceError.At(call, $"{target.Type.Name}.{name} is not supported"),
        };
    }

    /// <summary><c>texture[index]</c> read: a texel load of mip 0 (or of the UAV).</summary>
    public RValue ReadElement(SyntaxNode node, RValue target)
    {
        var texture = new TextureCall(_context, node, target, false);

        return Load(texture, Expressions.Lower(node[1]), 0);
    }

    /// <summary><c>texture[index] = value</c>: a texel store of the components the texture holds.</summary>
    public void WriteElement(SyntaxNode node, RValue target, RValue value)
    {
        var texture = new TextureCall(_context, node, target, false);
        var coordinates = Integers(Expressions.Lower(node[1]), texture.Shape.Coordinates);
        var data = _context.Convert.Convert(value, texture.Element);
        var components = Pad(data.Components, Module.Undefined(texture.ScalarType), MaxCoordinates);
        var mask = (byte)((1 << texture.Element.Count) - 1);
        texture.Ops.Overloaded(
            DxOpCode.TextureStore, "textureStore", AttributeSet.NoUnwind, texture.ScalarType, Module.Types.Void,
            [texture.Handle, coordinates[0], coordinates[1], coordinates[LevelsField - 1], .. components, Module.Int8(mask)]);
    }

    /// <summary>
    /// An Interlocked call on a texel (<c>InterlockedMax(texture[uv], value[, original])</c>,
    /// <c>InterlockedCompareExchange(texture[uv], compare, value, original)</c>).
    /// </summary>
    public RValue Atomic(SyntaxNode call, RValue target)
    {
        var element = call[0];
        var texture = new TextureCall(_context, element, target, true);
        var coordinates = Integers(Expressions.Lower(element[1]), texture.Shape.Coordinates);
        var int32 = Module.Types.Int32;

        if (call.Text is "InterlockedCompareExchange" or "InterlockedCompareStore")
        {
            var compare = Expressions.Lower(call[1], HlslType.Uint).Scalar;
            var exchanged = Expressions.Lower(call[LocationArgument], HlslType.Uint).Scalar;
            var old = texture.Ops.Overloaded(
                DxOpCode.AtomicCompareExchange, "atomicCompareExchange", AttributeSet.NoUnwind, int32, int32,
                texture.Handle, coordinates[0], coordinates[1], coordinates[LevelsField - 1], compare, exchanged);
            WriteOriginal(call, AfterLocation, old);

            return RValue.Void;
        }

        var argument = Expressions.Lower(call[1]);
        var wide = texture.Element.Kind is ScalarKind.Int64 or ScalarKind.Uint64;
        var code = AtomicCode(call, Choice.Of(wide, texture.Element.Kind, argument.Type.Kind));
        var valueType = Choice.Of(wide, HlslType.Uint64, HlslType.Uint);
        var overload = Choice.Of(wide, Module.Types.Int64, int32);

        var value = _context.Convert.Convert(argument, valueType).Scalar;
        var original = texture.Ops.Overloaded(
            DxOpCode.AtomicBinOp, "atomicBinOp", AttributeSet.NoUnwind, overload, overload,
            texture.Handle, Module.Int32((uint)code), coordinates[0], coordinates[1], coordinates[LevelsField - 1], value);
        WriteOriginal(call, LocationArgument, RValue.Of(valueType, original));

        return RValue.Void;
    }

    private void WriteOriginal(SyntaxNode call, int index, Value original)
    {
        WriteOriginal(call, index, RValue.Of(HlslType.Uint, original));
    }

    private void WriteOriginal(SyntaxNode call, int index, RValue original)
    {
        if (call.Children.Count > index)
        {
            _context.Variables.Write(Expressions.Place(call[index]), original);
        }
    }

    private static RawBufferLowerer.AtomicCode AtomicCode(SyntaxNode call, ScalarKind kind)
    {
        var signed = kind is ScalarKind.Int or ScalarKind.Int64;

        return call.Text switch
        {
            "InterlockedAdd" => RawBufferLowerer.AtomicCode.Add,
            "InterlockedAnd" => RawBufferLowerer.AtomicCode.And,
            "InterlockedOr" => RawBufferLowerer.AtomicCode.Or,
            "InterlockedXor" => RawBufferLowerer.AtomicCode.Xor,
            "InterlockedMin" => Choice.Of(signed, RawBufferLowerer.AtomicCode.IMin, RawBufferLowerer.AtomicCode.UMin),
            "InterlockedMax" => Choice.Of(signed, RawBufferLowerer.AtomicCode.IMax, RawBufferLowerer.AtomicCode.UMax),
            "InterlockedExchange" => RawBufferLowerer.AtomicCode.Exchange,
            _ => throw SourceError.At(call, $"{call.Text} on a texture is not supported"),
        };
    }

    /// <summary>A texel load: the coordinates, then (when <paramref name="mipLast"/> is 1) the mip level.</summary>
    private RValue Load(TextureCall texture, RValue address, int mipLast)
    {
        var count = texture.Shape.Coordinates;
        var integers = _context.Convert.Convert(address, HlslType.Vector(ScalarKind.Int, count + mipLast));
        var coordinates = Integers(integers, count);
        Value mip = Module.Undefined(Module.Types.Int32);

        if (mipLast == 1 && !texture.IsUav)
        {
            mip = integers.Components[count];
        }

        var offsets = OffsetArguments(texture, Argument(texture.Call, LoadOffsetArgument));
        var result = texture.Ops.Overloaded(
            DxOpCode.TextureLoad, "textureLoad", AttributeSet.ReadOnly, texture.ScalarType, texture.ResultType,
            [texture.Handle, mip, coordinates[0], coordinates[1], coordinates[LevelsField - 1], .. offsets]);

        return texture.Result(result);
    }

    /// <summary>
    /// Sample, SampleLevel, SampleBias, SampleCmp, SampleCmpLevelZero: sampler, location, then <paramref name="values"/>
    /// value arguments (lod, bias or compare value), then an optional offset.
    /// </summary>
    private RValue Sample(TextureCall texture, DxOpCode code, string name, int values)
    {
        var call = texture.Call;
        var sampler = Sampler(call[SamplerArgument]);
        var coordinates = Location(texture);
        var tail = new List<Value>();

        for (var index = 0; index < values; index++)
        {
            tail.Add(Expressions.Lower(call[AfterLocation + index], HlslType.Float).Scalar);
        }

        var offsets = OffsetArguments(texture, Argument(call, AfterLocation + values));
        List<Value> arguments = [texture.Handle, sampler, .. coordinates, .. offsets, .. tail];

        if (code is DxOpCode.Sample or DxOpCode.SampleBias or DxOpCode.SampleCmp)
        {
            // The clamp argument.
            arguments.Add(Module.Undefined(Module.Types.Float));
        }

        var result = texture.Ops.Overloaded(code, name, AttributeSet.ReadOnly, Module.Types.Float, texture.Ops.FloatResult, [.. arguments]);

        return texture.Result(result);
    }

    /// <summary>CalculateLevelOfDetail(Unclamped)(sampler, location): the mip level a sample there would use.</summary>
    private RValue LevelOfDetail(TextureCall texture, bool clamped)
    {
        var sampler = Sampler(texture.Call[SamplerArgument]);

        // The location without an array index: a cube's direction, else the texture's dimensions.
        var dimensions = texture.Shape.Offsets > 0 ? texture.Shape.Offsets : MaxOffsets;
        var location = Expressions.Lower(texture.Call[LocationArgument], HlslType.Vector(ScalarKind.Float, dimensions));
        var coordinates = Pad(location.Components, Module.Undefined(Module.Types.Float), MaxOffsets);
        var level = texture.Ops.Call(
            DxOpCode.CalculateLod, "calculateLOD.f32", AttributeSet.ReadOnly, Module.Types.Float,
            [texture.Handle, sampler, .. coordinates, Module.Bool(clamped)]);

        return RValue.Of(HlslType.Float, level);
    }

    /// <summary>SampleGrad: sampler, location, ddx, ddy, then an optional offset.</summary>
    private RValue SampleGrad(TextureCall texture)
    {
        var call = texture.Call;
        var sampler = Sampler(call[SamplerArgument]);
        var coordinates = Location(texture);
        var spatial = Math.Min(texture.Shape.Coordinates, MaxOffsets);

        if (texture.Shape.Offsets > 0)
        {
            spatial = texture.Shape.Offsets;
        }

        var x = Pad(Expressions.Lower(call[AfterLocation], HlslType.Vector(ScalarKind.Float, spatial)).Components, Module.Undefined(Module.Types.Float), MaxOffsets);
        var y = Pad(Expressions.Lower(call[AfterLocation + 1], HlslType.Vector(ScalarKind.Float, spatial)).Components, Module.Undefined(Module.Types.Float), MaxOffsets);
        var offsets = OffsetArguments(texture, Argument(call, AfterLocation + sizeof(short)));
        var result = texture.Ops.Overloaded(
            DxOpCode.SampleGrad, "sampleGrad", AttributeSet.ReadOnly, Module.Types.Float, texture.Ops.FloatResult,
            [texture.Handle, sampler, .. coordinates, .. offsets, .. x, .. y, Module.Undefined(Module.Types.Float)]);

        return texture.Result(result);
    }

    /// <summary>Gather/GatherRed..Alpha (sampler, location[, offset]) and GatherCmp* (sampler, location, compare[, offset]).</summary>
    private RValue Gather(TextureCall texture, string name)
    {
        var call = texture.Call;
        var compare = name.StartsWith(GatherComparePrefix, StringComparison.Ordinal);
        var prefix = Choice.Of(compare, GatherComparePrefix, GatherPrefix);
        var channel = Math.Max(0, Array.IndexOf(Channels, name[prefix.Length..]));
        var sampler = Sampler(call[SamplerArgument]);
        var coordinates = Location(texture);

        var next = AfterLocation;
        Value? compareValue = null;

        if (compare)
        {
            compareValue = Expressions.Lower(call[next], HlslType.Float).Scalar;
            next++;
        }

        var offsets = OffsetArguments(texture, Argument(call, next)).Take(sizeof(short));
        List<Value> arguments = [texture.Handle, sampler, .. coordinates, .. offsets, Module.Int32((uint)channel)];
        var code = DxOpCode.TextureGather;
        var className = "textureGather";

        if (compareValue is not null)
        {
            arguments.Add(compareValue);
            code = DxOpCode.TextureGatherCmp;
            className = "textureGatherCmp";
        }

        var result = texture.Ops.Overloaded(code, className, AttributeSet.ReadOnly, texture.ScalarType, texture.ResultType, [.. arguments]);

        return texture.Result(result);
    }

    /// <summary>GetDimensions([mip,] width[, height[, depth or elements]][, levels]).</summary>
    private RValue Dimensions(TextureCall texture)
    {
        var outputs = texture.Call.Children.Skip(1).ToList();
        Value mip = Module.Undefined(Module.Types.Int32);
        var hasLevels = !texture.IsUav && outputs.Count > texture.Shape.Coordinates;

        if (hasLevels && outputs.Count > texture.Shape.Coordinates + 1)
        {
            mip = Expressions.Index(outputs[0]);
            outputs.RemoveAt(0);
        }

        var dimensions = texture.Ops.Call(DxOpCode.GetDimensions, "getDimensions", AttributeSet.ReadOnly, texture.Ops.Dimensions, texture.Handle, mip);

        for (var index = 0; index < outputs.Count; index++)
        {
            var field = index;

            if (hasLevels && index == outputs.Count - 1)
            {
                field = LevelsField;
            }

            var place = Expressions.Place(outputs[index]);
            _context.Variables.Write(place, RValue.Of(HlslType.Uint, Builder.Extract(dimensions, field)));
        }

        return RValue.Void;
    }

    private static SyntaxNode? Argument(SyntaxNode call, int index)
    {
        if (index < call.Children.Count)
        {
            return call[index];
        }

        return null;
    }

    private Value[] Location(TextureCall texture)
    {
        var location = Expressions.Lower(texture.Call[LocationArgument], HlslType.Vector(ScalarKind.Float, texture.Shape.Coordinates));

        return Pad(location.Components, Module.Undefined(Module.Types.Float), MaxCoordinates);
    }

    private Value Sampler(SyntaxNode node)
    {
        var value = Expressions.Lower(node);

        if (value.Type.Shape != TypeShape.Resource)
        {
            throw SourceError.At(node, "expected a sampler");
        }

        return value.Scalar;
    }

    /// <summary>Three offset arguments: the written ones, the rest undefined.</summary>
    private Value[] OffsetArguments(TextureCall texture, SyntaxNode? node)
    {
        var undefined = Module.Undefined(Module.Types.Int32);

        if (node is null || texture.Shape.Offsets == 0)
        {
            return Pad([], undefined, MaxOffsets);
        }

        var value = Expressions.Lower(node, HlslType.Vector(ScalarKind.Int, texture.Shape.Offsets));

        return Pad(value.Components, undefined, MaxOffsets);
    }

    /// <summary>The first <paramref name="count"/> components as i32, padded to three with undefined.</summary>
    private Value[] Integers(RValue value, int count)
    {
        var integers = _context.Convert.Convert(value, value.Type.WithKind(ScalarKind.Int));

        return Pad([.. integers.Components.Take(count)], Module.Undefined(Module.Types.Int32), MaxOffsets);
    }

    private static Value[] Pad(IReadOnlyList<Value> components, Value undefined, int length)
    {
        var result = new Value[length];

        for (var index = 0; index < length; index++)
        {
            result[index] = undefined;

            if (index < components.Count)
            {
                result[index] = components[index];
            }
        }

        return result;
    }

    /// <summary>The parts of one texture call: its handle, shape, element type and result struct.</summary>
    private sealed class TextureCall
    {
        private readonly IrBuilder _builder;

        /// <param name="context">The lowering context.</param>
        /// <param name="call">The call, for errors.</param>
        /// <param name="target">The texture.</param>
        /// <param name="atomic">An Interlocked call: 64-bit texels are allowed.</param>
        public TextureCall(LoweringContext context, SyntaxNode call, RValue target, bool atomic)
        {
            Call = call;
            Ops = context.Ops;
            Handle = target.Scalar;
            IsUav = ResourceProperties.IsUav(target.Type.Name);

            if (!Shapes.TryGetValue(ResourceProperties.BaseName(target.Type.Name), out var shape))
            {
                throw SourceError.At(call, $"{target.Type.Name} is not supported");
            }

            Shape = shape;
            Element = target.Type.Element ?? HlslType.Vector(ScalarKind.Float, MaxCoordinates);

            // 64-bit texels are lowered only for atomics (InterlockedMax of a depth and an id in the alpha prepasses).
            if (Element.Kind is ScalarKind.Int64 or ScalarKind.Uint64 && !atomic)
            {
                throw SourceError.At(call, $"{target.Type.Name} of 64-bit elements is not supported");
            }

            var floating = Conversions.IsFloat(Element.Kind);
            ScalarType = Choice.Of(floating, context.Module.Types.Float, context.Module.Types.Int32);
            ResultType = Choice.Of(floating, context.Ops.FloatResult, context.Ops.Int32Result);
            _builder = context.Builder;
        }

        public SyntaxNode Call { get; }

        public DxOps Ops { get; }

        public Value Handle { get; }

        public bool IsUav { get; }

        public (int Coordinates, int Offsets) Shape { get; }

        public HlslType Element { get; }

        public IrType ScalarType { get; }

        public IrType ResultType { get; }

        /// <summary>The element's components of a ResRet result.</summary>
        public RValue Result(Value result)
        {
            var kind = Choice.Of(Conversions.IsFloat(Element.Kind), ScalarKind.Float, Element.Kind);
            var type = HlslType.Vector(kind, Element.Count);

            return new RValue(type, [.. Enumerable.Range(0, Element.Count).Select(index => _builder.Extract(result, index))]);
        }
    }
}
