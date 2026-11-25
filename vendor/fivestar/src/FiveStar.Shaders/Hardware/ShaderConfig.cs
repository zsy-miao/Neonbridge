// Copyright (c) 2026 Five Star Recomp contributors.
// SPDX-License-Identifier: GPL-3.0-or-later

namespace FiveStar.Shaders.Hardware;

/// <summary>
/// The hardware configuration decoded from an FSSM program header. Every member is null when unknown (for example
/// for the VS copy shader of a geometry program, which has no header of its own); the stage-specific blocks are
/// null for other stages.
/// </summary>
public sealed record ShaderConfig
{
    public HardwareStage? Stage { get; init; }

    public long? CodeOffset { get; init; }

    public long? CodeSize { get; init; }

    public uint? Flags { get; init; }

    /// <summary>32 or 64 lanes.</summary>
    public int? WaveSize { get; init; }

    public int? UserSgprs { get; init; }

    public int? FloatMode { get; init; }

    public int? LdsBytes { get; init; }

    public int? VgprCount { get; init; }

    public int? SgprCount { get; init; }

    public int? ResourceRecordCount { get; init; }

    public PixelConfig? Pixel { get; init; }

    public VertexConfig? Vertex { get; init; }

    public ComputeConfig? Compute { get; init; }

    public NggConfig? Ngg { get; init; }

    /// <summary>Header offset of the copy-shader program block of a geometry program.</summary>
    public int? CopyShaderBlock { get; init; }
}
