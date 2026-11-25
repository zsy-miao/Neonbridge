// Copyright (c) 2026 Five Star Recomp contributors.
// SPDX-License-Identifier: GPL-3.0-or-later

using System.Globalization;
using System.Text;
using FiveStar.Shaders.Tests.Reference;

namespace FiveStar.Shaders.Tests.Differential;

/// <summary>
/// Wraps a translated program so its every output lands somewhere a run can read back bit for bit:
/// a pixel shader writes each Out member as raw bits to its own uint render target; a vertex shader runs inside a compute
/// shader (64 vertices) that stores each Out member to a buffer; a mesh shader becomes its compute fallback.
/// </summary>
public static class StageHarness
{
    public const int MaxTargets = 8;
    public const int Vertices = 64;

    private const string Inner = "diff_inner";

    /// <summary>The pixel shader wrapper and its number of render targets.</summary>
    public static (string Source, int Targets) Pixel(string hlsl)
    {
        var outputs = HlslText.Members(hlsl, "Out").Take(MaxTargets).ToList();
        var (renamed, attributes, parameters) = HlslText.RenameEntry(hlsl, Inner);
        var source = new StringBuilder(renamed);
        source.Append("\nstruct DiffTargets {\n");

        var targets = Math.Max(1, outputs.Count);

        for (var index = 0; index < targets; index++)
        {
            source.Append(Invariant($"  uint4 t{index} : SV_Target{index};\n"));
        }

        source.Append("};\n").Append(attributes).Append(Invariant($"DiffTargets main({parameters}) {{\n"));
        source.Append(Invariant($"  Out o = {Inner}({HlslText.Arguments(parameters)});\n  DiffTargets d;\n"));

        if (outputs.Count == 0)
        {
            source.Append("  d.t0 = uint4(1u, 0u, 0u, 0u);\n");
        }

        for (var index = 0; index < outputs.Count; index++)
        {
            source.Append(Invariant($"  d.t{index} = {outputs[index].Bits4("o." + outputs[index].Name)};\n"));
        }

        source.Append("  return d;\n}\n");

        return (source.ToString(), targets);
    }

    /// <summary>
    /// The vertex shader that feeds a pixel shader: a triangle that covers the target, every PSIn member a constant per
    /// corner (integers flat, floats interpolated).
    /// </summary>
    public static string PixelFeeder(string pixelHlsl, SyntheticData data)
    {
        var inputs = HlslText.Members(pixelHlsl, "PSIn");

        // A program that reads no attribute declares PSIn but does not take it; its signature starts at SV_Position.
        if (!HlslText.RenameEntry(pixelHlsl, Inner).Parameters.Contains("PSIn ", StringComparison.Ordinal))
        {
            inputs = [];
        }

        var source = new StringBuilder("struct VOut {\n");

        foreach (var input in inputs)
        {
            source.Append("  ").Append(input.Line).Append('\n');
        }

        source.Append("  float4 diff_pos : SV_Position;\n};\nVOut main(uint vid : SV_VertexID) {\n  VOut o;\n");
        source.Append("  float2 uv = float2((vid << 1) & 2, vid & 2);\n");
        source.Append("  o.diff_pos = float4(uv * float2(1, -1) + float2(-1, 1), 0.5, 1);\n");

        foreach (var input in inputs)
        {
            var corners = Enumerable.Range(0, 3).Select(_ => Corner(input, data)).ToList();
            source.Append(Invariant($"  if (vid == 0) o.{input.Name} = {corners[0]};\n"));
            source.Append(Invariant($"  else if (vid == 1) o.{input.Name} = {corners[1]};\n"));
            source.Append(Invariant($"  else o.{input.Name} = {corners[2]};\n"));
        }

        source.Append("  return o;\n}\n");

        return source.ToString();
    }

    /// <summary>
    /// The compute wrapper of a vertex shader: thread i runs vertex i (instance 1) with fetch inputs from heap slot
    /// <see cref="HeapSlots.VertexInput"/> and stores its Out struct at i * stride in slot <see cref="HeapSlots.VertexOutput"/>.
    /// </summary>
    public static string Vertex(string hlsl)
    {
        var outputs = HlslText.Members(hlsl, "Out");
        var fetch = HlslText.Members(hlsl, "FetchIn");
        var (renamed, _, _) = HlslText.RenameEntry(hlsl, Inner);
        var stride = outputs.Sum(output => 4 * output.Width);

        var source = new StringBuilder(renamed);
        source.Append(Invariant($"\n[numthreads({Vertices}, 1, 1)]\nvoid main(uint3 diff_id : SV_DispatchThreadID) {{\n"));
        source.Append(Invariant($"  ByteAddressBuffer diff_in = ResourceDescriptorHeap[{HeapSlots.VertexInput}];\n"));
        source.Append(Invariant($"  RWByteAddressBuffer diff_out = ResourceDescriptorHeap[{HeapSlots.VertexOutput}];\n"));
        var arguments = "diff_id.x, 1u";

        if (fetch.Count > 0)
        {
            source.Append("  FetchIn fin;\n");

            for (var index = 0; index < fetch.Count; index++)
            {
                source.Append(Invariant($"  fin.{fetch[index].Name} = asfloat(diff_in.Load4((diff_id.x * {fetch.Count}u + {index}u) * 16u));\n"));
            }

            arguments += ", fin";
        }

        source.Append(Invariant($"  Out o = {Inner}({arguments});\n  uint a = diff_id.x * {stride}u;\n"));
        var offset = 0;

        foreach (var output in outputs)
        {
            var suffix = string.Empty;

            if (output.Width > 1)
            {
                suffix = output.Width.ToString(CultureInfo.InvariantCulture);
            }

            source.Append(Invariant($"  diff_out.Store{suffix}(a + {offset}u, {output.Bits("o." + output.Name)});\n"));
            offset += 4 * output.Width;
        }

        source.Append("}\n");

        return source.ToString();
    }

    /// <summary>Bytes a vertex wrapper writes for all <see cref="Vertices"/>.</summary>
    public static int VertexBytes(string hlsl)
    {
        return Vertices * HlslText.Members(hlsl, "Out").Sum(output => 4 * output.Width);
    }

    private static string Corner(StructMember input, SyntheticData data)
    {
        var words = Enumerable.Range(0, input.Width).Select(_ => Word(input, data)).Select(word => Invariant($"0x{word:x8}u"));
        var bits = $"{input.Vector("uint")}({string.Join(", ", words)})";

        switch (input.Scalar)
        {
            case "uint":
            case "min16uint":
                return bits;
            case "int":
            case "min16int":
                return $"asint({bits})";
            default:
                return $"asfloat({bits})";
        }
    }

    private static uint Word(StructMember input, SyntheticData data)
    {
        if (input.Scalar.Contains("int", StringComparison.Ordinal))
        {
            return data.SmallInteger();
        }

        return FloatBits.Of(data.Real());
    }

    private static string Invariant(FormattableString text)
    {
        return FormattableString.Invariant(text);
    }
}
