// Copyright (c) 2026 Five Star Recomp contributors.
// SPDX-License-Identifier: GPL-3.0-or-later

namespace FiveStar.Loader.Pe;

/// <summary>One section header of a PE image.</summary>
internal sealed record PeSection(string Name, uint VirtualAddress, uint VirtualSize, uint RawPointer, uint RawSize, SectionCharacteristics Characteristics);
