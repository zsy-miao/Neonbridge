// Copyright (c) 2026 Five Star Recomp contributors.
// SPDX-License-Identifier: GPL-3.0-or-later

using FiveStar.Shaders.Translation;

namespace FiveStar.Shaders.Tests.Differential;

/// <summary>
/// One control-flow form ready to run: its harness source and profile, the shader (and a pixel shader's feeder VS, its
/// target count, a vertex wrapper's output size), or why it could not be built.
/// </summary>
internal sealed record PreparedForm(
    string Name, ShaderStage Stage, string Source, string Profile, byte[] Shader, byte[]? Feeder, int Targets, int OutputBytes, string? Error);
