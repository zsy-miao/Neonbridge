// Copyright (c) 2026 Five Star Recomp contributors.
// SPDX-License-Identifier: GPL-3.0-or-later

using System.Buffers.Binary;
using System.Globalization;
using FiveStar.Formats.Common;
using FiveStar.Formats.Compression;

namespace FiveStar.Formats.Ttpf;

/// <summary>Raw page-table access and per-page storage census for texture page files, without decoding textures.</summary>
public sealed class PageTableSurvey
{
    private const int HeaderSize = 0x18;
    private const int CountField = 4;
    private const int EntrySize = 8;
    private const byte ZlibFirstByte = 0x78;
    private const int CodecFlagMask = 0x1C;
    private const int CodecShift = 2;

    private readonly IByteSource _source;
    private readonly ZlibInflater _inflater;

    public PageTableSurvey(IByteSource source, ZlibInflater inflater)
    {
        _source = source;
        _inflater = inflater;
    }

    /// <summary>The real page-table entries (the header count minus the trailing sentinel slot).</summary>
    public IReadOnlyList<PageTableEntry> Entries()
    {
        var count = (int)BinaryPrimitives.ReadUInt32LittleEndian(_source.Read(CountField, sizeof(uint))) - 1;
        var table = _source.Read(HeaderSize, EntrySize * Math.Max(count, 0));
        var entries = new List<PageTableEntry>(Math.Max(count, 0));

        for (var index = 0; index < count; index++)
        {
            var low = BinaryPrimitives.ReadUInt32LittleEndian(table[(index * EntrySize)..]);
            var high = BinaryPrimitives.ReadUInt32LittleEndian(table[((index * EntrySize) + sizeof(uint))..]);
            entries.Add(new PageTableEntry(low, high));
        }

        return entries;
    }

    /// <summary>
    /// Pages and stored bytes per storage kind: <c>zlib</c>, <c>stored</c> or <c>codecN</c> (BCPack). A page with no
    /// codec bits counts as zlib when its first byte is 0x78.
    /// </summary>
    public (Tally<string> Pages, Tally<string> Bytes) StorageCensus(TexturePageFile file)
    {
        var pages = new Tally<string>();
        var bytes = new Tally<string>();

        foreach (var page in file.Pages)
        {
            var codec = page.Flag & CodecFlagMask;
            string kind;

            if (codec == 0)
            {
                var first = _source.Read(page.FileOffset, 1);
                kind = first.Length > 0 && first[0] == ZlibFirstByte ? "zlib" : "stored";
            }
            else
            {
                kind = "codec" + (codec >> CodecShift).ToString(CultureInfo.InvariantCulture);
            }

            pages.Add(kind);
            bytes.Add(kind, page.FileLength);
        }

        return (pages, bytes);
    }

    /// <summary>
    /// What the stored bytes of a page are: <c>empty</c>, <c>zlib out=0x..</c> or <c>rawdeflate out=0x..</c> (data
    /// that inflates without error, possibly cut short), or <c>other</c>.
    /// </summary>
    public string Probe(ReadOnlySpan<byte> stored)
    {
        if (stored.IsEmpty)
        {
            return "empty";
        }

        var zlib = _inflater.Inflate(stored, DeflateWrapper.Zlib, int.MaxValue);

        if (zlib.Status != InflateStatus.Invalid)
        {
            return "zlib out=" + DecimalText.Hex(_inflater.Output.Length);
        }

        var raw = _inflater.Inflate(stored, DeflateWrapper.Raw, int.MaxValue);

        if (raw.Status != InflateStatus.Invalid)
        {
            return "rawdeflate out=" + DecimalText.Hex(_inflater.Output.Length);
        }

        return "other";
    }
}
