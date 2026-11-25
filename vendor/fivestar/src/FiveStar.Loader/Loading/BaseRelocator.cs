// Copyright (c) 2026 Five Star Recomp contributors.
// SPDX-License-Identifier: GPL-3.0-or-later

using FiveStar.Loader.Diagnostics;
using FiveStar.Loader.Pe;

namespace FiveStar.Loader.Loading;

/// <summary>Applies the .reloc fixups (DIR64, HIGHLOW, HIGH, LOW) of an image mapped away from its preferred base.</summary>
internal static unsafe class BaseRelocator
{
    public static void Apply(LoadedImage image, LoaderLog log)
    {
        long delta = (long)image.Base - (long)image.Pe.ImageBase;

        if (delta == 0)
        {
            return;
        }

        var directory = image.Pe.Directory(DataDirectoryIndex.BaseRelocation);
        byte* block = (byte*)image.Va(directory.Rva);
        byte* end = block + directory.Size;
        int count = 0;

        while (block + RelocationFormat.BlockHeaderSize <= end)
        {
            uint page = *(uint*)block;
            uint blockSize = *(uint*)(block + RelocationFormat.BlockSizeOffset);

            if (blockSize < RelocationFormat.BlockHeaderSize)
            {
                break;
            }

            count += ApplyBlock(image, page, (ushort*)(block + RelocationFormat.BlockHeaderSize), (blockSize - RelocationFormat.BlockHeaderSize) / RelocationFormat.EntrySize, delta);
            block += blockSize;
        }

        string sign = "+";

        if (delta < 0)
        {
            sign = "-";
        }

        log.Info($"  relocated by {sign}0x{Math.Abs(delta):x} ({count} fixups)");
    }

    private static int ApplyBlock(LoadedImage image, uint page, ushort* entries, uint entryCount, long delta)
    {
        int count = 0;

        for (int i = 0; i < entryCount; i++)
        {
            var type = (BaseRelocationType)(entries[i] >> RelocationFormat.TypeShift);
            byte* target = (byte*)image.Va(page + (uint)(entries[i] & RelocationFormat.OffsetMask));

            if (ApplyEntry(image, type, target, delta))
            {
                count++;
            }
        }

        return count;
    }

    /// <summary>Applies one fixup; returns false for the ABSOLUTE padding entry.</summary>
    private static bool ApplyEntry(LoadedImage image, BaseRelocationType type, byte* target, long delta)
    {
        switch (type)
        {
            case BaseRelocationType.Absolute:
                return false;
            case BaseRelocationType.Dir64:
                return ApplyDir64(target, delta);
            case BaseRelocationType.HighLow:
                return ApplyHighLow(target, delta);
            case BaseRelocationType.High:
                return ApplyHigh(target, delta);
            case BaseRelocationType.Low:
                return ApplyLow(target, delta);
            default:
                throw new BadImageFormatException($"{image.Name}: unsupported relocation type {(int)type}");
        }
    }

    private static bool ApplyDir64(byte* target, long delta)
    {
        *(long*)target += delta;

        return true;
    }

    private static bool ApplyHighLow(byte* target, long delta)
    {
        *(int*)target += (int)delta;

        return true;
    }

    private static bool ApplyHigh(byte* target, long delta)
    {
        *(ushort*)target += (ushort)((delta >> RelocationFormat.HighWordShift) & RelocationFormat.WordMask);

        return true;
    }

    private static bool ApplyLow(byte* target, long delta)
    {
        *(ushort*)target += (ushort)(delta & RelocationFormat.WordMask);

        return true;
    }
}
