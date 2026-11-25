// Copyright (c) 2026 Five Star Recomp contributors.
// SPDX-License-Identifier: GPL-3.0-or-later

using System.Text;
using FiveStar.Shaders.Containers;

namespace FiveStar.Shaders.Compilation;

/// <summary>
/// Writes a vertex shader whose outputs match the input signature of a compiled PS or GS: the same elements in
/// declaration order with the same types, widths, array rows and interpolation modes, so dxc's prefix-stable packing
/// puts them in the same registers. Values the rasterizer generates (front face, sample index, primitive id, ...) are
/// left out; SV_Position is appended when the consumer does not read it but needs it.
/// </summary>
public static class PassThroughVertexShader
{
    /// <summary>The HLSL source (entry point main) for the consumer's inputs.</summary>
    public static string Source(IReadOnlyList<PsvElement> consumerInputs, bool needsPosition)
    {
        var members = new StringBuilder();
        var hasPosition = false;
        var count = 0;

        foreach (var element in consumerInputs)
        {
            if (SignatureMembers.IsGenerated(element))
            {
                continue;
            }

            hasPosition |= element.SemanticKind == PsvSemanticKind.Position;
            members.Append(SignatureMembers.Member(element, $"e{count}"));
            count++;
        }

        if (needsPosition && !hasPosition)
        {
            members.Append("    float4 position : SV_Position;\n");
        }

        return $"struct PassOut\n{{\n{members}}};\n\nPassOut main()\n{{\n    PassOut o = (PassOut)0;\n    return o;\n}}\n";
    }
}
