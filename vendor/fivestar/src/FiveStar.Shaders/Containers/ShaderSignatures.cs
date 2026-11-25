// Copyright (c) 2026 Five Star Recomp contributors.
// SPDX-License-Identifier: GPL-3.0-or-later

namespace FiveStar.Shaders.Containers;

/// <summary>The input (ISG1) and output (OSG1) signatures of a program; either list may be empty.</summary>
public sealed record ShaderSignatures(IReadOnlyList<SignatureElement> Inputs, IReadOnlyList<SignatureElement> Outputs)
{
    /// <summary>Signatures with no elements, for programs whose container is not available.</summary>
    public static ShaderSignatures None { get; } = new([], []);
}
