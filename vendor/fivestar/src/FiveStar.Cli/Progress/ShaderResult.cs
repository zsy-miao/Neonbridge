// Copyright (c) 2026 Five Star Recomp contributors.
// SPDX-License-Identifier: GPL-3.0-or-later

namespace FiveStar.Cli.Progress;

/// <summary>One entry of the translate run's <c>results.json</c>.</summary>
/// <param name="Name">Program name; its <c>.hlsl</c> and <c>.dxil</c> sit next to <c>results.json</c>.</param>
/// <param name="Stage">Stage (<c>ps</c>, <c>vs</c>, <c>cs</c>, <c>ms</c>, <c>gs</c>), or null when unknown.</param>
/// <param name="Status"><c>ok</c>, <c>ok-msfb</c> (compiled through the mesh-shader fallback) or a failure kind.</param>
/// <param name="Reason">Failure detail, or null.</param>
public sealed record ShaderResult(string Name, string? Stage, string Status, string? Reason);
