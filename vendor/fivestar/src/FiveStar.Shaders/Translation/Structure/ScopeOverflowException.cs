// Copyright (c) 2026 Five Star Recomp contributors.
// SPDX-License-Identifier: GPL-3.0-or-later

namespace FiveStar.Shaders.Translation.Structure;

/// <summary>Thrown when the structured code of one scope grows past its limit; that scope is emitted guarded instead.</summary>
internal sealed class ScopeOverflowException : Exception
{
    public ScopeOverflowException(int frame)
        : base($"scope {frame} too large")
    {
        Frame = frame;
    }

    /// <summary>The scope frame that overflowed.</summary>
    public int Frame { get; }
}
