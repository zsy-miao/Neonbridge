// Copyright (c) 2026 Five Star Recomp contributors.
// SPDX-License-Identifier: GPL-3.0-or-later

using System.Text.RegularExpressions;
using FiveStar.Shaders.Tests.Warp;
using Vortice.DXGI;

namespace FiveStar.Shaders.Tests.Differential;

/// <summary>
/// Makes a real program safe to run on synthetic data: every descriptor-heap index it computes from its inputs is
/// redirected to a fixed slot of the declared resource type (two per type, chosen by the index's low bit), and every
/// sampler index to one of eight samplers (eight more for comparison samplers). The GPU-memory window keeps slots 0 and 1.
/// Both control-flow forms get the same rewrite, so their results stay comparable bit for bit.
/// </summary>
public static partial class HeapSlots
{
    public const int PerType = 2;
    public const int SamplerCount = 8;

    /// <summary>The compute wrapper of a vertex program: its inputs (SRV) and outputs (UAV).</summary>
    public const int VertexInput = 50;
    public const int VertexOutput = 51;

    /// <summary>The mesh fallback's u0..u2 / t0 table.</summary>
    public const int FallbackTable = 60;

    private const string Marker = "/*diff*/";

    public static IReadOnlyList<HeapEntry> Entries { get; } =
    [
        new("ByteAddressBuffer", 2, null, Format.Unknown, Writable: false),
        new("RWByteAddressBuffer", 4, null, Format.Unknown, Writable: true),
        new("Texture1D<float4>", 6, TextureKind.Texture1D, Format.R32G32B32A32_Float, Writable: false),
        new("Texture2D<float4>", 8, TextureKind.Texture2D, Format.R32G32B32A32_Float, Writable: false),
        new("Texture3D<float4>", 10, TextureKind.Texture3D, Format.R32G32B32A32_Float, Writable: false),
        new("Texture1DArray<float4>", 12, TextureKind.Texture1DArray, Format.R32G32B32A32_Float, Writable: false),
        new("Texture2DArray<float4>", 14, TextureKind.Texture2DArray, Format.R32G32B32A32_Float, Writable: false),
        new("TextureCube<float4>", 16, TextureKind.TextureCube, Format.R32G32B32A32_Float, Writable: false),
        new("RWTexture1D<float4>", 18, TextureKind.Texture1D, Format.R32G32B32A32_Float, Writable: true),
        new("RWTexture2D<float4>", 20, TextureKind.Texture2D, Format.R32G32B32A32_Float, Writable: true),
        new("RWTexture3D<float4>", 22, TextureKind.Texture3D, Format.R32G32B32A32_Float, Writable: true),
        new("RWTexture2DArray<float4>", 24, TextureKind.Texture2DArray, Format.R32G32B32A32_Float, Writable: true),
        new("RWTexture1D<uint>", 26, TextureKind.Texture1D, Format.R32_UInt, Writable: true),
        new("RWTexture2D<uint>", 28, TextureKind.Texture2D, Format.R32_UInt, Writable: true),
        new("RWTexture3D<uint>", 30, TextureKind.Texture3D, Format.R32_UInt, Writable: true),
        new("RWTexture2DArray<uint>", 32, TextureKind.Texture2DArray, Format.R32_UInt, Writable: true),
        new("RWTexture1D<int>", 34, TextureKind.Texture1D, Format.R32_SInt, Writable: true),
        new("RWTexture2D<int>", 36, TextureKind.Texture2D, Format.R32_SInt, Writable: true),
        new("RWTexture3D<int>", 38, TextureKind.Texture3D, Format.R32_SInt, Writable: true),
        new("RWTexture2DArray<int>", 40, TextureKind.Texture2DArray, Format.R32_SInt, Writable: true),
    ];

    /// <summary>The text with every heap index redirected; throws <see cref="NotSupportedException"/> for an unknown type.</summary>
    public static string Rewrite(string hlsl)
    {
        var rewritten = DeclarationPattern().Replace(hlsl, Redirect);

        if (UnrewrittenPattern().IsMatch(rewritten))
        {
            throw new NotSupportedException($"heap access not redirected: {UnrewrittenPattern().Match(rewritten).Value}");
        }

        return rewritten;
    }

    private static string Redirect(Match match)
    {
        var type = match.Groups["type"].Value;
        var index = match.Groups["index"].Value;
        var head = $"{type} {match.Groups["name"].Value} = {match.Groups["heap"].Value}DescriptorHeap[{Marker}";

        if (index.StartsWith("GPU_MEM_HEAP", StringComparison.Ordinal))
        {
            return $"{head}{index}];";
        }

        if (type == "SamplerState")
        {
            return $"{head}(({index}) & {SamplerCount - 1}u)];";
        }

        if (type == "SamplerComparisonState")
        {
            return $"{head}{SamplerCount}u + (({index}) & {SamplerCount - 1}u)];";
        }

        var key = type.Replace("RasterizerOrdered", "RW", StringComparison.Ordinal);
        var entry = Entries.FirstOrDefault(candidate => candidate.Type == key) ?? throw new NotSupportedException($"resource type {type}");

        return $"{head}{entry.Base}u + (({index}) & {PerType - 1}u)];";
    }

    [GeneratedRegex(@"(?<type>\b[A-Za-z]\w*(?:<\w+>)?) (?<name>\w+) = (?<heap>Resource|Sampler)DescriptorHeap\[(?<index>.*?)\];")]
    private static partial Regex DeclarationPattern();

    [GeneratedRegex(@"DescriptorHeap\[(?!/\*diff\*/)[^\n]*")]
    private static partial Regex UnrewrittenPattern();
}
