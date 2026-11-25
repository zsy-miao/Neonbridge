// Copyright (c) 2026 Five Star Recomp contributors.
// SPDX-License-Identifier: GPL-3.0-or-later

namespace FiveStar.Formats.Ttpf;

/// <summary>A raw page-table entry of a texture page file (see <see cref="TexturePage"/> for the bit layout).</summary>
/// <param name="Low">The first u32: flag and file offset / 0x100.</param>
/// <param name="High">The second u32: tag and virtual address / 0x4000.</param>
public readonly record struct PageTableEntry(uint Low, uint High)
{
    private const int FlagShift = 24;
    private const uint OffsetMask = 0xFFFFFF;
    private const int OffsetShift = 8;
    private const uint VirtualPageMask = 0xFFFFF;
    private const int TagShift = 20;

    public byte Flag => (byte)(Low >> FlagShift);

    public long FileOffset => (long)(Low & OffsetMask) << OffsetShift;

    /// <summary>Gets the virtual address in 0x4000-byte units.</summary>
    public long VirtualPage => High & VirtualPageMask;

    public uint Tag => High >> TagShift;
}
