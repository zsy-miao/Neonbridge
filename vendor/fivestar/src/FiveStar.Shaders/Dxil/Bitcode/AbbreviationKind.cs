// Copyright (c) 2026 Five Star Recomp contributors.
// SPDX-License-Identifier: GPL-3.0-or-later

namespace FiveStar.Shaders.Dxil.Bitcode;

/// <summary>Abbreviation operand encodings (LLVM BitCodes.h: 1 fixed, 2 VBR, 3 array); a literal is flagged apart.</summary>
public enum AbbreviationKind
{
    Literal = 0,
    Fixed = 1,
    Vbr = 2,
    Array = 3,
}
