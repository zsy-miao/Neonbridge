// Copyright (c) 2026 Five Star Recomp contributors.
// SPDX-License-Identifier: GPL-3.0-or-later

using FiveStar.Shaders.Translation.Core;
using static FiveStar.Shaders.Translation.Emitters.Operands;

namespace FiveStar.Shaders.Translation.Emitters;

/// <summary>
/// MIMG instructions: sample, gather, load, store, get_resinfo, get_lod and the BVH ray intersection. Resources are
/// bindless: T# dword0 indexes ResourceDescriptorHeap, S# dword0 SamplerDescriptorHeap (literal S#s get static
/// sampler slots). dmask selects the returned components; d16 packs them as f16 pairs. With typed image views
/// (<see cref="TranslationOptions.TypedImageViews"/>) a load, gather or store of an integer view (the class in T# dword 3)
/// goes through a <c>uint4</c> or <c>int4</c> declaration of the same descriptor, and d16 keeps an integer's low 16 bits.
/// </summary>
internal sealed class ImageEmitter
{
    private const int AddressOperand = 1;
    private const int ResourceOperand = 2;
    private const int SamplerOperand = 3;
    private const int AllComponents = 0xf;
    private const int Vec4 = 4;
    private const string ComponentNames = "xyzw";

    /// <summary>The T# dword the loader writes the view's component class to, its bits (0 float, 1 uint, 2 sint) and the class values.</summary>
    private const int ClassDword = 3;
    private const string ClassMask = "3u";
    private const string UintClass = "1u";
    private const string SintClass = "2u";

    // _o variants: the first address dword packs 6-bit signed texel offsets at bits 0, 8 and 16.
    private const int OffsetFieldTop = 26;
    private const int OffsetFieldStride = 8;
    private const int CubeFaceDerivatives = 4;

    private readonly TranslationState _state;
    private readonly OperandAccess _operands;
    private readonly ImageSampleBuilder _samples;
    private readonly BvhEmitter _bvh;

    public ImageEmitter(TranslationState state, OperandAccess operands, ImageSampleBuilder samples, BvhEmitter bvh)
    {
        _state = state;
        _operands = operands;
        _samples = samples;
        _bvh = bvh;
    }

    public void Translate(string op, string operandText, InstructionModifiers modifiers)
    {
        var arguments = ArgumentList.Split(operandText);

        if (op.StartsWith("image_bvh_intersect_ray", StringComparison.Ordinal) || op.StartsWith("image_bvh64_intersect_ray", StringComparison.Ordinal))
        {
            _bvh.Translate(op, arguments, modifiers);

            return;
        }

        var destination = OperandParser.Parse(arguments[Destination]);
        var coordinates = ImageDimensions.ReadAddress(_operands, arguments[AddressOperand]);
        var resource = OperandParser.Parse(arguments[ResourceOperand]);
        var sampler = arguments.Count > SamplerOperand ? OperandParser.Parse(arguments[SamplerOperand]) : null;

        var dimension = ImageDimensions.Name(modifiers);
        var (type, components) = ImageDimensions.Sampled(dimension) ?? throw new UnsupportedProgramException($"image dim {dimension}");
        var isCube = dimension == "CUBE";
        var isStore = op.StartsWith("image_store", StringComparison.Ordinal);

        if (isCube && (isStore || op.StartsWith("image_load", StringComparison.Ordinal)))
        {
            throw new UnsupportedProgramException($"{op} on cube");
        }

        var number = _state.NextTemporary();

        if (isStore)
        {
            Store(op, destination, modifiers, (type, components), resource, coordinates);

            return;
        }

        var request = Prepare(op, modifiers, (type, components, dimension), coordinates, number);
        request = request with { SamplerOperand = sampler };
        Declare(request, resource, sampler);
        Complete(op, request, destination, modifiers, resource);
    }

    private void Store(string op, Operand data, InstructionModifiers modifiers, (string Type, int Components) texture, Operand resource, List<string> coordinates)
    {
        var heapIndex = _operands.Read(resource, 0);

        if (modifiers.Has("a16"))
        {
            coordinates = coordinates.SelectMany(value => (string[])[$"({value} & 0xffffu)", $"({value} >> 16)"]).ToList();
        }

        if ((op != "image_store" && op != "image_store_mip") || modifiers.Has("d16"))
        {
            throw new UnsupportedProgramException(op);
        }

        if (coordinates.Count < texture.Components)
        {
            throw new UnsupportedProgramException($"{op} address components");
        }

        var mask = modifiers.Hex("dmask", AllComponents);
        var raw = new List<string>();
        var next = 0;

        for (var component = 0; component < Vec4; component++)
        {
            if (((mask >> component) & 1) != 0)
            {
                raw.Add(_operands.Read(data, next));
                next++;
            }
            else
            {
                raw.Add("0u");
            }
        }

        var floats = string.Join(", ", raw.Select(value => $"asfloat({value})"));
        var slot = NullTextureSlots.Slot("RW" + texture.Type, heapIndex);
        var name = $"_uav{_state.NextTemporary()}";
        _state.Emit($"  {{ RW{texture.Type}<float4> {name} = ResourceDescriptorHeap[{slot}];");
        var index = ImageDimensions.IntegerCoordinates(coordinates, texture.Components);

        if (!_state.TypedImageViews)
        {
            _state.Emit($"    if (active) {name}[{index}] = float4({floats}); }}");

            return;
        }

        // An integer view takes the registers' bits: through a float4 declaration the typed store would convert them.
        var bits = $"uint4({string.Join(", ", raw)})";
        _state.Emit($"    RW{texture.Type}<uint4> {name}u = ResourceDescriptorHeap[{slot}]; RW{texture.Type}<int4> {name}i = ResourceDescriptorHeap[{slot}];");
        _state.Emit($"    uint {name}c = {ClassOf(resource)};");
        _state.Emit($"    if (active && {name}c == {UintClass}) {name}u[{index}] = {bits}; else if (active && {name}c == {SintClass}) {name}i[{index}] = asint({bits}); else if (active) {name}[{index}] = float4({floats}); }}");
    }

    /// <summary>The view's component class from T# dword 3 (<see cref="TranslationOptions.TypedImageViews"/>).</summary>
    private string ClassOf(Operand resource)
    {
        return $"({_operands.Read(resource, ClassDword)} & {ClassMask})";
    }

    private static ImageRequest Prepare(
        string op, InstructionModifiers modifiers, (string Type, int Components, string Dimension) texture, List<string> coordinates, int number)
    {
        var operation = op.Replace("image_", string.Empty, StringComparison.Ordinal);

        if (modifiers.Has("d16") && (operation.StartsWith("get_resinfo", StringComparison.Ordinal) || operation.StartsWith("gather", StringComparison.Ordinal)))
        {
            throw new UnsupportedProgramException($"{op} d16");
        }

        var hasOffsets = operation.EndsWith("_o", StringComparison.Ordinal);

        if (hasOffsets)
        {
            operation = operation[..^"_o".Length];
        }

        var hasClamp = operation.Contains("_cl", StringComparison.Ordinal);

        if (hasClamp)
        {
            operation = operation.Replace("_cl", string.Empty, StringComparison.Ordinal);
        }

        if (hasClamp && modifiers.Has("a16"))
        {
            throw new UnsupportedProgramException($"{op} a16 + clamp");
        }

        var request = new ImageRequest
        {
            Operation = operation,
            Dimension = texture.Dimension,
            TextureType = texture.Type,
            Components = texture.Components,
            IsCube = texture.Dimension == "CUBE",
            Coordinates = coordinates,
            HasBias = operation.Contains("_b", StringComparison.Ordinal),
            HasCompare = operation.Contains("_c", StringComparison.Ordinal) && operation != "get_resinfo",
            HasClamp = hasClamp,
            Derivatives = 0,
            Texture = $"_tex{number}",
            Number = number,
        };

        request = request with { Derivatives = DerivativeCount(request) };

        if (hasOffsets)
        {
            request = TakeOffsets(request);
        }

        if (modifiers.Has("a16"))
        {
            request = request with { Coordinates = UnpackHalfAddress(request) };
        }

        return request;
    }

    private static int DerivativeCount(ImageRequest request)
    {
        if (!request.Operation.StartsWith("sample_d", StringComparison.Ordinal) && !request.Operation.StartsWith("sample_c_d", StringComparison.Ordinal))
        {
            return 0;
        }

        if (request.IsCube)
        {
            // Cube derivatives are 2D in face space: ds/dx, dt/dx, ds/dy, dt/dy.
            return CubeFaceDerivatives;
        }

        return PairDwords * request.SpatialComponents;
    }

    /// <summary>Takes the packed texel offsets off the front of the address.</summary>
    private static ImageRequest TakeOffsets(ImageRequest request)
    {
        var packed = request.Coordinates[0];
        var spatial = request.SpatialComponents;
        var fields = Enumerable.Range(0, spatial).Select(axis => $"(asint({packed} << {OffsetFieldTop - (OffsetFieldStride * axis)}) >> {OffsetFieldTop})").ToList();
        var offsets = fields[0];

        if (spatial > 1)
        {
            offsets = $"int{spatial}({string.Join(", ", fields)})";
        }

        return request with { Coordinates = request.Coordinates.Skip(1).ToList(), Offsets = offsets };
    }

    /// <summary>a16: 16-bit address components come two per VGPR (bias and compare keep their own dword).</summary>
    private static List<string> UnpackHalfAddress(ImageRequest request)
    {
        var unpacked = new List<string>();
        var next = 0;

        if (request.HasBias)
        {
            unpacked.Add($"asuint(f16tof32({request.Coordinates[next]}))");
            next++;
        }

        if (request.HasCompare)
        {
            unpacked.Add(request.Coordinates[next]);
            next++;
        }

        var integers = request.Operation.StartsWith("load", StringComparison.Ordinal) || request.Operation.StartsWith("get_resinfo", StringComparison.Ordinal);

        foreach (var value in request.Coordinates.Skip(next))
        {
            if (integers)
            {
                unpacked.Add($"({value} & 0xffffu)");
                unpacked.Add($"({value} >> 16)");
            }
            else
            {
                unpacked.Add($"asuint(f16tof32({value}))");
                unpacked.Add($"asuint(f16tof32({value} >> 16))");
            }
        }

        return unpacked;
    }

    private void Declare(ImageRequest request, Operand resource, Operand? sampler)
    {
        _state.Emit($"  {request.TextureType}<float4> {request.Texture} = ResourceDescriptorHeap[{NullTextureSlots.Slot(request.TextureType, _operands.Read(resource, 0))}];");

        if (sampler is not null && !request.HasCompare)
        {
            _state.Emit($"  SamplerState _smp{request.Number} = SamplerDescriptorHeap[sampler_slot({_operands.SamplerIndex(sampler, comparison: false)})];");
        }
    }

    private void Complete(string op, ImageRequest request, Operand destination, InstructionModifiers modifiers, Operand resource)
    {
        if (request.SamplerOperand is not null && !request.HasCompare)
        {
            request = request with { Sampler = $"_smp{request.Number}" };
        }

        var mask = modifiers.Hex("dmask", AllComponents);

        if (request.Operation == "get_resinfo")
        {
            ResourceInfo(request, destination, mask);

            return;
        }

        var asked = mask;
        var call = _samples.Call(op, request, ref mask) ?? throw new UnsupportedProgramException(op);
        var result = $"_img{request.Number}";
        string? integer = null;

        if (Typed(request))
        {
            integer = $"_cls{request.Number}";
            EmitTyped(op, request, resource, asked, call, result, integer);
        }
        else
        {
            _state.Emit($"  float4 {result} = {call};");
        }

        WriteResult(destination, result, mask, modifiers.Has("d16"), integer);
    }

    /// <summary>
    /// Whether this read follows the view's class (<see cref="TranslationOptions.TypedImageViews"/>): loads and plain
    /// gathers, which return the texel's values; a sample filters, a compare gather compares, and a gather at a level is
    /// assembled from float loads.
    /// </summary>
    private bool Typed(ImageRequest request)
    {
        if (!_state.TypedImageViews)
        {
            return false;
        }

        if (request.Operation is "load" or "load_mip")
        {
            return true;
        }

        bool gather = request.Operation.StartsWith("gather4", StringComparison.Ordinal) && !request.HasCompare;

        return gather && !(request.Operation == "gather4_l" && request.Dimension is "2D" or "2D_ARRAY");
    }

    /// <summary>The read through a uint4 or int4 declaration of the descriptor for an integer view, its bits in <paramref name="result"/>.</summary>
    private void EmitTyped(string op, ImageRequest request, Operand resource, int mask, string call, string result, string integer)
    {
        var slot = NullTextureSlots.Slot(request.TextureType, _operands.Read(resource, 0));
        var unsigned = request with { Texture = $"{request.Texture}u" };
        var signed = request with { Texture = $"{request.Texture}i" };

        int unsignedMask = mask;
        int signedMask = mask;
        var unsignedCall = _samples.Call(op, unsigned, ref unsignedMask);
        var signedCall = _samples.Call(op, signed, ref signedMask);

        _state.Emit($"  {request.TextureType}<uint4> {unsigned.Texture} = ResourceDescriptorHeap[{slot}]; {request.TextureType}<int4> {signed.Texture} = ResourceDescriptorHeap[{slot}];");
        _state.Emit($"  uint {integer} = {ClassOf(resource)};");
        _state.Emit($"  float4 {result};");
        _state.Emit($"  if ({integer} == {UintClass}) {result} = asfloat({unsignedCall}); else if ({integer} == {SintClass}) {result} = asfloat({signedCall}); else {result} = {call};");
    }

    private void ResourceInfo(ImageRequest request, Operand destination, int mask)
    {
        string[] allNames = ["_w", "_h", "_d"];

        var dimensions = request.Dimension switch
        {
            "1D" => 1,
            "2D" or "1D_ARRAY" => PairDwords,
            "3D" or "2D_ARRAY" => allNames.Length,
            _ => throw new KeyNotFoundException(request.Dimension),
        };
        var number = request.Number;
        var names = allNames.Take(dimensions).Select(name => $"{name}{number}").ToList();
        _state.Emit($"  uint {string.Join(", ", names)}, _lv{number};");
        _state.Emit($"  {request.Texture}.GetDimensions({request.Coordinates[0]}, {string.Join(", ", names)}, _lv{number});");

        var values = names.Concat(Enumerable.Repeat("0", allNames.Length - dimensions)).Append($"_lv{number}");
        var result = $"_img{number}";
        _state.Emit($"  uint4 {result} = uint4({string.Join(", ", values)});");
        var next = 0;

        for (var component = 0; component < Vec4; component++)
        {
            if (((mask >> component) & 1) != 0)
            {
                _operands.Write(destination, $"{result}.{ComponentNames[component]}", next);
                next++;
            }
        }
    }

    /// <summary>
    /// The components dmask selects; d16 packs two per register, as halves, or (<paramref name="integer"/> names the
    /// view's class when it is typed) as the low 16 bits of an integer view's values.
    /// </summary>
    private void WriteResult(Operand destination, string result, int mask, bool packedHalves, string? integer)
    {
        var components = Enumerable.Range(0, Vec4).Where(component => ((mask >> component) & 1) != 0).ToList();

        if (!packedHalves)
        {
            for (var index = 0; index < components.Count; index++)
            {
                _operands.Write(destination, $"asuint({result}.{ComponentNames[components[index]]})", index);
            }

            return;
        }

        for (var index = 0; index < components.Count; index += PairDwords)
        {
            var low = Half(result, components[index], integer);
            var high = "0u";

            if (index + 1 < components.Count)
            {
                high = $"({Half(result, components[index + 1], integer)} << 16)";
            }

            _operands.Write(destination, $"({low} | {high})", index / PairDwords);
        }
    }

    /// <summary>One d16 half: the float's f16, or for an integer view (class in <paramref name="integer"/>) the value's low 16 bits.</summary>
    private static string Half(string result, int component, string? integer)
    {
        var value = $"{result}.{ComponentNames[component]}";

        if (integer is null)
        {
            return $"f32tof16({value})";
        }

        return $"select({integer} != 0u, asuint({value}) & 0xffffu, f32tof16({value}))";
    }
}
