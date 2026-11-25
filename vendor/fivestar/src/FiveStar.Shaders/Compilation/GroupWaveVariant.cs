// Copyright (c) 2026 Five Star Recomp contributors.
// SPDX-License-Identifier: GPL-3.0-or-later

namespace FiveStar.Shaders.Compilation;

/// <summary>
/// The wave32 variant of a wave64-pinned compute program (<see cref="Translation.HlslTranslator.GroupWaveVariant"/>): its
/// HLSL, or the reason it could not be built.
/// </summary>
public sealed record GroupWaveVariant(string? Hlsl, string? Reason);
