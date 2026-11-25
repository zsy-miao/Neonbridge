// Copyright (c) 2026 Five Star Recomp contributors.
// SPDX-License-Identifier: GPL-3.0-or-later

using FiveStar.Shaders.Translation;

namespace FiveStar.Shaders.Compilation;

/// <summary>
/// One program after loading and translation: the HLSL when it translated, otherwise the status and reason that stopped
/// it. <see cref="Stage"/> is known once the header decoded; <see cref="Instructions"/> counts the disassembled code.
/// </summary>
public sealed record TranslatedProgram(string Name, ShaderStage? Stage, string? Hlsl, string? Status, string? Reason, int Instructions)
{
    /// <summary>The wave32 variant of a wave64-pinned compute program; null for every other program.</summary>
    public GroupWaveVariant? GroupWave { get; init; }
}
