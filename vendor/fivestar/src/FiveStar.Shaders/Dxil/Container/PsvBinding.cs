// Copyright (c) 2026 Five Star Recomp contributors.
// SPDX-License-Identifier: GPL-3.0-or-later

namespace FiveStar.Shaders.Dxil.Container;

/// <summary>A resource bound by register, as PSV0 lists it: PSV type (1 sampler, 2 CBV), space, register, DXIL kind.</summary>
public sealed record PsvBinding(uint Type, uint Space, uint Register, uint Kind);
