// Copyright (c) 2026 Five Star Recomp contributors.
// SPDX-License-Identifier: GPL-3.0-or-later

namespace FiveStar.Formats.Platform;

/// <summary>A COFF symbol table record.</summary>
/// <param name="Name">The symbol name (short names inline, long names from the string table).</param>
/// <param name="Value">For a defined symbol, its offset in its section.</param>
/// <param name="Section">1-based section number; 0 undefined, negative for absolute and debug symbols.</param>
/// <param name="IsFunction">The type field says "function" (0x20).</param>
public sealed record CoffSymbol(string Name, uint Value, int Section, bool IsFunction);
