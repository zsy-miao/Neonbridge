// Copyright (c) 2026 Five Star Recomp contributors.
// SPDX-License-Identifier: GPL-3.0-or-later

using System.Runtime.CompilerServices;

namespace FiveStar.Loader.Shims.Xg;

/// <summary>The planes of a surface.</summary>
[InlineArray(XgLimits.MaxPlanes)]
internal struct XgPlaneArray
{
    private XgPlane _plane;
}
