// Copyright (c) 2026 Five Star Recomp contributors.
// SPDX-License-Identifier: GPL-3.0-or-later

using System.Buffers.Binary;

namespace FiveStar.Loader.Pe;

/// <summary>Writes a .reloc section of IMAGE_REL_BASED_DIR64 entries, one block per 4 KiB page.</summary>
internal sealed class BaseRelocationBuilder
{
    private readonly SortedSet<uint> _rvas = [];

    public void AddDir64(uint rva)
    {
        _rvas.Add(rva);
    }

    public byte[] Build()
    {
        var bytes = new List<byte>();

        foreach (var page in _rvas.GroupBy(rva => rva & RelocationFormat.PageMask))
        {
            AppendBlock(bytes, page.Key, page.ToList());
        }

        return bytes.ToArray();
    }

    private static void AppendBlock(List<byte> bytes, uint page, List<uint> rvas)
    {
        int entries = rvas.Count + (rvas.Count % (RelocationFormat.BlockAlignment / RelocationFormat.EntrySize));
        var block = new byte[RelocationFormat.BlockHeaderSize + (entries * RelocationFormat.EntrySize)];
        BinaryPrimitives.WriteUInt32LittleEndian(block, page);
        BinaryPrimitives.WriteUInt32LittleEndian(block.AsSpan(RelocationFormat.BlockSizeOffset), (uint)block.Length);

        for (int i = 0; i < rvas.Count; i++)
        {
            int offset = (int)(rvas[i] - page);
            ushort entry = (ushort)(((int)BaseRelocationType.Dir64 << RelocationFormat.TypeShift) | offset);
            BinaryPrimitives.WriteUInt16LittleEndian(block.AsSpan(RelocationFormat.BlockHeaderSize + (i * RelocationFormat.EntrySize)), entry);
        }

        bytes.AddRange(block);
    }
}
