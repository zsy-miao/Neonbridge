// Copyright (c) 2026 Five Star Recomp contributors.
// SPDX-License-Identifier: GPL-3.0-or-later

using System.Security.Cryptography;
using System.Text;
using FiveStar.Shaders.Containers;

namespace FiveStar.Shaders.Compilation;

/// <summary>
/// Writes the geometry shader that draws a console rectangle-list primitive on the desktop (docs/d3d12x-shim.md,
/// Rectangle lists). Its vertex struct is the vertex shader's output signature, in and out, so the pixel shader links
/// as before. Per triangle it finds the rectangle's corner (the vertex opposite the longest screen-space edge), adds
/// the fourth vertex as the sum of the corner's neighbours minus the corner (every output member) and emits the
/// rectangle as a strip of two triangles with the input triangle's winding.
/// </summary>
public static class RectangleGeometryShader
{
    /// <summary>The folder of the compiled shaders inside a translate output folder.</summary>
    public const string Folder = "rect-gs";

    private const string FilePrefix = "gs_";
    private const string DxilExtension = ".dxil";
    private const int HashCharacters = 16;

    /// <summary>The HLSL source (entry point main) for a vertex shader's output elements.</summary>
    public static string Source(IReadOnlyList<PsvElement> vertexOutputs)
    {
        var members = new StringBuilder();
        var opposite = new StringBuilder();
        string? position = null;

        for (var index = 0; index < vertexOutputs.Count; index++)
        {
            var element = vertexOutputs[index];
            var name = $"e{index}";
            members.Append(SignatureMembers.Member(element, name));

            if (element.SemanticKind == PsvSemanticKind.Position)
            {
                position ??= name;
            }

            opposite.Append(OppositeLine(element, name));
        }

        if (position is null)
        {
            throw new NotSupportedException("a vertex shader without SV_Position");
        }

        return $"struct Vertex\n{{\n{members}}};\n\n{Helpers(opposite.ToString())}\n{Main(position)}";
    }

    /// <summary>The compiled file's name for <paramref name="source"/>: a hash of the text, so equal signatures share one file.</summary>
    public static string FileName(string source)
    {
        var hash = Convert.ToHexStringLower(SHA256.HashData(Encoding.UTF8.GetBytes(source)))[..HashCharacters];

        return FilePrefix + hash + DxilExtension;
    }

    /// <summary>The fourth vertex's member: neighbours minus corner; the target and viewport indices stay the corner's.</summary>
    private static string OppositeLine(PsvElement element, string name)
    {
        if (element.SemanticKind is PsvSemanticKind.RenderTargetArrayIndex or PsvSemanticKind.ViewportArrayIndex)
        {
            return string.Empty;
        }

        if (element.Rows > 1)
        {
            return $"    [unroll] for (uint r = 0; r < {element.Rows}; r++) {{ e.{name}[r] = b.{name}[r] + d.{name}[r] - a.{name}[r]; }}\n";
        }

        return $"    e.{name} = b.{name} + d.{name} - a.{name};\n";
    }

    private static string Helpers(string opposite)
    {
        return "float2 Screen(float4 position)\n{\n"
            + "    float w = position.w;\n    if (w == 0.0) { w = 1.0; }\n    return position.xy / w;\n}\n\n"
            + $"Vertex Opposite(Vertex a, Vertex b, Vertex d)\n{{\n    Vertex e = a;\n{opposite}    return e;\n}}\n";
    }

    private static string Main(string position)
    {
        return "[maxvertexcount(4)]\nvoid main(triangle Vertex input[3], inout TriangleStream<Vertex> output)\n{\n"
            + $"    float2 s0 = Screen(input[0].{position});\n    float2 s1 = Screen(input[1].{position});\n    float2 s2 = Screen(input[2].{position});\n"
            + "    float l0 = dot(s1 - s2, s1 - s2);\n    float l1 = dot(s2 - s0, s2 - s0);\n    float l2 = dot(s0 - s1, s0 - s1);\n"
            + "    uint corner = 0;\n    if (l1 > l0 && l1 >= l2) { corner = 1; } else if (l2 > l0 && l2 > l1) { corner = 2; }\n"
            + "    Vertex a = input[corner];\n    Vertex b = input[(corner + 1) % 3];\n    Vertex d = input[(corner + 2) % 3];\n"
            + "    output.Append(d);\n    output.Append(a);\n    output.Append(Opposite(a, b, d));\n    output.Append(b);\n}\n";
    }
}
