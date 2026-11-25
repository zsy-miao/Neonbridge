// Copyright (c) 2026 Five Star Recomp contributors.
// SPDX-License-Identifier: GPL-3.0-or-later

namespace FiveStar.Cli.Exe;

/// <summary>The low-level PE facts the report leads with.</summary>
/// <param name="Sections">The sections, with entropy and the packed and console flags.</param>
/// <param name="TlsCallbacks">RVAs of the TLS callbacks.</param>
/// <param name="PdataFunctions">RUNTIME_FUNCTION entries in the exception directory.</param>
/// <param name="PdataBytes">Size of the exception directory in bytes.</param>
/// <param name="Exports">Export names (empty for a typical executable).</param>
public sealed record PeBasics(
    IReadOnlyList<SectionInfo> Sections,
    IReadOnlyList<uint> TlsCallbacks,
    int PdataFunctions,
    uint PdataBytes,
    IReadOnlyList<string> Exports);
