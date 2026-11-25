// Copyright (c) 2026 Five Star Recomp contributors.
// SPDX-License-Identifier: GPL-3.0-or-later

namespace FiveStar.Shaders.Dxil.Container;

/// <summary>
/// How a semantic is treated at a signature point (DxilSigPoint.inl): packed into registers (arbitrary, system value,
/// system generated, clip/cull, render target), in the signature without a register (not packed, shadow), or read
/// through an operation of its own (not in the signature).
/// </summary>
public enum SemanticInterpretation
{
    Arbitrary,
    SystemValue,
    SystemGenerated,
    ClipCull,
    Target,
    NotPacked,
    Shadow,
    NotInSignature,
}
