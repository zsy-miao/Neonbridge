// Copyright (c) 2026 Five Star Recomp contributors.
// SPDX-License-Identifier: GPL-3.0-or-later

namespace FiveStar.Shaders.Containers;

/// <summary>The input and output signature elements of a PSV0 chunk, in declaration order.</summary>
public sealed record PsvSignatures(IReadOnlyList<PsvElement> Inputs, IReadOnlyList<PsvElement> Outputs);
