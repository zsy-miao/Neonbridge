// Copyright (c) 2026 Five Star Recomp contributors.
// SPDX-License-Identifier: GPL-3.0-or-later

namespace FiveStar.Formats.Models;

/// <summary>
/// One attribute stream of a vertex buffer: its slot in the stream table, its format byte and its byte offset from the
/// start of the buffer. The format's high nibble is the element size in 2-byte units.
/// </summary>
public readonly record struct VertexChannel(int Slot, byte Format, int Offset)
{
    private const int SizeShift = 4;
    private const int SizeUnit = 2;

    public int ElementSize => (Format >> SizeShift) * SizeUnit;
}
