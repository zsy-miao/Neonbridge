// Copyright (c) 2026 Five Star Recomp contributors.
// SPDX-License-Identifier: GPL-3.0-or-later

using System.Globalization;
using System.Text.RegularExpressions;

namespace FiveStar.Shaders.Translation;

/// <summary>
/// Rewrites a translated mesh shader whose output exceeds the PC limit (32 KB of vertices and primitives) into a
/// compute shader plus a vertex-pulling vertex shader. The compute shader has the same body; at SetMeshOutputCounts
/// lane 0 allocates space in the output buffers, and at the end every lane stores its vertex record and triangle. The
/// vertex shader loads the record by SV_VertexID and returns the same Out struct, so the pixel shader links unchanged.
/// The runtime contract (register space 9) is described in docs/shader-translation.md.
/// </summary>
public static partial class MeshComputeFallback
{
    private const int BytesPerComponent = 4;

    /// <summary>The compute shader and the vertex shader that replace a translated mesh shader.</summary>
    public static (string Compute, string Vertex) Rewrite(string meshHlsl)
    {
        var layout = OutLayout(meshHlsl);
        var entry = EntryPattern().Match(meshHlsl);

        if (!entry.Success)
        {
            throw new UnsupportedProgramException("mesh entry point not found");
        }

        var threads = Number(entry.Groups["threads"].Value);
        var maxVertices = Number(entry.Groups["verts"].Value);
        var maxPrimitives = Number(entry.Groups["prims"].Value);
        var bodyStart = entry.Index + entry.Length;

        if (ReturnPattern().IsMatch(meshHlsl[bodyStart..]))
        {
            throw new UnsupportedProgramException("early return in mesh body");
        }

        if (!meshHlsl.TrimEnd().EndsWith('}'))
        {
            throw new UnsupportedProgramException("unexpected tail");
        }

        var head = meshHlsl[..entry.Index];
        var structIndex = head.IndexOf(layout.StructText, StringComparison.Ordinal);
        head = head[..structIndex] + layout.StructText + "\n" + Declarations(layout, maxVertices, maxPrimitives)
            + head[(structIndex + layout.StructText.Length)..];
        head += $"[numthreads({threads}, 1, 1)]\nvoid main(uint3 in_gid : SV_GroupID, uint in_gi : SV_GroupIndex) {{\n"
            + "  Out vo = (Out)0; uint3 to = uint3(0, 0, 0); bool tc = true;\n";

        var body = meshHlsl[bodyStart..].TrimEnd()[..^1];
        var compute = head + RewriteBody(body, maxVertices, maxPrimitives) + StoreTail;

        return (compute, VertexShader(layout));
    }

    private static OutLayout OutLayout(string hlsl)
    {
        var match = OutStructPattern().Match(hlsl);

        if (!match.Success)
        {
            throw new UnsupportedProgramException("no Out struct");
        }

        var members = new List<OutMember>();
        var offset = 0;

        foreach (var line in match.Groups[1].Value.Split('\n'))
        {
            var member = MemberPattern().Match(line);

            if (!member.Success)
            {
                throw new UnsupportedProgramException($"Out member not understood: {line.Trim()}");
            }

            var width = member.Groups["width"].Value.Length == 0 ? 1 : Number(member.Groups["width"].Value);
            members.Add(new OutMember(member.Groups["name"].Value, member.Groups[1].Value, width, offset));
            offset += BytesPerComponent * width;
        }

        return new OutLayout(match.Value, members, offset);
    }

    private static string Declarations(OutLayout layout, int maxVertices, int maxPrimitives)
    {
        var lines = new List<string>
        {
            $"// mesh->compute fallback: vertex record stride {layout.Stride} B, max {maxVertices} vertices / {maxPrimitives} triangles per group",
            "RWByteAddressBuffer g_msfb_vtx : register(u0, space9);",
            "RWByteAddressBuffer g_msfb_idx : register(u1, space9);",
            "RWByteAddressBuffer g_msfb_cnt : register(u2, space9);",
            "groupshared uint g_ms_bv, g_ms_bp, g_ms_npr;",
            $"#define MSFB_STRIDE {layout.Stride}u",
            "void msfb_store(uint a, Out o) {",
        };
        lines.AddRange(layout.Members.Select(member => $"  g_msfb_vtx.Store{Suffix(member.Width)}(a + {member.Offset}u, asuint(o.{member.Name}));"));
        lines.Add("}");

        return string.Join("\n", lines);
    }

    private static string RewriteBody(string body, int maxVertices, int maxPrimitives)
    {
        // Lane 0 allocates the group's vertices and indices where SetMeshOutputCounts was; a group that does not fit is
        // dropped, but the triangles it counted stay in the draw arguments.
        var allocation = "  if (in_gi == 0) {\n"
            + $"    uint nv = min(g_ms_nv, {maxVertices}u), np = min(g_ms_np, {maxPrimitives}u), bv, bp, cap_v, cap_i;\n"
            + "    g_msfb_vtx.GetDimensions(cap_v); g_msfb_idx.GetDimensions(cap_i);\n"
            + "    g_msfb_cnt.InterlockedAdd(20, nv, bv); g_msfb_cnt.InterlockedAdd(0, np * 3u, bp);\n"
            + "    g_ms_npr = np;   // triangles counted in the draw arguments, even if the group is dropped\n"
            + "    if ((bv + nv) * MSFB_STRIDE > cap_v || (bp + np * 3u) * 4u > cap_i) { nv = 0; np = 0; }\n"
            + "    g_ms_bv = bv; g_ms_bp = bp; g_ms_nv = nv; g_ms_np = np;\n"
            + "  }\n"
            + "  GroupMemoryBarrierWithGroupSync();";

        if (!SetCountsPattern().IsMatch(body))
        {
            throw new UnsupportedProgramException("SetMeshOutputCounts not found");
        }

        body = SetCountsPattern().Replace(body, _ => allocation, 1);
        body = CullPattern().Replace(body, "tc");
        body = VertexPattern().Replace(body, "vo");
        body = TrianglePattern().Replace(body, "to");

        if (AnyOutputPattern().IsMatch(body))
        {
            throw new UnsupportedProgramException("mesh output indexed by something other than in_gi");
        }

        return body;
    }

    private const string StoreTail =
        "  if (in_gi < g_ms_nv) msfb_store((g_ms_bv + in_gi) * MSFB_STRIDE, vo);\n"
        + "  if (in_gi < g_ms_np) {\n"
        + "    uint3 t = (tc ? uint3(0, 0, 0) : min(to, uint3(g_ms_nv - 1, g_ms_nv - 1, g_ms_nv - 1))) + g_ms_bv;\n"
        + "    g_msfb_idx.Store3((g_ms_bp + in_gi * 3u) * 4u, t);\n"
        + "  }\n"
        + "  // a dropped group still owns the index range it added to the draw count: make it degenerate\n"
        + "  // (otherwise stale index data in that range is drawn); out-of-range index reads return 0 anyway\n"
        + "  if (in_gi >= g_ms_np && in_gi < g_ms_npr) {\n"
        + "    uint a = (g_ms_bp + in_gi * 3u) * 4u, cap_i; g_msfb_idx.GetDimensions(cap_i);\n"
        + "    if (a + 12u <= cap_i) g_msfb_idx.Store3(a, uint3(0, 0, 0));\n"
        + "  }\n"
        + "}\n";

    private static string VertexShader(OutLayout layout)
    {
        var lines = new List<string>
        {
            $"// vertex-pulling VS for the mesh->compute fallback (stride {layout.Stride} B)",
            layout.StructText,
            "ByteAddressBuffer g_msfb_vtx_r : register(t0, space9);",
            "Out main(uint vid : SV_VertexID) {",
            $"  Out o = (Out)0; uint a = vid * {layout.Stride}u;",
        };

        foreach (var member in layout.Members)
        {
            var load = $"g_msfb_vtx_r.Load{Suffix(member.Width)}(a + {member.Offset}u)";
            var conversion = member.Type switch
            {
                "float" => "asfloat",
                "int" => "asint",
                _ => string.Empty,
            };
            var value = conversion.Length == 0 ? load : $"{conversion}({load})";
            lines.Add($"  o.{member.Name} = {value};");
        }

        lines.AddRange(["  return o;", "}", string.Empty]);

        return string.Join("\n", lines);
    }

    private static string Suffix(int width)
    {
        if (width > 1)
        {
            return width.ToString(CultureInfo.InvariantCulture);
        }

        return string.Empty;
    }

    private static int Number(string text)
    {
        return int.Parse(text, CultureInfo.InvariantCulture);
    }

    [GeneratedRegex(@"^struct Out \{\n(.*?)\n\};", RegexOptions.Singleline | RegexOptions.Multiline)]
    private static partial Regex OutStructPattern();

    [GeneratedRegex(@"^\s*(?:(?:nointerpolation|linear|noperspective|centroid|sample)\s+)*(float|uint|int)(?<width>[1-4]?)\s+(?<name>\w+)\s*:")]
    private static partial Regex MemberPattern();

    [GeneratedRegex(@"\[numthreads\((?<threads>\d+), 1, 1\)\]\n\[outputtopology\(""triangle""\)\]\nvoid main\(uint3 in_gid : SV_GroupID, uint in_gi : SV_GroupIndex, out vertices Out verts\[(?<verts>\d+)\], out indices uint3 tris\[(?<prims>\d+)\], out primitives MSPrim prims\[\d+\]\) \{\n")]
    private static partial Regex EntryPattern();

    [GeneratedRegex(@"\breturn\b")]
    private static partial Regex ReturnPattern();

    [GeneratedRegex(@"^ *SetMeshOutputCounts\([^;]*\);", RegexOptions.Multiline)]
    private static partial Regex SetCountsPattern();

    [GeneratedRegex(@"\bprims\[in_gi\]\.cull\b")]
    private static partial Regex CullPattern();

    [GeneratedRegex(@"\bverts\[in_gi\]")]
    private static partial Regex VertexPattern();

    [GeneratedRegex(@"\btris\[in_gi\]")]
    private static partial Regex TrianglePattern();

    [GeneratedRegex(@"\b(verts|tris|prims)\[")]
    private static partial Regex AnyOutputPattern();
}
