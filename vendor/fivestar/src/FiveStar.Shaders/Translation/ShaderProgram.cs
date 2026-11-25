// Copyright (c) 2026 Five Star Recomp contributors.
// SPDX-License-Identifier: GPL-3.0-or-later

using FiveStar.Shaders.Containers;
using FiveStar.Shaders.Disassembly;
using FiveStar.Shaders.Hardware;

namespace FiveStar.Shaders.Translation;

/// <summary>
/// Everything the translator reads about one program: its stage, the disassembled instructions, the raw code bytes
/// (for PC-relative constant tables), the signatures and PSV0 info of its DXBC container, and the hardware config from
/// its FSSM header. Geometry programs also carry the disassembled VS copy shader.
/// </summary>
public sealed record ShaderProgram(
    ShaderStage Stage,
    IReadOnlyList<Instruction> Instructions,
    byte[]? Code,
    ShaderSignatures Signatures,
    ShaderConfig Config,
    PipelineStateInfo? PipelineState)
{
    /// <summary>The disassembled copy shader of a geometry program.</summary>
    public IReadOnlyList<Instruction>? CopyShader { get; init; }
}
