// Copyright (c) 2026 Five Star Recomp contributors.
// SPDX-License-Identifier: GPL-3.0-or-later

namespace FiveStar.Formats.Platform;

/// <summary>A section header of a PE image (40 bytes: name, virtual size and address, raw size and offset, ...).</summary>
/// <param name="Name">The 8-byte name without trailing NULs.</param>
/// <param name="VirtualSize">Size in memory.</param>
/// <param name="VirtualAddress">RVA of the section.</param>
/// <param name="RawSize">Size in the file.</param>
/// <param name="RawOffset">File offset of the section data.</param>
public sealed record PeSection(string Name, uint VirtualSize, uint VirtualAddress, uint RawSize, uint RawOffset);
