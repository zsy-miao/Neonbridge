// Copyright (c) 2026 Five Star Recomp contributors.
// SPDX-License-Identifier: GPL-3.0-or-later

namespace FiveStar.Loader.Shims.D3D12X.Pipelines;

/// <summary>A pipeline ready to build: its desktop stream (owned), the description it came from and its programs.</summary>
/// <param name="Stream">The desktop stream; whoever builds or drops the recipe disposes it.</param>
/// <param name="Desktop">The graphics description with the translated vertex elements; null for compute.</param>
/// <param name="Programs">The translated programs, in stream order.</param>
/// <param name="Names">The program names joined with " + ", for messages.</param>
internal sealed record PipelineRecipe(DesktopStream Stream, GraphicsPipelineDescription? Desktop, IReadOnlyList<TranslatedProgram> Programs, string Names);
