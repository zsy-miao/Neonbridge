// Copyright (c) 2026 Five Star Recomp contributors.
// SPDX-License-Identifier: GPL-3.0-or-later

using System.Runtime.CompilerServices;

namespace FiveStar.Loader.Shims.D3D12X.Resources;

/// <summary>An RGBA sampler border color: four floats, as in D3D12_SAMPLER_DESC::BorderColor.</summary>
[InlineArray(Components)]
internal struct SamplerBorderColor
{
    public const int Components = 4;

    private float _component;
}
