// Copyright (c) 2026 Five Star Recomp contributors.
// SPDX-License-Identifier: GPL-3.0-or-later

namespace FiveStar.Loader.Loading;

/// <summary>One bound import: what was asked for, where it was found and the address written to the IAT.</summary>
internal sealed record ImportRecord(string Module, string Symbol, ResolutionSource Source, string Detail, nint Address, bool Delay);
