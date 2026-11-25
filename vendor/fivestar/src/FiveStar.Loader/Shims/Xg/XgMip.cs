// Copyright (c) 2026 Five Star Recomp contributors.
// SPDX-License-Identifier: GPL-3.0-or-later

using System.Runtime.InteropServices;

namespace FiveStar.Loader.Shims.Xg;

/// <summary>Where one mip of one array slice of a plane lives, relative to the start of the slice.</summary>
[StructLayout(LayoutKind.Sequential)]
internal struct XgMip
{
    /// <summary>Byte offset of the mip in its slice (of the tail block for a mip in the tail).</summary>
    public ulong Offset;

    /// <summary>Bytes the mip occupies (pitch x padded height, all depth slices).</summary>
    public ulong Size;

    /// <summary>Bytes of one depth slice of the mip.</summary>
    public ulong Slice2DSize;

    public uint WidthElements;
    public uint HeightElements;
    public uint DepthElements;
    public uint PitchElements;
    public uint PaddedHeightElements;

    /// <summary>For a mip in the tail: its first element's position inside the tail block.</summary>
    public uint TailX;
    public uint TailY;

    /// <summary>1 when the mip shares the tail block with the smaller mips.</summary>
    public uint InTail;
}
