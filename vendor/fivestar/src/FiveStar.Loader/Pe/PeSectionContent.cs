// Copyright (c) 2026 Five Star Recomp contributors.
// SPDX-License-Identifier: GPL-3.0-or-later

namespace FiveStar.Loader.Pe;

/// <summary>A section handed to <see cref="PeImageBuilder"/>: its name, RVA, bytes and flags.</summary>
internal sealed record PeSectionContent(string Name, uint Rva, byte[] Content, SectionCharacteristics Characteristics);
