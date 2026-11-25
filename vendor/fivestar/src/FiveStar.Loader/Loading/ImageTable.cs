// Copyright (c) 2026 Five Star Recomp contributors.
// SPDX-License-Identifier: GPL-3.0-or-later

using FiveStar.Loader.Native;

namespace FiveStar.Loader.Loading;

/// <summary>
/// Every image this loader has mapped, in mapping order. Read lock-free by shims; replaced on change. The same images'
/// address ranges are also kept in native memory (<see cref="Ranges"/>), for the fault reporter's native entry.
/// </summary>
internal sealed unsafe class ImageTable
{
    /// <summary>Images the native range table holds; later ones are left out of it.</summary>
    public const int MaxRanges = 256;

    /// <summary>Entries per image in the range table (base, end) and the count's one before them.</summary>
    private const int RangeEntries = 2;
    private const int CountEntries = 1;

    private readonly object _gate = new();
    private readonly ulong* _ranges = (ulong*)System.Runtime.InteropServices.NativeMemory.AllocZeroed((nuint)((CountEntries + (RangeEntries * MaxRanges)) * sizeof(ulong)));
    private LoadedImage[] _images = [];

    public IReadOnlyList<LoadedImage> All => Volatile.Read(ref _images);

    /// <summary>The native range table: the count, then each image's base and end. Never freed.</summary>
    public nint Ranges => (nint)_ranges;

    public void Add(LoadedImage image)
    {
        lock (_gate)
        {
            _images = [.. _images, image];
            WriteRanges();
        }
    }

    public void Remove(LoadedImage image)
    {
        lock (_gate)
        {
            _images = _images.Where(i => i != image).ToArray();
            WriteRanges();
        }
    }

    /// <summary>Rewrites the range table: the count goes to 0 first, so a reader meanwhile sees fewer ranges, not wrong ones.</summary>
    private void WriteRanges()
    {
        int count = Math.Min(_images.Length, MaxRanges);
        Volatile.Write(ref _ranges[0], 0);

        for (int i = 0; i < count; i++)
        {
            ulong* range = _ranges + CountEntries + (RangeEntries * i);
            range[0] = (ulong)_images[i].Base;
            range[1] = (ulong)_images[i].Base + (ulong)_images[i].Size;
        }

        Volatile.Write(ref _ranges[0], (ulong)count);
    }

    public LoadedImage? FindByBase(nint imageBase)
    {
        foreach (var image in All)
        {
            if (image.Base == imageBase)
            {
                return image;
            }
        }

        return null;
    }

    public LoadedImage? FindByName(string name)
    {
        foreach (var image in All)
        {
            if (string.Equals(image.Name, name, StringComparison.OrdinalIgnoreCase))
            {
                return image;
            }
        }

        return null;
    }

    public LoadedImage? FindByAddress(nint address)
    {
        foreach (var image in All)
        {
            if (image.Contains(address))
            {
                return image;
            }
        }

        return null;
    }

    /// <summary>"image.dll+0xoffset" for a mapped image, else the OS module the address belongs to.</summary>
    public string Describe(nint address)
    {
        var image = FindByAddress(address);

        if (image is null)
        {
            return SystemModules.Describe(address);
        }

        return $"{image.Name}+0x{(long)address - (long)image.Base:x}";
    }
}
