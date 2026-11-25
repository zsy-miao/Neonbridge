// Copyright (c) 2026 Five Star Recomp contributors.
// SPDX-License-Identifier: GPL-3.0-or-later

namespace FiveStar.Formats.Platform;

/// <summary>How many GDK thunks of a module the library names, and the ones it does not.</summary>
/// <param name="Module">The module file name.</param>
/// <param name="Thunks">Distinct (clsid, iid, slot) thunks found in the module.</param>
/// <param name="Unnamed">Thunks without a library entry (console-only functions).</param>
public sealed record GdkModuleCoverage(string Module, int Thunks, IReadOnlyList<GdkThunk> Unnamed);
