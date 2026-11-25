// Copyright (c) 2026 Five Star Recomp contributors.
// SPDX-License-Identifier: GPL-3.0-or-later

namespace FiveStar.Loader.Calls;

/// <summary>
/// An export that was disassembled before being called: the module and export, a short alias, what the disassembly
/// showed, how to call it, and whether "--call all" includes it.
/// </summary>
internal sealed record ExportCallRecipe(string Module, string Export, string Alias, string Why, Func<ExportCallContext, ExportCallResult> Run, bool InAll);
