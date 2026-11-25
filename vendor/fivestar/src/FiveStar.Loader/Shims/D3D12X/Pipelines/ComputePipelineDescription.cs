// Copyright (c) 2026 Five Star Recomp contributors.
// SPDX-License-Identifier: GPL-3.0-or-later

namespace FiveStar.Loader.Shims.D3D12X.Pipelines;

/// <summary>A compute pipeline as the game described it.</summary>
/// <param name="GameRootSignature">The game's root signature (kept for binding translation; the pipeline uses ours).</param>
/// <param name="Shader">The compute shader as the game handed it over.</param>
internal sealed record ComputePipelineDescription(nint GameRootSignature, ShaderSource Shader);
