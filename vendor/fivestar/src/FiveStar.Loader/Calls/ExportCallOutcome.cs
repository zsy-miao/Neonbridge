// Copyright (c) 2026 Five Star Recomp contributors.
// SPDX-License-Identifier: GPL-3.0-or-later

namespace FiveStar.Loader.Calls;

/// <summary>One recipe run: its alias and result, or the reason it could not run (module not loaded, export missing).</summary>
internal sealed record ExportCallOutcome(string Alias, ExportCallResult? Result, string? Error);
