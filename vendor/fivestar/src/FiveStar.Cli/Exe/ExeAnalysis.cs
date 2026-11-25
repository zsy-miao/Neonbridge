// Copyright (c) 2026 Five Star Recomp contributors.
// SPDX-License-Identifier: GPL-3.0-or-later

namespace FiveStar.Cli.Exe;

/// <summary>Everything the report shows about one executable; the writers turn it into Markdown and JSON.</summary>
public sealed record ExeAnalysis
{
    public required string FileName { get; init; }

    public required long FileLength { get; init; }

    public required bool Is64Bit { get; init; }

    public required ushort Machine { get; init; }

    public required uint EntryPoint { get; init; }

    public required ulong ImageBase { get; init; }

    public required uint TimeStamp { get; init; }

    public required ushort Subsystem { get; init; }

    public required ushort DllCharacteristics { get; init; }

    public required IReadOnlyList<SectionInfo> Sections { get; init; }

    public required IReadOnlyList<uint> TlsCallbacks { get; init; }

    public required int PdataFunctions { get; init; }

    public required uint PdataBytes { get; init; }

    public required IReadOnlyList<string> Exports { get; init; }

    public required IReadOnlyList<ImportGap> Imports { get; init; }

    /// <summary>True when the loader classified the imports; false when fivestar-loader was not found or failed.</summary>
    public required bool LoaderClassified { get; init; }

    public required IReadOnlyList<GdkCall> GdkCalls { get; init; }

    public required int GdkThunkCount { get; init; }

    public required int GdkNamedCount { get; init; }

    public string? GdkLibName { get; init; }

    public required IReadOnlyList<ImportGap> GraphicsImports { get; init; }

    public required IReadOnlyList<SymbolReference> ArchiveReferences { get; init; }

    public required IReadOnlyList<SymbolReference> ShaderReferences { get; init; }

    public required IReadOnlyList<GapItem> DayOneGaps { get; init; }
}
