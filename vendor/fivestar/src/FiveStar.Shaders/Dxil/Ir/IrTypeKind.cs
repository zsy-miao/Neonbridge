// Copyright (c) 2026 Five Star Recomp contributors.
// SPDX-License-Identifier: GPL-3.0-or-later

namespace FiveStar.Shaders.Dxil.Ir;

/// <summary>The kinds of LLVM types a DXIL module uses.</summary>
public enum IrTypeKind
{
    Void,
    Integer,
    Half,
    Float,
    Double,
    Pointer,
    Struct,
    Array,
    Function,
}
