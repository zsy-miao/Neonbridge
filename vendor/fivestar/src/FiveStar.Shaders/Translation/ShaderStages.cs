// Copyright (c) 2026 Five Star Recomp contributors.
// SPDX-License-Identifier: GPL-3.0-or-later

using FiveStar.Shaders.Hardware;

namespace FiveStar.Shaders.Translation;

/// <summary>Maps hardware stages to translated stages and names them the way HLSL profiles do.</summary>
public static class ShaderStages
{
    /// <summary>The translated stage of a hardware stage, or null when the stage is not translated.</summary>
    public static ShaderStage? FromHardware(HardwareStage? stage)
    {
        switch (stage)
        {
            case HardwareStage.Pixel:
                return ShaderStage.Pixel;
            case HardwareStage.Vertex:
                return ShaderStage.Vertex;
            case HardwareStage.Compute:
                return ShaderStage.Compute;
            case HardwareStage.Ngg:
                return ShaderStage.Mesh;
            case HardwareStage.Geometry:
                return ShaderStage.Geometry;
            default:
                return null;
        }
    }

    /// <summary>The profile prefix: ps, vs, cs, ms or gs.</summary>
    public static string ProfileName(ShaderStage stage)
    {
        switch (stage)
        {
            case ShaderStage.Pixel:
                return "ps";
            case ShaderStage.Vertex:
                return "vs";
            case ShaderStage.Compute:
                return "cs";
            case ShaderStage.Mesh:
                return "ms";
            case ShaderStage.Geometry:
                return "gs";
            default:
                throw new ArgumentOutOfRangeException(nameof(stage));
        }
    }

    /// <summary>The stage with this profile prefix, or null.</summary>
    public static ShaderStage? FromProfileName(string name)
    {
        foreach (var stage in Enum.GetValues<ShaderStage>())
        {
            if (ProfileName(stage) == name)
            {
                return stage;
            }
        }

        return null;
    }
}
