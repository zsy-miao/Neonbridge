// Copyright (c) 2026 Five Star Recomp contributors.
// SPDX-License-Identifier: GPL-3.0-or-later

namespace FiveStar.Shaders.Dxil.Lowering;

/// <summary>
/// HLSL scalar types. Literals keep their exact value until an operation gives them a type; half and min-precision
/// types are 32-bit, as HLSL compiles them without 16-bit types.
/// </summary>
public enum ScalarKind
{
    Bool,
    Int,
    Uint,
    Float,
    Double,
    Int64,
    Uint64,
    LiteralInt,
    LiteralFloat,
}
