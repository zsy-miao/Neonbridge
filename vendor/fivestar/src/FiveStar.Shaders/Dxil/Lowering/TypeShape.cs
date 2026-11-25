// Copyright (c) 2026 Five Star Recomp contributors.
// SPDX-License-Identifier: GPL-3.0-or-later

namespace FiveStar.Shaders.Dxil.Lowering;

/// <summary>The shapes of HLSL types.</summary>
public enum TypeShape
{
    Void,
    Scalar,
    Vector,
    Matrix,
    Array,
    Struct,
    Resource,
}
