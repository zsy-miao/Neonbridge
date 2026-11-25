// Copyright (c) 2026 Five Star Recomp contributors.
// SPDX-License-Identifier: GPL-3.0-or-later

namespace FiveStar.Shaders.Translation;

/// <summary>Thrown when a program uses an instruction, operand or shape the translator does not handle.</summary>
public sealed class UnsupportedProgramException : Exception
{
    public UnsupportedProgramException(string message)
        : base(message)
    {
    }
}
