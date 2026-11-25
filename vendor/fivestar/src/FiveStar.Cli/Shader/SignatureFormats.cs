// Copyright (c) 2026 Five Star Recomp contributors.
// SPDX-License-Identifier: GPL-3.0-or-later

using FiveStar.Shaders.Containers;
using Vortice.Direct3D12;
using Vortice.DXGI;

namespace FiveStar.Cli.Shader;

/// <summary>DXGI formats for the input layout of a vertex shader and the targets of a pixel shader, from their signatures.</summary>
internal static class SignatureFormats
{
    // DXIL signature component types (PSV0): 1 uint, 2 int, 3 float, 4 min16uint, 5 min16int, 6 min16float.
    private const int UintType = 1;
    private const int IntType = 2;
    private const int Min16UintType = 4;
    private const int Min16IntType = 5;
    private const int Min16FloatType = 6;

    private static readonly Format[] FloatFormats = [Format.R32_Float, Format.R32G32_Float, Format.R32G32B32_Float, Format.R32G32B32A32_Float];
    private static readonly Format[] UintFormats = [Format.R32_UInt, Format.R32G32_UInt, Format.R32G32B32_UInt, Format.R32G32B32A32_UInt];
    private static readonly Format[] IntFormats = [Format.R32_SInt, Format.R32G32_SInt, Format.R32G32B32_SInt, Format.R32G32B32A32_SInt];

    /// <summary>One element per non-system ISG1 element, packed in vertex buffer slot 0 (min-precision types read 32-bit data).</summary>
    public static List<InputElementDescription> InputLayout(IReadOnlyList<SignatureElement> inputs)
    {
        var layout = new List<InputElementDescription>();

        foreach (var element in inputs.Where(element => element.SystemValue.Length == 0))
        {
            var formats = FloatFormats;

            if (element.ComponentType.EndsWith("uint", StringComparison.Ordinal))
            {
                formats = UintFormats;
            }
            else if (element.ComponentType.EndsWith("int", StringComparison.Ordinal))
            {
                formats = IntFormats;
            }

            var format = formats[Math.Max(element.Mask.Length, 1) - 1];
            layout.Add(new InputElementDescription(element.Name, (uint)element.SemanticIndex, format, InputElementDescription.AppendAligned, 0));
        }

        return layout;
    }

    /// <summary>Render-target formats indexed by SV_Target number (gaps Unknown), four components of the output's type.</summary>
    public static List<Format> Targets(IReadOnlyList<PsvElement> outputs)
    {
        var targets = new List<Format>();

        foreach (var element in outputs.Where(element => element.SemanticKind == PsvSemanticKind.Target))
        {
            for (var row = 0; row < element.Rows; row++)
            {
                var slot = element.SemanticIndex + row;

                while (targets.Count <= slot)
                {
                    targets.Add(Format.Unknown);
                }

                targets[slot] = TargetFormat(element.ComponentType);
            }
        }

        return targets;
    }

    /// <summary>D32_FLOAT for a depth output, D32_FLOAT_S8X24_UINT with a stencil reference, otherwise Unknown.</summary>
    public static Format Depth(IReadOnlyList<PsvElement> outputs)
    {
        if (outputs.Any(element => element.SemanticKind == PsvSemanticKind.StencilRef))
        {
            return Format.D32_Float_S8X24_UInt;
        }

        var depthKinds = new[] { PsvSemanticKind.Depth, PsvSemanticKind.DepthLessEqual, PsvSemanticKind.DepthGreaterEqual };

        if (outputs.Any(element => depthKinds.Contains(element.SemanticKind)))
        {
            return Format.D32_Float;
        }

        return Format.Unknown;
    }

    private static Format TargetFormat(int componentType)
    {
        switch (componentType)
        {
            case UintType:
                return Format.R32G32B32A32_UInt;
            case IntType:
                return Format.R32G32B32A32_SInt;
            case Min16UintType:
                return Format.R16G16B16A16_UInt;
            case Min16IntType:
                return Format.R16G16B16A16_SInt;
            case Min16FloatType:
                return Format.R16G16B16A16_Float;
            default:
                return Format.R32G32B32A32_Float;
        }
    }
}
