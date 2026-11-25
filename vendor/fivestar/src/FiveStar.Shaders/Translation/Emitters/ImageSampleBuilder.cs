// Copyright (c) 2026 Five Star Recomp contributors.
// SPDX-License-Identifier: GPL-3.0-or-later

using FiveStar.Shaders.Translation.Core;

namespace FiveStar.Shaders.Translation.Emitters;

/// <summary>
/// Builds the HLSL call of a sample, get_lod, load or gather. The address layout is
/// [offset][bias][z-compare][derivatives][coordinates][lod or mip][clamp]; the offset was already taken off.
/// </summary>
internal sealed class ImageSampleBuilder
{
    private const int CubeAddress = 3;
    private const int CubeFaceDerivatives = 4;
    private const int AllComponents = 0xf;

    // A 2D array's address: x, y, then the slice.
    private const int ArraySlice = 2;

    private static readonly HashSet<string> SampleOperations =
    [
        "sample", "sample_lz", "sample_l", "sample_b", "sample_c", "sample_c_lz", "sample_c_l", "sample_d", "sample_c_d",
    ];

    private static readonly HashSet<string> GatherOperations = ["gather4", "gather4_lz", "gather4_l", "gather4_c", "gather4_c_lz"];

    // gather4 dmask selects the one gathered channel.
    private static readonly IReadOnlyDictionary<int, string> GatherChannels = new Dictionary<int, string>
    {
        [1] = "Red",
        [2] = "Green",
        [4] = "Blue",
        [8] = "Alpha",
    };

    private static readonly IReadOnlyDictionary<string, string> ComponentOf = new Dictionary<string, string>
    {
        ["Red"] = "x",
        ["Green"] = "y",
        ["Blue"] = "z",
        ["Alpha"] = "w",
    };

    private readonly TranslationState _state;
    private readonly OperandAccess _operands;

    public ImageSampleBuilder(TranslationState state, OperandAccess operands)
    {
        _state = state;
        _operands = operands;
    }

    /// <summary>The float4 call expression, or null for an operation this builder does not handle.</summary>
    public string? Call(string op, ImageRequest request, ref int mask)
    {
        if (SampleOperations.Contains(request.Operation))
        {
            return Sample(request);
        }

        if (GatherOperations.Contains(request.Operation))
        {
            var call = Gather(request, mask);
            mask = AllComponents;

            return call;
        }

        switch (request.Operation)
        {
            case "get_lod":
                return LevelOfDetail(request);
            case "load":
            case "load_mip":
                return Load(request);
            default:
                return null;
        }
    }

    private string Sample(ImageRequest request)
    {
        var address = new SampleAddress(request);
        var call = SampleCall(request, address);

        return call;
    }

    private string SampleCall(ImageRequest request, SampleAddress address)
    {
        var tail = address.Tail;
        var offsetTail = address.OffsetTail;
        var texture = request.Texture;
        var sampler = request.Sampler;
        var operation = request.Operation;

        if (address.Compare is not null)
        {
            return CompareCall(request, address);
        }

        if (address.DerivativeX is not null)
        {
            return $"{texture}.SampleGrad({sampler}, {address.Coordinates}, {address.DerivativeX}, {address.DerivativeY}{tail})";
        }

        if (operation.EndsWith("_lz", StringComparison.Ordinal) || (_state.Stage != ShaderStage.Pixel && operation == "sample"))
        {
            return $"{texture}.SampleLevel({sampler}, {address.Coordinates}, 0{offsetTail})";
        }

        if (address.Lod is not null)
        {
            return $"{texture}.SampleLevel({sampler}, {address.Coordinates}, {address.Lod}{offsetTail})";
        }

        if (address.Bias is not null)
        {
            return $"{texture}.SampleBias({sampler}, {address.Coordinates}, {address.Bias}{tail})";
        }

        return $"{texture}.Sample({sampler}, {address.Coordinates}{tail})";
    }

    private string CompareCall(ImageRequest request, SampleAddress address)
    {
        var state = $"_smpc{request.Number}";
        _state.Emit($"  SamplerComparisonState {state} = SamplerDescriptorHeap[sampler_slot({SamplerIndex(request)})];");
        var texture = request.Texture;
        var coordinates = address.Coordinates;

        if (request.Operation == "sample_c" && _state.Stage == ShaderStage.Pixel)
        {
            return $"{texture}.SampleCmp({state}, {coordinates}, {address.Compare}{address.Tail}).xxxx";
        }

        if (request.Operation == "sample_c_d")
        {
            return $"{texture}.SampleCmpGrad({state}, {coordinates}, {address.Compare}, {address.DerivativeX}, {address.DerivativeY}{address.Tail}).xxxx";
        }

        if (request.Operation == "sample_c_l")
        {
            return $"{texture}.SampleCmpLevel({state}, {coordinates}, {address.Compare}, {address.Lod}{address.OffsetTail}).xxxx";
        }

        return $"{texture}.SampleCmpLevelZero({state}, {coordinates}, {address.Compare}{address.OffsetTail}).xxxx";
    }

    private string SamplerIndex(ImageRequest request)
    {
        var sampler = request.SamplerOperand ?? throw new UnsupportedProgramException("compare sample without a sampler");

        return _operands.SamplerIndex(sampler, comparison: true);
    }

    private static string LevelOfDetail(ImageRequest request)
    {
        var coordinates = request.FloatVector(request.Coordinates.Take(request.SpatialComponents).ToList());
        var texture = request.Texture;

        return $"float4({texture}.CalculateLevelOfDetail({request.Sampler}, {coordinates}), "
            + $"{texture}.CalculateLevelOfDetailUnclamped({request.Sampler}, {coordinates}), 0, 0)";
    }

    private static string Load(ImageRequest request)
    {
        var address = request.Coordinates.Take(request.Components).ToList();
        var level = "0u";

        if (request.Operation == "load_mip")
        {
            level = request.Coordinates[request.Components];
        }

        address.Add(level);

        return $"{request.Texture}.Load(int{request.Components + 1}({string.Join(", ", address.Select(value => $"asint({value})"))}))";
    }

    private string Gather(ImageRequest request, int mask)
    {
        if (!GatherChannels.TryGetValue(mask, out var channel))
        {
            throw new UnsupportedProgramException("gather dmask");
        }

        var offsetTail = request.Offsets is null ? string.Empty : $", {request.Offsets}";
        var coordinates = request.Coordinates;

        if (request.HasCompare)
        {
            var state = $"_smpc{request.Number}";
            _state.Emit($"  SamplerComparisonState {state} = SamplerDescriptorHeap[sampler_slot({SamplerIndex(request)})];");
            var location = request.FloatVector(coordinates.Skip(1).Take(request.Components).ToList());

            return $"{request.Texture}.GatherCmp{channel}({state}, {location}, asfloat({coordinates[0]}){offsetTail})";
        }

        if (request.Operation == "gather4_l" && request.Dimension is "2D" or "2D_ARRAY")
        {
            return GatherAtLevel(request, ComponentOf[channel]);
        }

        // gather4_l on other dimensions: HLSL Gather has no level operand (it reads level 0).
        var position = request.FloatVector(coordinates.Take(request.Components).ToList());

        return $"{request.Texture}.Gather{channel}({request.Sampler}, {position}{offsetTail})";
    }

    /// <summary>
    /// gather4_l (and _o) on a 2D texture or array. HLSL Gather has no level operand and reads level 0, so the four texels
    /// are loaded from the level a gather takes (the LOD rounded, clamped to the view's levels), in gather order (i0, j1)
    /// (i1, j1) (i1, j0) (i0, j0), each coordinate clamped to the level's edge. Depth-pyramid culling gathers at a level.
    /// </summary>
    private string GatherAtLevel(ImageRequest request, string component)
    {
        var n = request.Number;
        var values = request.Coordinates;
        var texture = request.Texture;
        var array = request.Dimension == "2D_ARRAY";
        var (level, size, low, high) = ($"_gm{n}", $"_gd{n}", $"_g0{n}", $"_g1{n}");
        var slice = string.Empty;

        if (array)
        {
            _state.Emit($"  uint _gw{n}, _gh{n}, _gs{n}, _gn{n}; {texture}.GetDimensions(0u, _gw{n}, _gh{n}, _gs{n}, _gn{n});");
            slice = $", (int)min((uint)max(round(asfloat({values[ArraySlice]})), 0.0), _gs{n} - 1u)";
        }
        else
        {
            _state.Emit($"  uint _gw{n}, _gh{n}, _gn{n}; {texture}.GetDimensions(0u, _gw{n}, _gh{n}, _gn{n});");
        }

        var offset = request.Offsets is null ? string.Empty : $" + {request.Offsets}";
        _state.Emit($"  uint {level} = min((uint)max(round(asfloat({values[request.Components]})), 0.0), _gn{n} - 1u);");
        _state.Emit($"  int2 {size} = int2(max(_gw{n} >> {level}, 1u), max(_gh{n} >> {level}, 1u));");

        _state.Emit($"  int2 {low} = int2(floor(float2(asfloat({values[0]}), asfloat({values[1]})) * float2({size}) - 0.5)){offset};");
        _state.Emit($"  int2 {high} = clamp({low} + int2(1, 1), int2(0, 0), {size} - int2(1, 1));");
        _state.Emit($"  {low} = clamp({low}, int2(0, 0), {size} - int2(1, 1));");
        var vector = array ? "int4" : "int3";

        string Texel(string x, string y) => $"{texture}.Load({vector}({x}, {y}{slice}, (int){level})).{component}";

        return $"float4({Texel($"{low}.x", $"{high}.y")}, {Texel($"{high}.x", $"{high}.y")}, {Texel($"{high}.x", $"{low}.y")}, {Texel($"{low}.x", $"{low}.y")})";
    }

    /// <summary>The parts of a sample address in HLSL form.</summary>
    private sealed class SampleAddress
    {
        private const int PairGradient = 2;

        public SampleAddress(ImageRequest request)
        {
            var values = request.Coordinates;
            var next = 0;

            if (request.HasBias)
            {
                Bias = $"asfloat({values[next]})";
                next++;
            }

            if (request.HasCompare)
            {
                Compare = $"asfloat({values[next]})";
                next++;
            }

            next = ReadDerivatives(request, values, next);
            Coordinates = request.FloatVector(values.Skip(next).Take(request.Components).ToList());
            next += request.Components;

            if (request.Operation.EndsWith("_l", StringComparison.Ordinal))
            {
                Lod = $"asfloat({values[next]})";
                next++;
            }

            Clamp = request.HasClamp ? $"asfloat({values[next]})" : null;
            (Tail, OffsetTail) = Tails(request, Clamp);
        }

        public string? Bias { get; }

        public string? Compare { get; }

        public string? DerivativeX { get; private set; }

        public string? DerivativeY { get; private set; }

        public string Coordinates { get; }

        public string? Lod { get; }

        public string? Clamp { get; }

        /// <summary>The optional offset and clamp arguments.</summary>
        public string Tail { get; }

        /// <summary>The optional offset argument (overloads without clamp).</summary>
        public string OffsetTail { get; }

        private int ReadDerivatives(ImageRequest request, List<string> values, int next)
        {
            var count = request.Derivatives;

            if (count == 0)
            {
                return next;
            }

            var half = count / PairGradient;
            DerivativeX = request.FloatVector(values.Skip(next).Take(half).ToList());
            DerivativeY = request.FloatVector(values.Skip(next + half).Take(count - half).ToList());
            next += count;

            if (request.IsCube)
            {
                CubeDerivatives(values, next);
            }

            return next;
        }

        /// <summary>cube_dir is linear in (s, t) on a face, so the finite difference is the exact direction gradient.</summary>
        private void CubeDerivatives(List<string> values, int next)
        {
            var face = values.Skip(next).Take(CubeAddress).Select(value => $"asfloat({value})").ToList();
            var gradients = values.Skip(next - CubeFaceDerivatives).Take(CubeFaceDerivatives).Select(value => $"asfloat({value})").ToList();
            var (s, t, f) = (face[0], face[1], face[^1]);
            var origin = $"cube_dir({s}, {t}, {f})";
            DerivativeX = $"(cube_dir({s} + {gradients[0]}, {t} + {gradients[1]}, {f}) - {origin})";
            DerivativeY = $"(cube_dir({s} + {gradients[^PairGradient]}, {t} + {gradients[^1]}, {f}) - {origin})";
        }

        private static (string Tail, string OffsetTail) Tails(ImageRequest request, string? clamp)
        {
            var spatial = request.SpatialComponents;
            var offsets = request.Offsets;

            // An explicit clamp needs an offset argument before it; TextureCube overloads have none.
            if (offsets is null && clamp is not null && !request.IsCube)
            {
                offsets = "0";

                if (spatial > 1)
                {
                    offsets = $"int{spatial}({string.Join(", ", Enumerable.Repeat("0", spatial))})";
                }
            }

            var offsetTail = offsets is null ? string.Empty : $", {offsets}";
            var clampTail = clamp is null ? string.Empty : $", {clamp}";

            return (offsetTail + clampTail, offsetTail);
        }
    }
}
