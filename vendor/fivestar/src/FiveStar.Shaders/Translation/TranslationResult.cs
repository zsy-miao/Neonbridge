// Copyright (c) 2026 Five Star Recomp contributors.
// SPDX-License-Identifier: GPL-3.0-or-later

namespace FiveStar.Shaders.Translation;

/// <summary>A translated program: HLSL source (entry point <c>main</c>) that includes the runtime headers.</summary>
public sealed record TranslationResult(ShaderStage Stage, string Hlsl);
