// Copyright (c) 2026 Five Star Recomp contributors.
// SPDX-License-Identifier: GPL-3.0-or-later

namespace FiveStar.Formats.Platform;

/// <summary>The functions a PE image imports from one DLL; ordinal-only imports are named <c>#N</c>.</summary>
/// <param name="Dll">The DLL name as stored.</param>
/// <param name="Functions">Function names in table order.</param>
/// <param name="Delayed">True for a delay-load import.</param>
/// <param name="FirstThunkRva">RVA of the import address table: slot <c>i</c> of <see cref="Functions"/> is a pointer at <c>FirstThunkRva + i * pointer size</c>.</param>
public sealed record PeImport(string Dll, IReadOnlyList<string> Functions, bool Delayed, uint FirstThunkRva);
