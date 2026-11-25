// Copyright (c) 2026 Five Star Recomp contributors.
// SPDX-License-Identifier: GPL-3.0-or-later

namespace FiveStar.Shaders.Cache;

/// <summary>
/// A <c>D3D12_RASTERIZER_DESC</c> from PA_SU_SC_MODE_CNTL, PA_CL_CLIP_CNTL and the polygon-offset registers. Fill and
/// cull mode are D3D12 enum values (0 = no D3D12 equivalent). The depth bias is the hardware's: slope scale is the
/// register over 16, the constant bias the offset register as is.
/// </summary>
public sealed record RasterizerState(
    int FillMode,
    int CullMode,
    bool FrontCounterClockwise,
    int DepthBias,
    float DepthBiasClamp,
    float SlopeScaledDepthBias,
    bool DepthClipEnable);
