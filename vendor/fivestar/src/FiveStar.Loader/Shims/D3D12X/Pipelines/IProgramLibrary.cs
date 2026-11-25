// Copyright (c) 2026 Five Star Recomp contributors.
// SPDX-License-Identifier: GPL-3.0-or-later

namespace FiveStar.Loader.Shims.D3D12X.Pipelines;

/// <summary>The translated programs, found by the 24-byte hash prefix of their code block.</summary>
internal interface IProgramLibrary
{
    /// <summary>The program whose code starts with <paramref name="prefix"/>, or null when none was translated.</summary>
    TranslatedProgram? Find(ReadOnlySpan<byte> prefix);

    /// <summary>The extracted programs whose code starts with <paramref name="prefix"/>, translated or not.</summary>
    IReadOnlyList<string> Names(ReadOnlySpan<byte> prefix);

    /// <summary>The translated program named <paramref name="name"/>, or null when it has no loadable form.</summary>
    TranslatedProgram? Load(string name);

    /// <summary>Bytes of DXIL the library keeps in memory.</summary>
    long HeldBytes { get; }
}
