// Copyright (c) 2026 Five Star Recomp contributors.
// SPDX-License-Identifier: GPL-3.0-or-later

namespace FiveStar.Formats.Platform;

/// <summary>How many times a module's code calls one imported function through its IAT slot.</summary>
/// <param name="Dll">The DLL the function is imported from.</param>
/// <param name="Function">The function name (or <c>#N</c> for an ordinal import).</param>
/// <param name="Delayed">True for a delay-load import.</param>
/// <param name="SlotRva">RVA of the import address table slot the call sites reference.</param>
/// <param name="Sites">Number of <c>call</c>/<c>jmp</c> instructions in the code that target the slot.</param>
public sealed record ImportCallSite(string Dll, string Function, bool Delayed, uint SlotRva, int Sites);
