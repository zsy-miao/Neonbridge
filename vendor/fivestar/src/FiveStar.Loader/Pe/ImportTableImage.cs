// Copyright (c) 2026 Five Star Recomp contributors.
// SPDX-License-Identifier: GPL-3.0-or-later

namespace FiveStar.Loader.Pe;

/// <summary>
/// An import table written by <see cref="ImportTableBuilder"/>: its bytes, where the descriptor array starts and how
/// long it is, and the RVA of the IAT slot of each "module!symbol".
/// </summary>
internal sealed record ImportTableImage(byte[] Bytes, uint DescriptorRva, uint DescriptorSize, IReadOnlyDictionary<string, uint> IatSlots);
