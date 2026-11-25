// Copyright (c) 2026 Five Star Recomp contributors.
// SPDX-License-Identifier: GPL-3.0-or-later

using FiveStar.Shaders.Containers;

namespace FiveStar.Shaders.Compilation;

/// <summary>
/// HLSL struct members for PSV0 signature elements: the same type, width, array rows, semantic and interpolation mode,
/// so a generated shader that declares them in signature order gets dxc's prefix-stable packing, the registers of the
/// shader the signature came from.
/// </summary>
public static class SignatureMembers
{
    // HLSL scalar type per DXIL signature component type (1 uint, 2 int, 3 float, 4..6 their 16-bit forms).
    private static readonly string[] ScalarTypes = [string.Empty, "uint", "int", "float", "min16uint", "min16int", "min16float"];

    private static readonly Dictionary<PsvSemanticKind, string> Passed = new()
    {
        [PsvSemanticKind.Position] = "SV_Position",
        [PsvSemanticKind.ClipDistance] = "SV_ClipDistance",
        [PsvSemanticKind.CullDistance] = "SV_CullDistance",
        [PsvSemanticKind.RenderTargetArrayIndex] = "SV_RenderTargetArrayIndex",
        [PsvSemanticKind.ViewportArrayIndex] = "SV_ViewportArrayIndex",
    };

    private static readonly HashSet<PsvSemanticKind> Generated =
    [
        PsvSemanticKind.PrimitiveId, PsvSemanticKind.GsInstanceId, PsvSemanticKind.SampleIndex, PsvSemanticKind.IsFrontFace,
        PsvSemanticKind.Coverage, PsvSemanticKind.InnerCoverage, PsvSemanticKind.ViewId, PsvSemanticKind.Barycentrics,
        PsvSemanticKind.ShadingRate,
    ];

    /// <summary>Whether the rasterizer generates the value (front face, sample index, primitive id, ...).</summary>
    public static bool IsGenerated(PsvElement element)
    {
        return Generated.Contains(element.SemanticKind);
    }

    /// <summary>The member line (four-space indent, newline) of <paramref name="element"/> named <paramref name="name"/>.</summary>
    public static string Member(PsvElement element, string name)
    {
        var semantic = element.Name + element.SemanticIndex;
        var modifier = string.Empty;

        if (element.SemanticKind == PsvSemanticKind.Arbitrary)
        {
            modifier = InterpolationModes.Prefix(element.InterpolationMode);
        }
        else if (Passed.TryGetValue(element.SemanticKind, out var systemName))
        {
            semantic = systemName + element.SemanticIndex;
        }
        else
        {
            throw new NotSupportedException($"pass-through of semantic kind {element.SemanticKind}");
        }

        var type = ScalarTypes[element.ComponentType];

        if (element.Columns > 1)
        {
            type += element.Columns;
        }

        var array = string.Empty;

        if (element.Rows > 1)
        {
            array = $"[{element.Rows}]";
        }

        return $"    {modifier}{type} {name}{array} : {semantic};\n";
    }
}
