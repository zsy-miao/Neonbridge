// Copyright (c) 2026 Five Star Recomp contributors.
// SPDX-License-Identifier: GPL-3.0-or-later

namespace FiveStar.Cli.Exe;

/// <summary>A place in the image that refers to a known constant or string, reported with its address.</summary>
/// <param name="What">What was found (the constant or the string).</param>
/// <param name="Rva">RVA of the reference.</param>
/// <param name="Section">The section the reference sits in, or <c>?</c>.</param>
/// <param name="Note">How it was recognised (an immediate, an ASCII string).</param>
public sealed record SymbolReference(string What, uint Rva, string Section, string Note);
