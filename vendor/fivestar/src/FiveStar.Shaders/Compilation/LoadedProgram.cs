// Copyright (c) 2026 Five Star Recomp contributors.
// SPDX-License-Identifier: GPL-3.0-or-later

using FiveStar.Shaders.Translation;

namespace FiveStar.Shaders.Compilation;

/// <summary>
/// The outcome of loading one FSSM file: the program ready to translate, or the status (with reason) that stopped it.
/// <see cref="Stage"/> is known once the header decoded.
/// </summary>
public sealed record LoadedProgram(string Name, ShaderStage? Stage, ShaderProgram? Program, string? Status, string? Reason);
