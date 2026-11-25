// Copyright (c) 2026 Five Star Recomp contributors.
// SPDX-License-Identifier: GPL-3.0-or-later

namespace FiveStar.Shaders.Dxil.Hlsl;

/// <summary>The kinds of HLSL tokens.</summary>
public enum TokenKind
{
    End,
    Identifier,
    Integer,
    Real,
    Text,
    Punctuator,
}
