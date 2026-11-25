// Copyright (c) 2026 Five Star Recomp contributors.
// SPDX-License-Identifier: GPL-3.0-or-later

using FiveStar.Shaders.Containers;

namespace FiveStar.Shaders.Translation.Stages;

/// <summary>
/// Maps AMD attribute/parameter numbers onto signature elements: attrN (PS) and paramN (VS, mesh) are the N-th
/// non-system signature register, so the translated programs keep the original semantic names, masks and types.
/// </summary>
internal static class SignatureLayout
{
    /// <summary>System values read through their own PS inputs rather than attributes.</summary>
    public static readonly IReadOnlySet<string> PixelSystemInputs = new HashSet<string>
    {
        "SV_Position", "SV_IsFrontFace", "SV_SampleIndex", "SV_Coverage", "SV_PrimitiveID", "SV_ClipDistance",
        "SV_CullDistance", "SV_RenderTargetArrayIndex", "SV_ViewportArrayIndex",
    };

    /// <summary>System values a vertex or mesh shader exports through pos exports rather than params.</summary>
    public static readonly IReadOnlySet<string> VertexSystemOutputs = new HashSet<string>
    {
        "SV_Position", "SV_ClipDistance", "SV_CullDistance", "SV_RenderTargetArrayIndex", "SV_ViewportArrayIndex",
    };

    /// <summary>Non-system elements grouped by register (in register order), each group sorted by mask.</summary>
    public static List<List<SignatureElement>> Registers(IReadOnlyList<SignatureElement> elements, IReadOnlySet<string> system)
    {
        var byRegister = new SortedDictionary<int, List<SignatureElement>>();

        foreach (var element in elements)
        {
            if (system.Contains(element.SystemValue) || system.Contains(element.Name))
            {
                continue;
            }

            if (!byRegister.TryGetValue(element.Register, out var group))
            {
                group = [];
                byRegister[element.Register] = group;
            }

            group.Add(element);
        }

        return byRegister.Values.Select(group => group.OrderBy(element => element.Mask, StringComparer.Ordinal).ToList()).ToList();
    }

    /// <summary>The HLSL scalar type of an element: uint, int or float (min-precision types read as float).</summary>
    public static string TypeName(SignatureElement? element)
    {
        if (element is null)
        {
            return "float";
        }

        switch (element.ComponentType)
        {
            case "uint":
                return "uint";
            case "int":
                return "int";
            default:
                return "float";
        }
    }

    /// <summary>The vector suffix of a mask: nothing for one component, else the component count.</summary>
    public static string Width(string mask)
    {
        if (mask.Length > 1)
        {
            return mask.Length.ToString(System.Globalization.CultureInfo.InvariantCulture);
        }

        return string.Empty;
    }

    /// <summary>A signature value as the float the translated program works with.</summary>
    public static string ToFloat(SignatureElement element, string value)
    {
        if (TypeName(element) == "float")
        {
            return value;
        }

        return $"asfloat({value})";
    }

    /// <summary>A float export value as the element's type.</summary>
    public static string FromFloat(SignatureElement element, string value)
    {
        var type = TypeName(element);

        if (type == "float")
        {
            return value;
        }

        return $"as{type}({value})";
    }
}
