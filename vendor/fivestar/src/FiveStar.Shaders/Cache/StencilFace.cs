// Copyright (c) 2026 Five Star Recomp contributors.
// SPDX-License-Identifier: GPL-3.0-or-later

namespace FiveStar.Shaders.Cache;

/// <summary>A <c>D3D12_DEPTH_STENCILOP_DESC</c>: D3D12 stencil operations and comparison (0 = no D3D12 equivalent).</summary>
public sealed record StencilFace(int FailOp, int DepthFailOp, int PassOp, int Func);
