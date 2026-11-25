// Copyright (c) 2026 Five Star Recomp contributors.
// SPDX-License-Identifier: GPL-3.0-or-later

namespace FiveStar.Debugger.Symbols;

/// <summary>
/// A symbol store in the symstore layout (<c>name.pdb\GUIDage\name.pdb</c>): a local or shared folder, and the server
/// that fills it when the PDB is missing.
/// </summary>
internal sealed record SymbolStore(string Cache, string? Server);
