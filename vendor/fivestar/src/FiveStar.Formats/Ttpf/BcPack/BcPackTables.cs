// Copyright (c) 2026 Five Star Recomp contributors.
// SPDX-License-Identifier: GPL-3.0-or-later

// The file layout follows VIctory (crates/victory-formats/src/bcpack.rs, GPL-3.0); see THIRD_PARTY_NOTICES.md.

using System.Buffers.Binary;
using System.Runtime.CompilerServices;
using System.Runtime.InteropServices;
using System.Security.Cryptography;

namespace FiveStar.Formats.Ttpf.BcPack;

/// <summary>
/// The constant tables of the BCPack block layer, read from a file the user supplies (<c>bcpack-tables.bin</c>, 77,982
/// bytes); this repository does not ship them. The file holds 128 x 9 u16 model frequencies (the same as our rANS
/// models), the index rank tables (BC1 256 bytes, BC3 alpha 64 and colour 256, BC4 64, BC5 64), 512 u16 BC7 control
/// words, then 255 dictionary entries per codec, little-endian without padding. Only the known file is accepted: size
/// and SHA-256 must match. The arrays are pinned, so decoders keep pointers into them. Immutable and thread-safe.
/// </summary>
public sealed class BcPackTables
{
    /// <summary>The file's size in bytes.</summary>
    public const int FileSize = 77_982;

    /// <summary>The file's SHA-256, upper-case hex.</summary>
    public const string Sha256 = "1F1D4760419202BB5672F0C428CE51E456B5D08FBAEC8F371D043FD94E01C36B";

    /// <summary>The usual file name.</summary>
    public const string FileName = "bcpack-tables.bin";

    /// <summary>Entries of each codec's dictionary: back references to blocks -1 to -255.</summary>
    public const int DictionaryEntries = 255;

    private const int ModelBytes = 128 * 9 * sizeof(ushort);
    private const int QuadRankEntries = 256;
    private const int PairRankEntries = 64;
    private const int ControlWords = 512;
    private const int Bc1Endpoints = 6;
    private const int Bc3Endpoints = 8;
    private const int Bc4Endpoints = 2;
    private const int Bc5Endpoints = 4;
    private const int SmallIndices = 16;
    private const int LargeIndices = 32;

    private BcPackTables(ReadOnlySpan<byte> file)
    {
        var at = ModelBytes;
        QuadRank1 = Take(file, ref at, QuadRankEntries);
        PairRank3 = Take(file, ref at, PairRankEntries);
        QuadRank3 = Take(file, ref at, QuadRankEntries);
        PairRank4 = Take(file, ref at, PairRankEntries);
        PairRank5 = Take(file, ref at, PairRankEntries);

        Controls = GC.AllocateArray<ushort>(ControlWords, pinned: true);

        for (var word = 0; word < ControlWords; word++)
        {
            Controls[word] = ReadU16(file, ref at);
        }

        Bc1 = ReadPlain(file, ref at, Bc1Endpoints, SmallIndices);
        Bc3 = ReadPlain(file, ref at, Bc3Endpoints, LargeIndices);
        Bc4 = ReadPlain(file, ref at, Bc4Endpoints, SmallIndices);
        Bc5 = ReadPlain(file, ref at, Bc5Endpoints, LargeIndices);
        Bc6H = ReadBc6H(file, ref at);
        Bc7 = ReadBc7(file, ref at);
    }

    /// <summary>Gets the BC1 2x2 index rank table: symbol to four 2-bit residuals (pixels TL, TR, BL, BR).</summary>
    internal byte[] QuadRank1 { get; }

    /// <summary>Gets the BC3 alpha pair rank table: symbol to two 3-bit residuals (bits 0-2 and 4-6).</summary>
    internal byte[] PairRank3 { get; }

    /// <summary>Gets the BC3 colour 2x2 index rank table.</summary>
    internal byte[] QuadRank3 { get; }

    internal byte[] PairRank4 { get; }

    internal byte[] PairRank5 { get; }

    /// <summary>Gets the BC7 control word of each lane 15 symbol pair: mode, rotation, index selection, subsets, partition.</summary>
    internal ushort[] Controls { get; }

    internal PlainEntry[] Bc1 { get; }

    internal PlainEntry[] Bc3 { get; }

    internal PlainEntry[] Bc4 { get; }

    internal PlainEntry[] Bc5 { get; }

    internal Bc6HEntry[] Bc6H { get; }

    internal Bc7Entry[] Bc7 { get; }

    /// <summary>Reads the tables from <paramref name="path"/>.</summary>
    /// <exception cref="InvalidDataException">The file is not the known table file.</exception>
    public static BcPackTables Load(string path) => Parse(File.ReadAllBytes(path));

    /// <summary>Reads the tables from the file's bytes.</summary>
    /// <exception cref="InvalidDataException">The bytes are not the known table file.</exception>
    public static BcPackTables Parse(ReadOnlySpan<byte> file)
    {
        var problem = Check(file);

        if (problem is not null)
        {
            throw new InvalidDataException(problem);
        }

        return new BcPackTables(file);
    }

    /// <summary>Why <paramref name="file"/> is not the known table file, or null when it is.</summary>
    public static string? Check(ReadOnlySpan<byte> file)
    {
        if (file.Length != FileSize)
        {
            return $"a BCPack table file has {FileSize} bytes, not {file.Length}";
        }

        if (Convert.ToHexString(SHA256.HashData(file)) != Sha256)
        {
            return $"the BCPack table file has the right size but not the known content (SHA-256 {Sha256})";
        }

        return null;
    }

    /// <summary>The address of a pinned table array.</summary>
    internal static unsafe T* Address<T>(T[] pinned)
        where T : unmanaged => (T*)Unsafe.AsPointer(ref MemoryMarshal.GetArrayDataReference(pinned));

    private static byte[] Take(ReadOnlySpan<byte> file, ref int at, int count)
    {
        var bytes = GC.AllocateArray<byte>(count, pinned: true);
        file.Slice(at, count).CopyTo(bytes);
        at += count;

        return bytes;
    }

    private static ushort ReadU16(ReadOnlySpan<byte> file, ref int at)
    {
        var value = BinaryPrimitives.ReadUInt16LittleEndian(file[at..]);
        at += sizeof(ushort);

        return value;
    }

    private static unsafe PlainEntry[] ReadPlain(ReadOnlySpan<byte> file, ref int at, int endpoints, int indices)
    {
        var entries = GC.AllocateArray<PlainEntry>(DictionaryEntries, pinned: true);

        for (var entry = 0; entry < DictionaryEntries; entry++)
        {
            for (var k = 0; k < endpoints; k++)
            {
                entries[entry].Endpoints[k] = file[at++];
            }

            for (var k = 0; k < indices; k++)
            {
                entries[entry].Indices[k] = file[at++];
            }
        }

        return entries;
    }

    private static unsafe Bc6HEntry[] ReadBc6H(ReadOnlySpan<byte> file, ref int at)
    {
        var entries = GC.AllocateArray<Bc6HEntry>(DictionaryEntries, pinned: true);

        for (var entry = 0; entry < DictionaryEntries; entry++)
        {
            // The endpoint half's own mode byte plays no part in decoding.
            at++;

            for (var k = 0; k < Bc6HEntry.EndpointValues; k++)
            {
                entries[entry].Endpoints[k] = ReadU16(file, ref at);
            }

            entries[entry].Mode = file[at++];

            for (var k = 0; k < Bc6HEntry.IndexCount; k++)
            {
                entries[entry].Indices[k] = (short)ReadU16(file, ref at);
            }
        }

        return entries;
    }

    private static unsafe Bc7Entry[] ReadBc7(ReadOnlySpan<byte> file, ref int at)
    {
        var entries = GC.AllocateArray<Bc7Entry>(DictionaryEntries, pinned: true);

        for (var entry = 0; entry < DictionaryEntries; entry++)
        {
            entries[entry].Control = ReadU16(file, ref at);

            for (var k = 0; k < Bc7Entry.RowValues; k++)
            {
                entries[entry].Rows[k] = ReadU16(file, ref at);
            }

            for (var set = 0; set < Bc7Entry.IndexSets; set++)
            {
                entries[entry].Bits[set] = file[at++];
            }

            for (var k = 0; k < Bc7Entry.IndexValues; k++)
            {
                entries[entry].Indices[k] = (short)ReadU16(file, ref at);
            }
        }

        return entries;
    }
}
