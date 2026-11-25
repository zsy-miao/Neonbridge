// Copyright (c) 2026 Five Star Recomp contributors.
// SPDX-License-Identifier: GPL-3.0-or-later

namespace FiveStar.Loader.Shims.D3D12X.Pipelines;

/// <summary>One pipeline of the console pipeline cache for the warm-up: exactly one of the two descriptions is set.</summary>
/// <param name="Record">The record's index in its table, for messages.</param>
/// <param name="Graphics">The description slot 57 would get for a graphics record.</param>
/// <param name="Compute">The description slot 67 would get for a compute record.</param>
internal sealed record WarmTarget(int Record, GraphicsPipelineDescription? Graphics, ComputePipelineDescription? Compute);
