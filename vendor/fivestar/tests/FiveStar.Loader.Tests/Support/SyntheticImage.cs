// Copyright (c) 2026 Five Star Recomp contributors.
// SPDX-License-Identifier: GPL-3.0-or-later

using System.Buffers.Binary;
using FiveStar.Loader.Pe;

namespace FiveStar.Loader.Tests.Support;

/// <summary>
/// Builds a small x64 DLL in memory: .text at 0x1000, .data at 0x2000, .rdata at 0x3000 (imports, then exports at
/// +0x800) and .reloc at 0x4000. The entry point is 0, so the loader calls no DllMain.
/// </summary>
internal sealed class SyntheticImage
{
    public const uint TextRva = 0x1000;
    public const uint DataRva = 0x2000;
    public const uint ReadOnlyRva = 0x3000;
    public const uint ExportsRva = 0x3800;
    public const uint RelocRva = 0x4000;

    private const SectionCharacteristics CodeFlags = SectionCharacteristics.ContainsCode | SectionCharacteristics.MemoryExecute | SectionCharacteristics.MemoryRead;
    private const SectionCharacteristics ReadWrite = SectionCharacteristics.ContainsInitializedData | SectionCharacteristics.MemoryRead | SectionCharacteristics.MemoryWrite;
    private const SectionCharacteristics ReadOnly = SectionCharacteristics.ContainsInitializedData | SectionCharacteristics.MemoryRead;
    private const SectionCharacteristics Discardable = ReadOnly | SectionCharacteristics.MemoryDiscardable;

    private readonly string _name;
    private readonly ulong _imageBase;
    private readonly ImportTableBuilder _imports = new();
    private readonly BaseRelocationBuilder _relocations = new();
    private readonly List<(string Name, uint Rva)> _exports = [];
    private bool _hasImports;
    private DataDirectory _tls;

    public SyntheticImage(string name, ulong imageBase)
    {
        _name = name;
        _imageBase = imageBase;
    }

    public byte[] Text { get; } = new byte[0x200];

    public byte[] Data { get; } = new byte[0x200];

    public ulong ImageBase => _imageBase;

    public void Export(string name, uint rva)
    {
        _exports.Add((name, rva));
    }

    public void Import(string module, string symbol)
    {
        _imports.Add(module, symbol);
        _hasImports = true;
    }

    /// <summary>Writes the absolute address of <paramref name="targetRva"/> into .data at <paramref name="dataOffset"/>, with a DIR64 fixup.</summary>
    public void WritePointer(int dataOffset, uint targetRva)
    {
        BinaryPrimitives.WriteUInt64LittleEndian(Data.AsSpan(dataOffset), _imageBase + targetRva);
        _relocations.AddDir64(DataRva + (uint)dataOffset);
    }

    public void SetTlsDirectory(uint rva)
    {
        _tls = new DataDirectory(rva, TlsFormat.DirectorySize);
    }

    /// <summary>Builds the image and writes it to <paramref name="folder"/>; returns the path and the IAT slots.</summary>
    public (string Path, IReadOnlyDictionary<string, uint> IatSlots) WriteTo(string folder)
    {
        var importTable = _imports.Build(ReadOnlyRva);
        var readOnly = new byte[0x1000];
        importTable.Bytes.CopyTo(readOnly, 0);
        var exportTable = ExportTableWriter.Write(_name, _exports, ExportsRva);
        exportTable.CopyTo(readOnly, ExportsRva - ReadOnlyRva);

        // The .reloc section always has a block, so the image can move.
        _relocations.AddDir64(DataRva + (uint)Data.Length - sizeof(ulong));
        var builder = new PeImageBuilder(_imageBase, PeFormat.FileExecutableImage | PeFormat.FileLargeAddressAware | PeFormat.FileDll);
        builder.AddSection(new PeSectionContent(".text", TextRva, Text, CodeFlags));
        builder.AddSection(new PeSectionContent(".data", DataRva, Data, ReadWrite));
        builder.AddSection(new PeSectionContent(".rdata", ReadOnlyRva, readOnly, ReadOnly));

        var reloc = _relocations.Build();
        builder.AddSection(new PeSectionContent(".reloc", RelocRva, reloc, Discardable));

        if (_hasImports)
        {
            builder.SetDirectory(DataDirectoryIndex.Import, importTable.DescriptorRva, importTable.DescriptorSize);
        }

        if (_exports.Count > 0)
        {
            builder.SetDirectory(DataDirectoryIndex.Export, ExportsRva, (uint)exportTable.Length);
        }

        builder.SetDirectory(DataDirectoryIndex.BaseRelocation, RelocRva, (uint)reloc.Length);
        builder.SetDirectory(DataDirectoryIndex.Tls, _tls.Rva, _tls.Size);
        string path = Path.Combine(folder, _name);
        File.WriteAllBytes(path, builder.Build());

        return (path, importTable.IatSlots);
    }

    /// <summary>Copies machine code into .text at <paramref name="offset"/>.</summary>
    public void Code(int offset, params byte[] code)
    {
        code.CopyTo(Text, offset);
    }
}
