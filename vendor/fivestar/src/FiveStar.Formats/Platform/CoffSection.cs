// Copyright (c) 2026 Five Star Recomp contributors.
// SPDX-License-Identifier: GPL-3.0-or-later

namespace FiveStar.Formats.Platform;

/// <summary>A section of a COFF object: its raw data and relocations.</summary>
/// <param name="Name">The section name, such as <c>.text$mn</c>.</param>
/// <param name="Data">The raw data (empty for uninitialized sections).</param>
/// <param name="Relocations">The relocations, in file order.</param>
public sealed record CoffSection(string Name, ReadOnlyMemory<byte> Data, IReadOnlyList<CoffRelocation> Relocations);
