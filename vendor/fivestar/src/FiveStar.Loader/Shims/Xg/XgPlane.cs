// Copyright (c) 2026 Five Star Recomp contributors.
// SPDX-License-Identifier: GPL-3.0-or-later

using System.Runtime.InteropServices;

namespace FiveStar.Loader.Shims.Xg;

/// <summary>
/// The layout of one plane: array slices of <see cref="SliceStride"/> bytes from <see cref="Offset"/>, each holding
/// the mip tail block (if any) first and then the larger mips, smallest first, so mip 0 ends the slice.
/// </summary>
[StructLayout(LayoutKind.Sequential)]
internal struct XgPlane
{
    public int BytesPerElement;
    public int BlockDimension;

    /// <summary>Bytes of one swizzle block, or 0 for a linear (row-major) plane.</summary>
    public int BlockBytes;

    public XgSwizzle Swizzle;
    public int MipCount;

    /// <summary>The first mip in the tail block, or <see cref="MipCount"/> when there is no tail.</summary>
    public int FirstTailMip;

    /// <summary>
    /// The depth (3D) or array size. A thick plane (a 3D texture in a Z or S mode) holds one mip chain of bricks
    /// instead: <see cref="SliceStride"/> and <see cref="Size"/> are the whole volume, and a slice is a depth.
    /// </summary>
    public int Slices;

    public bool Thick;
    public ulong Offset;
    public ulong SliceStride;
    public ulong Size;
    public XgMipArray Mips;
}
