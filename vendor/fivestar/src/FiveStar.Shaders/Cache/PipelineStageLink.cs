// Copyright (c) 2026 Five Star Recomp contributors.
// SPDX-License-Identifier: GPL-3.0-or-later

namespace FiveStar.Shaders.Cache;

/// <summary>One used shader slot of a pipeline: the shader index entry and the shader's metadata entry.</summary>
public readonly record struct PipelineStageLink(PipelineSlot Slot, int Shader, int Meta);
