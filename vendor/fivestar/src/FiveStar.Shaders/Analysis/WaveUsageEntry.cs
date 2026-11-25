// Copyright (c) 2026 Five Star Recomp contributors.
// SPDX-License-Identifier: GPL-3.0-or-later

namespace FiveStar.Shaders.Analysis;

/// <summary>
/// One translated program's wave classification and the cross-lane operations it uses. <paramref name="Floor"/> is the
/// narrowest hardware wave with console results when the program was translated with wave emulation, else null.
/// </summary>
public sealed record WaveUsageEntry(
    string Name, string? Stage, int WaveSize, WaveDependence Dependence, IReadOnlySet<string> DataOperations, int? Floor);
