// Copyright (c) 2026 Five Star Recomp contributors.
// SPDX-License-Identifier: GPL-3.0-or-later

using System.Runtime.InteropServices;

namespace FiveStar.Loader.Shims.D3D12X.Present;

/// <summary>
/// One plane of queue slot 32 (PresentX, *inferred*), 0x48 bytes, as the game fills it at 0x142482313. The bytes
/// past <see cref="ColorSpace"/> are zero in every plane the game builds.
/// </summary>
[StructLayout(LayoutKind.Explicit, Size = Size)]
internal unsafe struct PresentPlane
{
    /// <summary>The stride of the plane array.</summary>
    public const int Size = 0x48;

    /// <summary>The frame token from device slot 80 (the same value queue slot 35 gets as its third argument).</summary>
    [FieldOffset(0x00)]
    public ulong Token;

    /// <summary>Always 1: the number of resources behind <see cref="Resources"/> (*inferred*).</summary>
    [FieldOffset(0x08)]
    public uint ResourceCount;

    /// <summary>ID3D12Resource* const*: the plane's image, a wrapped resource.</summary>
    [FieldOffset(0x10)]
    public nint* Resources;

    /// <summary>RECT*: the part of the resource to show, {0, 0, width, height}.</summary>
    [FieldOffset(0x18)]
    public PlaneRect* SourceRect;

    /// <summary>float[4]*: where it goes on the screen, normalized; {0, 0, 1, 1} (constant at 0x146159cd0).</summary>
    [FieldOffset(0x20)]
    public float* DestinationRect;

    /// <summary>A dword from the game's present record (+8 of each plane entry), read as the color space (*inferred*).</summary>
    [FieldOffset(0x28)]
    public uint ColorSpace;
}
