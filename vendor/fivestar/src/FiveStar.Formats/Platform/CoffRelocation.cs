// Copyright (c) 2026 Five Star Recomp contributors.
// SPDX-License-Identifier: GPL-3.0-or-later

namespace FiveStar.Formats.Platform;

/// <summary>A COFF relocation (10 bytes).</summary>
/// <param name="Offset">Offset of the patched field in its section.</param>
/// <param name="Symbol">The target symbol, or null when its index is not a primary symbol record.</param>
/// <param name="Type">The machine-specific type (x64: 4 = REL32).</param>
public sealed record CoffRelocation(uint Offset, CoffSymbol? Symbol, ushort Type);
