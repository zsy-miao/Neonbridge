// Copyright (c) 2026 Five Star Recomp contributors.
// SPDX-License-Identifier: GPL-3.0-or-later

namespace FiveStar.Shaders.Dxil.Container;

/// <summary>DXIL shader kinds (the program header's and PSV0's numbers).</summary>
public enum DxilShaderKind
{
    Pixel = 0,
    Vertex = 1,
    Geometry = 2,
    Compute = 5,
    Mesh = 13,
}
