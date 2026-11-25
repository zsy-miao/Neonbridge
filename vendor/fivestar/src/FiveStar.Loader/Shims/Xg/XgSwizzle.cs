// Copyright (c) 2026 Five Star Recomp contributors.
// SPDX-License-Identifier: GPL-3.0-or-later

using System.Numerics;
using System.Runtime.Intrinsics.X86;

namespace FiveStar.Loader.Shims.Xg;

/// <summary>
/// A block swizzle as two bit masks over the element index inside a block: index bit k is the next x bit of the
/// element's position when bit k of <see cref="XMask"/> is set, the next y bit otherwise. Blocks are laid out
/// row-major. A pattern with a x bits and b y bits covers a 2^a by 2^b element block. With
/// <see cref="LowYSwapped"/> the first two y bits of the mask are y bit 1, then y bit 0. A thick (3D) block also has
/// z bits (<see cref="ZMask"/>): a brick of 2^a by 2^b by 2^c elements.
/// </summary>
internal readonly struct XgSwizzle
{
    private const uint LowYBits = 3;

    public XgSwizzle(uint xMask, int bits, bool lowYSwapped = false, uint zMask = 0)
    {
        XMask = xMask;
        ZMask = zMask;
        YMask = ((1u << bits) - 1) & ~xMask & ~zMask;
        Bits = bits;
        LowYSwapped = lowYSwapped;

        TileWidth = 1 << BitOperations.PopCount(XMask);
        TileHeight = 1 << BitOperations.PopCount(YMask);
        TileDepth = 1 << BitOperations.PopCount(ZMask);
    }

    public bool LowYSwapped { get; }

    public uint XMask { get; }

    public uint YMask { get; }

    /// <summary>Gets the z bits of a thick block; 0 for a 2D block.</summary>
    public uint ZMask { get; }

    /// <summary>Gets the block depth in elements (1 for a 2D block).</summary>
    public int TileDepth { get; }

    /// <summary>Gets the number of index bits: log2 of the elements in one block.</summary>
    public int Bits { get; }

    /// <summary>Gets the block width in elements.</summary>
    public int TileWidth { get; }

    /// <summary>Gets the block height in elements.</summary>
    public int TileHeight { get; }

    /// <summary>The element index inside a block of the element at (x, y) of that block.</summary>
    [HotPath]
    public uint Index(uint x, uint y)
    {
        y = SwapLowY(y);

        if (Bmi2.IsSupported)
        {
            return Bmi2.ParallelBitDeposit(x, XMask) | Bmi2.ParallelBitDeposit(y, YMask);
        }

        return Deposit(x, XMask) | Deposit(y, YMask);
    }

    /// <summary>The element index inside a thick block of the element at (x, y, z) of that block.</summary>
    [HotPath]
    public uint Index(uint x, uint y, uint z)
    {
        if (Bmi2.IsSupported)
        {
            return Index(x, y) | Bmi2.ParallelBitDeposit(z, ZMask);
        }

        return Index(x, y) | Deposit(z, ZMask);
    }

    /// <summary>The x position of the element at an index inside its block.</summary>
    [HotPath]
    public uint X(uint index)
    {
        return Extract(index, XMask);
    }

    /// <summary>The y position of the element at an index inside its block.</summary>
    [HotPath]
    public uint Y(uint index)
    {
        return SwapLowY(Extract(index, YMask));
    }

    /// <summary>Exchanges y bits 0 and 1 when the block orders them the other way round.</summary>
    [HotPath]
    private uint SwapLowY(uint y)
    {
        if (!LowYSwapped)
        {
            return y;
        }

        return (y & ~LowYBits) | ((y & 1) << 1) | ((y >> 1) & 1);
    }

    private static uint Extract(uint value, uint mask)
    {
        if (Bmi2.IsSupported)
        {
            return Bmi2.ParallelBitExtract(value, mask);
        }

        uint result = 0;
        int bit = 0;

        for (uint rest = mask; rest != 0; rest &= rest - 1)
        {
            uint lowest = rest & (~rest + 1);

            if ((value & lowest) != 0)
            {
                result |= 1u << bit;
            }

            bit++;
        }

        return result;
    }

    private static uint Deposit(uint value, uint mask)
    {
        uint result = 0;
        int bit = 0;

        for (uint rest = mask; rest != 0; rest &= rest - 1)
        {
            uint lowest = rest & (~rest + 1);

            if ((value & (1u << bit)) != 0)
            {
                result |= lowest;
            }

            bit++;
        }

        return result;
    }
}
