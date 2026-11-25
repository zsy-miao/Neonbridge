// Copyright (c) 2026 Five Star Recomp contributors.
// SPDX-License-Identifier: GPL-3.0-or-later

namespace FiveStar.Formats.Audio;

/// <summary>An AWC (<c>ADAT</c>) audio container found inside an archive.</summary>
/// <param name="Offset">Archive offset of the <c>ADAT</c> magic.</param>
/// <param name="Size">The end of the furthest chunk.</param>
/// <param name="Flags">The u16 flags at +0x06.</param>
/// <param name="Streams">The streams in table order (sorted by id).</param>
public sealed record AwcContainer(long Offset, long Size, ushort Flags, IReadOnlyList<AwcStreamEntry> Streams);
