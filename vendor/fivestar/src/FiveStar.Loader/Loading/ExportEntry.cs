// Copyright (c) 2026 Five Star Recomp contributors.
// SPDX-License-Identifier: GPL-3.0-or-later

namespace FiveStar.Loader.Loading;

/// <summary>One export of a mapped image; <see cref="Forwarder"/> is "DLL.Func" or "DLL.#N" for a forwarded export.</summary>
internal readonly record struct ExportEntry(uint Ordinal, string? Name, uint Rva, string? Forwarder);
