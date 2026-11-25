// Copyright (c) 2026 Five Star Recomp contributors.
// SPDX-License-Identifier: GPL-3.0-or-later

namespace FiveStar.Cli.Exe;

/// <summary>One imported function with the loader's verdict and how often the code calls it.</summary>
/// <param name="Dll">The DLL the function is imported from.</param>
/// <param name="Function">The function name (or <c>#N</c> for an ordinal import).</param>
/// <param name="Delayed">True for a delay-load import.</param>
/// <param name="Kind">The loader's class: <c>Shim</c>, <c>XboxStub</c>, <c>Host</c> or <c>Unclassified</c>.</param>
/// <param name="Detail">The shim behind it or the reason nothing in the loader answers.</param>
/// <param name="Sites">Number of call sites in the code.</param>
public sealed record ImportGap(string Dll, string Function, bool Delayed, string Kind, string Detail, int Sites);
