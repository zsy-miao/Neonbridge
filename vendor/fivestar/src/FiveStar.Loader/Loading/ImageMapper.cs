// Copyright (c) 2026 Five Star Recomp contributors.
// SPDX-License-Identifier: GPL-3.0-or-later

using System.Runtime.InteropServices;
using FiveStar.Loader.Diagnostics;
using FiveStar.Loader.Native;
using FiveStar.Loader.Pe;

namespace FiveStar.Loader.Loading;

/// <summary>Reserves an image's memory (at its preferred base when free) and copies the headers and sections into it.</summary>
internal static class ImageMapper
{
    public static LoadedImage Map(PeImage pe, string name, string path, LoaderLog log)
    {
        nint imageBase = Allocate(pe, name);
        var data = pe.Data;
        Marshal.Copy(data, 0, imageBase, (int)Math.Min(pe.SizeOfHeaders, (uint)data.Length));

        foreach (var section in pe.Sections)
        {
            CopySection(data, section, imageBase);
        }

        var image = new LoadedImage(name, path, pe, imageBase);
        log.Info($"  mapped {image}  entry=+0x{pe.EntryPointRva:x}  {pe.Sections.Count} sections");

        return image;
    }

    private static nint Allocate(PeImage pe, string name)
    {
        nuint size = pe.SizeOfImage;
        nint imageBase = Kernel32.VirtualAlloc((nint)pe.ImageBase, size, Win32Constants.MemReserveCommit, Win32Constants.PageReadWrite);

        if (imageBase != 0)
        {
            return imageBase;
        }

        if (pe.RelocationsStripped || !pe.Directory(DataDirectoryIndex.BaseRelocation).IsPresent)
        {
            throw new BadImageFormatException($"{name}: preferred base 0x{pe.ImageBase:x} unavailable and image has no relocations");
        }

        imageBase = Kernel32.VirtualAlloc(0, size, Win32Constants.MemReserveCommit, Win32Constants.PageReadWrite);

        if (imageBase == 0)
        {
            throw new InvalidOperationException($"{name}: VirtualAlloc(0x{size:x}) failed");
        }

        return imageBase;
    }

    private static void CopySection(byte[] data, PeSection section, nint imageBase)
    {
        uint length = section.RawSize;

        if (section.VirtualSize != 0)
        {
            length = Math.Min(section.RawSize, section.VirtualSize);
        }

        if (length == 0 || section.RawPointer == 0)
        {
            return;
        }

        if (section.RawPointer + length > data.Length)
        {
            length = (uint)Math.Max(0, data.Length - section.RawPointer);
        }

        Marshal.Copy(data, (int)section.RawPointer, imageBase + (nint)section.VirtualAddress, (int)length);
    }
}
