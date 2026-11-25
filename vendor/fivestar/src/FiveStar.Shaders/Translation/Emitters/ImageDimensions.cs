// Copyright (c) 2026 Five Star Recomp contributors.
// SPDX-License-Identifier: GPL-3.0-or-later

using FiveStar.Shaders.Translation.Core;

namespace FiveStar.Shaders.Translation.Emitters;

/// <summary>Image dimensions (the dim: modifier) as HLSL texture types, and image address operands.</summary>
internal static class ImageDimensions
{
    private const string DimensionPrefix = "SQ_RSRC_IMG_";
    private const string DefaultDimension = "SQ_RSRC_IMG_2D";
    private const int OneComponent = 1;
    private const int TwoComponents = 2;
    private const int ThreeComponents = 3;

    /// <summary>The dimension name without its prefix: 1D, 2D, 3D, CUBE, 1D_ARRAY, 2D_ARRAY, ...</summary>
    public static string Name(InstructionModifiers modifiers)
    {
        return (modifiers.Value("dim") ?? DefaultDimension).Replace(DimensionPrefix, string.Empty, StringComparison.Ordinal);
    }

    /// <summary>True for array dimensions, whose last address component is the slice.</summary>
    public static bool IsArray(string dimension)
    {
        return dimension.Contains("ARRAY", StringComparison.Ordinal);
    }

    /// <summary>The sampled texture type and its number of address components, or null.</summary>
    public static (string Type, int Components)? Sampled(string dimension)
    {
        switch (dimension)
        {
            case "1D":
                return ("Texture1D", OneComponent);
            case "2D":
                return ("Texture2D", TwoComponents);
            case "3D":
                return ("Texture3D", ThreeComponents);
            case "2D_ARRAY":
                return ("Texture2DArray", ThreeComponents);
            case "1D_ARRAY":
                return ("Texture1DArray", TwoComponents);
            case "CUBE":
                return ("TextureCube", ThreeComponents);
            default:
                return null;
        }
    }

    /// <summary>The writable texture type for image atomics, or null.</summary>
    public static (string Type, int Components)? Writable(string dimension)
    {
        switch (dimension)
        {
            case "1D":
                return ("RWTexture1D", OneComponent);
            case "2D":
                return ("RWTexture2D", TwoComponents);
            case "3D":
                return ("RWTexture3D", ThreeComponents);
            case "2D_ARRAY":
                return ("RWTexture2DArray", ThreeComponents);
            default:
                return null;
        }
    }

    /// <summary>The uint value of every address dword: a VGPR range or an NSA list <c>[v1, v4, v2]</c>.</summary>
    public static List<string> ReadAddress(OperandAccess operands, string text)
    {
        var values = new List<string>();

        if (text.StartsWith('['))
        {
            foreach (var part in text[1..^1].Split(','))
            {
                values.Add(operands.Read(OperandParser.Parse(part.Trim())));
            }

            return values;
        }

        var range = OperandParser.Parse(text);

        for (var dword = 0; dword < range.Width; dword++)
        {
            values.Add(operands.Read(range, dword));
        }

        return values;
    }

    /// <summary>The first <paramref name="components"/> address values as an int vector (or int).</summary>
    public static string IntegerCoordinates(IReadOnlyList<string> coordinates, int components)
    {
        if (components > 1)
        {
            return $"int{components}({string.Join(", ", coordinates.Take(components).Select(value => $"asint({value})"))})";
        }

        return $"asint({coordinates[0]})";
    }
}
