// Copyright (c) 2026 Five Star Recomp contributors.
// SPDX-License-Identifier: GPL-3.0-or-later

using System.Runtime.CompilerServices;

namespace FiveStar.Loader.Shims.Xg;

/// <summary>The mips of a plane, up to D3D12_REQ_MIP_LEVELS.</summary>
[InlineArray(XgLimits.MaxMips)]
internal struct XgMipArray
{
    private XgMip _mip;
}
