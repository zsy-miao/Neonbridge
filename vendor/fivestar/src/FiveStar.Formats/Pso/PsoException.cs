// Copyright (c) 2026 Five Star Recomp contributors.
// SPDX-License-Identifier: GPL-3.0-or-later

namespace FiveStar.Formats.Pso;

/// <summary>A PSO file or resource that does not parse or walk.</summary>
public sealed class PsoException : Exception
{
    /// <param name="message">What is wrong.</param>
    /// <param name="located">True once the message names the member, type and struct where the walk failed.</param>
    public PsoException(string message, bool located)
        : base(message)
    {
        Located = located;
    }

    /// <summary>Gets a value indicating whether the message already names the failing member.</summary>
    public bool Located { get; }
}
