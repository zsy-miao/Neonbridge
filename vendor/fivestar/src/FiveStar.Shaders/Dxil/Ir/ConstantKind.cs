// Copyright (c) 2026 Five Star Recomp contributors.
// SPDX-License-Identifier: GPL-3.0-or-later

namespace FiveStar.Shaders.Dxil.Ir;

/// <summary>What a module-level constant holds.</summary>
public enum ConstantKind
{
    Integer,
    FloatingPoint,
    Undefined,
    Aggregate,
    Null,
}
