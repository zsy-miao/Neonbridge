// Copyright (c) 2026 Five Star Recomp contributors.
// SPDX-License-Identifier: GPL-3.0-or-later

namespace FiveStar.Shaders.Cache;

/// <summary>
/// One render target's <c>D3D12_RENDER_TARGET_BLEND_DESC</c>, decoded from CB_BLENDn_CONTROL and CB_TARGET_MASK.
/// Factors and operations are D3D12 enum values; 0 marks a hardware value without a D3D12 equivalent.
/// </summary>
public sealed record BlendTarget(
    bool Enable,
    int SrcBlend,
    int DestBlend,
    int BlendOp,
    int SrcBlendAlpha,
    int DestBlendAlpha,
    int BlendOpAlpha,
    int WriteMask);
