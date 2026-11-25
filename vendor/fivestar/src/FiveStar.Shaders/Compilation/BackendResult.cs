// Copyright (c) 2026 Five Star Recomp contributors.
// SPDX-License-Identifier: GPL-3.0-or-later

namespace FiveStar.Shaders.Compilation;

/// <summary>A verdict of the DXIL backend and, when the program was not compiled, why.</summary>
public sealed record BackendResult(BackendVerdict Verdict, string Reason)
{
    public static BackendResult Compiled { get; } = new(BackendVerdict.Compiled, string.Empty);
}
