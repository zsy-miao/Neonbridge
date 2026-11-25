// Copyright (c) 2026 Five Star Recomp contributors.
// SPDX-License-Identifier: GPL-3.0-or-later

namespace FiveStar.Loader.Shims.Xg;

/// <summary>
/// The texture layout field of the description. Values from 0x100 carry a swizzle mode in the low byte (the game writes
/// 0x100 + mode, 0x102 and 0x10a); the modes are numbered as AMD's ADDR_SW_* modes of the RDNA address library:
/// 0 linear, 1-3 256 B, 4-7 4 KB, 8-15 64 KB, 16-19 64 KB (_T), 20-23 4 KB (_X), 24-31 64 KB (_X). Values below 0x100
/// are D3D12_TEXTURE_LAYOUT: 0 unknown (suggest), 1 row major, 2 and 3 64 KB.
/// </summary>
internal static class XgSwizzleModes
{
    /// <summary>The first layout value that carries a swizzle mode.</summary>
    public const uint ModeBase = 0x100;

    public const uint Linear = 0;

    /// <summary>ADDR_SW_4KB_R_X: what XGSuggestSwizzleMode answers for smaller textures.</summary>
    public const uint Small = 23;

    /// <summary>ADDR_SW_64KB_R_X: what XGSuggestSwizzleMode answers for larger textures.</summary>
    public const uint Large = 27;

    private const uint ModeMask = 0xFF;
    private const uint LayoutUnknown = 0;
    private const uint LayoutRowMajor = 1;
    private const uint LastMicro = 3;
    private const uint LastSmall = 7;
    private const uint FirstSmallX = 20;
    private const uint LastSmallX = 23;

    /// <summary>The low two mode bits: 0 Z, 1 S, 2 D, 3 R.</summary>
    private const uint KindMask = 3;
    private const uint FirstThinKind = 2;

    /// <summary>
    /// Mip 0 sizes from which the shipped textures use 64 KB blocks: every texture of 1 MiB or more does, every one
    /// under 128 KiB uses 4 KB blocks (docs/xg.md).
    /// </summary>
    private const long LargeFromBytes = 0x100000;

    /// <summary>The swizzle mode a layout value names; an unknown layout gets the suggested mode.</summary>
    [HotPath]
    public static uint Mode(uint layout, long mip0Bytes)
    {
        if (layout >= ModeBase)
        {
            return layout & ModeMask;
        }

        if (layout == LayoutUnknown)
        {
            return Suggest(mip0Bytes);
        }

        if (layout == LayoutRowMajor)
        {
            return Linear;
        }

        return Large;
    }

    [HotPath]
    public static uint Suggest(long mip0Bytes)
    {
        if (mip0Bytes >= LargeFromBytes)
        {
            return Large;
        }

        return Small;
    }

    /// <summary>
    /// Whether a 3D texture in this mode is laid out in thick bricks (z inside the block): the Z and S modes of 4 KB
    /// and 64 KB blocks (mode mod 4 below 2), as in the RDNA address library; D and R modes keep 2D depth slices.
    /// </summary>
    [HotPath]
    public static bool IsThick(uint mode)
    {
        return BlockBytes(mode) >= XgSwizzles.SmallBlockBytes && (mode & KindMask) < FirstThinKind;
    }

    /// <summary>Bytes of one swizzle block of a mode, or 0 for linear.</summary>
    [HotPath]
    public static int BlockBytes(uint mode)
    {
        if (mode == Linear)
        {
            return 0;
        }

        if (mode <= LastMicro)
        {
            return XgSwizzles.MicroBlockBytes;
        }

        if (mode <= LastSmall || (mode >= FirstSmallX && mode <= LastSmallX))
        {
            return XgSwizzles.SmallBlockBytes;
        }

        return XgSwizzles.LargeBlockBytes;
    }
}
