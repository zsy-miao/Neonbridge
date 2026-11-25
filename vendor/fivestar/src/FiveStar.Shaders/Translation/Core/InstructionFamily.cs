// Copyright (c) 2026 Five Star Recomp contributors.
// SPDX-License-Identifier: GPL-3.0-or-later

namespace FiveStar.Shaders.Translation.Core;

/// <summary>The instruction families recognised by mnemonic alone, before modifiers are parsed.</summary>
internal enum InstructionFamily
{
    Interpolation,
    Swizzle,
    Lds,
    Barrier,
    Message,
    Other,
}
