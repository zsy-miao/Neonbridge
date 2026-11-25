// Copyright (c) 2026 Five Star Recomp contributors.
// SPDX-License-Identifier: GPL-3.0-or-later

using FiveStar.Loader.Pe;

namespace FiveStar.Loader.Loading;

/// <summary>An image this loader mapped, with the state of its load.</summary>
internal sealed class LoadedImage
{
    public LoadedImage(string name, string path, PeImage pe, nint imageBase)
    {
        Name = name;
        Path = path;
        Pe = pe;
        Base = imageBase;
        Exports = new ExportTable(imageBase, default);
    }

    /// <summary>Lower-case file name, for example "bink2scarlett.dll".</summary>
    public string Name { get; }

    public string Path { get; }

    public PeImage Pe { get; }

    public nint Base { get; }

    public uint Size => Pe.SizeOfImage;

    public bool Relocated => (ulong)Base != Pe.ImageBase;

    public ExportTable Exports { get; set; }

    public List<ImportRecord> Imports { get; } = [];

    public nint FunctionTable { get; set; }

    public uint FunctionCount { get; set; }

    public nint TlsHost { get; set; }

    public string? TlsHostPath { get; set; }

    public uint TlsIndex { get; set; }

    public int TlsCallbackCount { get; set; }

    /// <summary>Offset of the saved slot-0 pointer in the TLS block of an executable (<see cref="TlsSlotZero"/>); -1 otherwise.</summary>
    public int TlsSlotZeroSaveOffset { get; set; } = -1;

    public bool EntryCalled { get; set; }

    public int EntryResult { get; set; }

    /// <summary>Requested by the user (--load), as opposed to a dependency or a runtime LoadLibrary.</summary>
    public bool TopLevel { get; set; }

    /// <summary>First mapped because mapped code called LoadLibrary, so FreeLibrary may unload it.</summary>
    public bool LoadedDynamically { get; set; }

    /// <summary>Outstanding LoadLibrary references from mapped code; static imports are not counted (they pin).</summary>
    public int DynamicRefs { get; set; }

    public bool Contains(nint address)
    {
        return (ulong)address >= (ulong)Base && (ulong)address < (ulong)Base + Size;
    }

    public nint Va(uint rva)
    {
        return Base + (nint)rva;
    }

    public override string ToString()
    {
        string relocated = string.Empty;

        if (Relocated)
        {
            relocated = ", relocated";
        }

        return $"{Name} @ 0x{Base:x} (size 0x{Size:x}{relocated})";
    }
}
