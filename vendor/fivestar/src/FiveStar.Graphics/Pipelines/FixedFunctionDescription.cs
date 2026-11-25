// Copyright (c) 2026 Five Star Recomp contributors.
// SPDX-License-Identifier: GPL-3.0-or-later

using Vortice.Direct3D12;

namespace FiveStar.Graphics.Pipelines;

/// <summary>The blend, rasterizer and depth-stencil state of a graphics pipeline (for instance from the console's pipeline cache).</summary>
public sealed record FixedFunctionDescription(BlendDescription Blend, RasterizerDescription Rasterizer, DepthStencilDescription DepthStencil);
