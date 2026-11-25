// Copyright (c) 2026 Five Star Recomp contributors.
// SPDX-License-Identifier: GPL-3.0-or-later

using System.Text.RegularExpressions;
using FiveStar.Shaders.Containers;
using FiveStar.Shaders.Translation.Core;

namespace FiveStar.Shaders.Translation.Stages;

/// <summary>
/// Mesh-shader (NGG) declarations. In two-phase mode exports write the outputs directly; otherwise every export is
/// staged in compact groupshared structs and copied to the outputs after SetMeshOutputCounts. A primitive export holds
/// three 9-bit vertex indices at bits 0/10/20 and bit 31 = culled.
/// </summary>
internal sealed partial class MeshDeclarations
{
    // NGG groups hold up to 128 vertices and primitives when PSV0 does not say otherwise.
    private const int DefaultMaxOutputs = 128;

    // D3D12 groupshared limit left for staging (32 KB minus the other groupshared data).
    private const int StagingLimit = 28672;
    private const int BytesPerComponent = 4;
    private const int CounterBytes = 8;
    private const int Vec4 = 4;
    private const string Components = "xyzw";

    private readonly TranslationState _state;

    public MeshDeclarations(TranslationState state)
    {
        _state = state;
    }

    public int MaxVertices => NonZero(_state.PipelineState?.MaxVertices);

    public int MaxPrimitives => NonZero(_state.PipelineState?.MaxPrimitives);

    /// <summary>The mesh entry point: numthreads, triangle topology and the vertex, index and primitive outputs.</summary>
    public string EntrySignature()
    {
        return $"[numthreads({_state.MeshThreads}, 1, 1)]\n[outputtopology(\"triangle\")]\n"
            + $"void main(uint3 in_gid : SV_GroupID, uint in_gi : SV_GroupIndex, out vertices Out verts[{MaxVertices}], "
            + $"out indices uint3 tris[{MaxPrimitives}], out primitives MSPrim prims[{MaxPrimitives}])";
    }

    public static string DirectDeclarations()
    {
        return "groupshared uint g_ms_nv, g_ms_np, g_ms_alloc;\ngroupshared uint g_ms_live[3];\nstruct MSPrim { bool cull : SV_CullPrimitive; };";
    }

    /// <summary>The staging structs, their converters and the groupshared arrays, sized to what is written.</summary>
    public string StagingDeclarations(List<List<SignatureElement>> outputRegisters)
    {
        var groupSize = Math.Max(_state.MeshThreads, Math.Max(MaxVertices, MaxPrimitives));
        var widths = StagedWidths(outputRegisters);
        var lds = 0;

        if (_state.UsesLds)
        {
            lds = _state.Config.LdsBytes ?? 0;
        }

        var size = (groupSize * BytesPerComponent * (widths.Values.Sum() + 1)) + CounterBytes + lds;

        if (size > StagingLimit)
        {
            throw new UnsupportedProgramException($"mesh staging {size} B > 28 KB groupshared");
        }

        var ordered = widths.OrderBy(pair => pair.Key, StringComparer.Ordinal).ToList();
        var lines = new List<string> { "struct MSExp {" };
        lines.AddRange(ordered.Select(pair => $"  float{VectorSuffix(pair.Value)} {pair.Key};"));
        lines.Add("};");
        lines.AddRange(ordered.Select(pair => $"float{VectorSuffix(pair.Value)} MSW_{pair.Key}(float4 v) {{ return v.{Components[..pair.Value]}; }}"));

        lines.Add("Exp ms_unstage(MSExp s) {");
        lines.Add("  Exp x = (Exp)0;");
        lines.AddRange(ordered.Select(pair => $"  x.{pair.Key}.{Components[..pair.Value]} = s.{pair.Key};"));
        lines.Add("  return x;");
        lines.Add("}");

        return string.Join("\n", lines)
            + $"\ngroupshared uint g_ms_nv, g_ms_np, g_ms_alloc;\ngroupshared MSExp g_ms_exp[{groupSize}];\ngroupshared uint g_ms_prim[{groupSize}];\n"
            + "struct MSPrim { bool cull : SV_CullPrimitive; };";
    }

    /// <summary>Copies staged vertices and primitives to the outputs after the dispatch loop.</summary>
    public List<string> StagingEpilogue()
    {
        var lines = new List<string>
        {
            "  GroupMemoryBarrierWithGroupSync();",
            $"  SetMeshOutputCounts(min(g_ms_nv, {MaxVertices}u), min(g_ms_np, {MaxPrimitives}u));",
            $"  if (in_gi < min(g_ms_nv, {MaxVertices}u)) {{",
            "    Out o = (Out)0; x = ms_unstage(g_ms_exp[in_gi]);",
        };
        lines.AddRange(_state.OutputMap.Select(line => "  " + line));
        lines.AddRange(
        [
            "    verts[in_gi] = o;",
            "  }",
            $"  if (in_gi < min(g_ms_np, {MaxPrimitives}u)) {{",
            "    uint p = g_ms_prim[in_gi];   // gfx10 prim export: 3 x 9-bit indices at bits 0/10/20, bit 31 = null",
            "    tris[in_gi] = uint3(p & 0x1ffu, (p >> 10) & 0x1ffu, (p >> 20) & 0x1ffu);",
            "    prims[in_gi].cull = (p >> 31) != 0;",
            "  }",
            "}",
        ]);

        return lines;
    }

    /// <summary>Two-phase mode: replaces each export placeholder with that target's output statements.</summary>
    public void ExpandDirectExports()
    {
        foreach (var block in _state.Blocks)
        {
            for (var index = 0; index < block.Lines.Count; index++)
            {
                var placeholder = PlaceholderPattern().Match(block.Lines[index]);

                if (placeholder.Success)
                {
                    block.Lines[index] = block.Lines[index].Replace(placeholder.Value, DirectStatements(placeholder.Groups[1].Value), StringComparison.Ordinal);
                }
            }
        }
    }

    /// <summary>True for a statement that writes a mesh output (dropped from the first phase).</summary>
    /// <summary>
    /// After SetMeshOutputCounts in a direct-export form: every primitive of the group's count starts culled, so one the
    /// program never exports (a lane whose export is off) is not drawn from undefined indices. Uninitialised primitives
    /// hung NVIDIA's mesh pipeline (VIctory #587 initialises them all). Each thread its own, as every output write is.
    /// </summary>
    public List<string> InitialPrimitives()
    {
        return
        [
            $"  if (in_gi < min(g_ms_np, {MaxPrimitives}u)) {{",
            "    tris[in_gi] = uint3(0u, 0u, 0u);",
            "    prims[in_gi].cull = true;",
            "  }",
        ];
    }

    /// <summary>
    /// After a two-phase program cut off by its round bound (<paramref name="cut"/>, an HLSL condition, the same on every
    /// thread): its primitives culled, so none of the outputs it never wrote is drawn (WARP's rasterizer crashed on them).
    /// Each thread its own, as every output write is (the mesh-to-compute fallback needs that).
    /// </summary>
    public List<string> CutEpilogue(string cut)
    {
        return
        [
            $"  if ({cut} && in_gi < min(g_ms_np, {MaxPrimitives}u)) {{",
            "    tris[in_gi] = uint3(0u, 0u, 0u);",
            "    prims[in_gi].cull = true;",
            "  }",
        ];
    }

    public static bool WritesOutput(string line)
    {
        return OutputWritePattern().IsMatch(line);
    }

    private string DirectStatements(string target)
    {
        var export = new Regex($@"\bx\.{target}\b");
        var statements = _state.OutputMap
            .Where(line => export.IsMatch(line))
            .Select(line => export.Replace(OutputPattern().Replace(line.Trim(), "verts[in_gi]."), "_e"));

        return string.Join(" ", statements);
    }

    private Dictionary<string, int> StagedWidths(List<List<SignatureElement>> outputRegisters)
    {
        var widths = new Dictionary<string, int>(_state.MeshWidths);

        for (var register = 0; register < outputRegisters.Count; register++)
        {
            var target = $"param{register}";

            if (!widths.ContainsKey(target))
            {
                continue;
            }

            foreach (var element in outputRegisters[register])
            {
                var highest = element.Mask.Max(component => Components.IndexOf(component, StringComparison.Ordinal)) + 1;
                widths[target] = Math.Max(widths[target], highest);
            }
        }

        if (widths.ContainsKey("pos0"))
        {
            widths["pos0"] = Vec4;
        }

        return widths;
    }

    private static string VectorSuffix(int width)
    {
        if (width > 1)
        {
            return width.ToString(System.Globalization.CultureInfo.InvariantCulture);
        }

        return string.Empty;
    }

    private static int NonZero(int? value)
    {
        if (value is null || value.Value == 0)
        {
            return DefaultMaxOutputs;
        }

        return value.Value;
    }

    [GeneratedRegex(@"/\*MSEXP:(\w+)\*/")]
    private static partial Regex PlaceholderPattern();

    [GeneratedRegex(@"\bo\.")]
    private static partial Regex OutputPattern();

    [GeneratedRegex(@"verts\[in_gi\]|tris\[in_gi\]|prims\[in_gi\]")]
    private static partial Regex OutputWritePattern();
}
