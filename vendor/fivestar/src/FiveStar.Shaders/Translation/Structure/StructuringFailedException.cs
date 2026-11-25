// Copyright (c) 2026 Five Star Recomp contributors.
// SPDX-License-Identifier: GPL-3.0-or-later

namespace FiveStar.Shaders.Translation.Structure;

/// <summary>Thrown when a control-flow graph cannot be emitted as structured code; the program keeps the dispatch form.</summary>
internal sealed class StructuringFailedException : Exception
{
    public StructuringFailedException(string message)
        : base(message)
    {
    }
}
