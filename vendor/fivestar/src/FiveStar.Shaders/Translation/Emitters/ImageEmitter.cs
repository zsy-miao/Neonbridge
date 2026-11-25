// Copyright (c) 2026 Five Star Recomp contributors.
// SPDX-License-Identifier: GPL-3.0-or-later

using FiveStar.Shaders.Translation.Core;
using static FiveStar.Shaders.Translation.Emitters.Operands;

namespace FiveStar.Shaders.Translation.Emitters;

/// <summary>
/// MIMG instructions: sample, gather, load, store, get_resinfo, get_lod and the BVH ray intersection. Resources are
/// bindless: T# dword0 indexes ResourceDescriptorHeap, S# dword0 SamplerDescriptorHeap (literal S#s get static
/// sampler slots). dmask selects the returned components; d16 packs them as f16 pairs.
/// </summary>
internal sealed class ImageEmitter
{
    private const int AddressOperand = 1;
    private const int ResourceOperand = 2;
    private const int SamplerOperand = 3;
    private const int AllComponents = 0xf;
    private const int Vec4 = 4;
    private const string ComponentNames = "xyzw";

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
            Store(op, destination, modifiers, (type, components), _operands.Read(resource, 0), coordinates);

            return;
        }

        var request = Prepare(op, modifiers, (type, components, dimension), coordinates, number);
        request = request with { SamplerOperand = sampler };
        Declare(request, resource, sampler);
        Complete(op, request, destination, modifiers);
    }

    private void Store(string op, Operand data, InstructionModifiers modifiers, (string Type, int Components) texture, string heapIndex, List<string> coordinates)
    {
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
        var values = new List<string>();
        var next = 0;

        for (var component = 0; component < Vec4; component++)
        {
            if (((mask >> component) & 1) != 0)
            {
                values.Add($"asfloat({_operands.Read(data, next)})");
                next++;
            }
            else
            {
                values.Add("0");
            }
        }

        var name = $"_uav{_state.NextTemporary()}";
        _state.Emit($"  {{ RW{texture.Type}<float4> {name} = ResourceDescriptorHeap[{NullTextureSlots.Slot("RW" + texture.Type, heapIndex)}];");
        var index = ImageDimensions.IntegerCoordinates(coordinates, texture.Components);
        _state.Emit($"    if (active) {name}[{index}] = float4({string.Join(", ", values)}); }}");
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

    private void Complete(string op, ImageRequest request, Operand destination, InstructionModifiers modifiers)
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

        var call = _samples.Call(op, request, ref mask) ?? throw new UnsupportedProgramException(op);
        var result = $"_img{request.Number}";
        _state.Emit($"  float4 {result} = {call};");
        WriteResult(destination, result, mask, modifiers.Has("d16"));
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

    private void WriteResult(Operand destination, string result, int mask, bool packedHalves)
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
            var low = $"f32tof16({result}.{ComponentNames[components[index]]})";
            var high = "0u";

            if (index + 1 < components.Count)
            {
                high = $"(f32tof16({result}.{ComponentNames[components[index + 1]]}) << 16)";
            }

            _operands.Write(destination, $"({low} | {high})", index / PairDwords);
        }
    }
}
