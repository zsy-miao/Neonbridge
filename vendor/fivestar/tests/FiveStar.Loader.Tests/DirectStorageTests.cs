// Copyright (c) 2026 Five Star Recomp contributors.
// SPDX-License-Identifier: GPL-3.0-or-later

using System.IO.Compression;
using FiveStar.Loader.Tests.Support;
using Xunit;

namespace FiveStar.Loader.Tests;

/// <summary>The CPU DirectStorage queue, driven through the factory and queue vtables the way the executable calls them.</summary>
public sealed unsafe class DirectStorageTests
{
    private const byte None = 0;
    private const byte Zlib = 1;
    private const int InvalidData = unchecked((int)0x8007000D);

    [Fact]
    public void A_submitted_batch_reads_a_range_inflates_a_zlib_chunk_and_reports_a_bad_chunk_in_its_status()
    {
        // A file with a pattern, a zlib stream at a 16-byte (not sector) aligned offset and a corrupt stream after it.
        byte[] plain = Pattern(150_000, 7);
        byte[] packed = Compress(plain);
        const int PackedAt = 0x1230;
        int corruptAt = PackedAt + packed.Length + 0x10;
        byte[] content = Pattern(corruptAt + packed.Length + 0x3000, 3);

        packed.CopyTo(content, PackedAt);
        packed.CopyTo(content, corruptAt);
        content[corruptAt + (packed.Length / 2)] ^= 0xFF;

        using var test = TestRuntime.Create();
        string path = Path.Combine(test.Folder, "package.bin");
        File.WriteAllBytes(path, content);

        var api = StorageApi.Create(test);
        nint file = api.OpenFile(path);
        nint statuses = api.CreateStatusArray(2);
        nint queue = api.CreateQueue();

        byte[] range = new byte[5000];
        byte[] inflated = new byte[plain.Length];
        byte[] corrupt = new byte[plain.Length];

        int first;
        int second;

        fixed (byte* rangeOut = range, inflatedOut = inflated, corruptOut = corrupt)
        {
            StorageApi.EnqueueRead(queue, file, 0x11, (uint)range.Length, rangeOut, (uint)range.Length, None);
            StorageApi.EnqueueRead(queue, file, PackedAt, (uint)packed.Length, inflatedOut, (uint)plain.Length, Zlib);
            StorageApi.EnqueueStatus(queue, statuses, 0);
            StorageApi.EnqueueRead(queue, file, (ulong)corruptAt, (uint)packed.Length, corruptOut, (uint)plain.Length, Zlib);
            StorageApi.EnqueueStatus(queue, statuses, 1);
            StorageApi.Submit(queue);

            first = StorageApi.WaitStatus(statuses, 0);
            second = StorageApi.WaitStatus(statuses, 1);
        }

        StorageApi.Release(queue);
        StorageApi.Release(file);
        StorageApi.Release(statuses);

        Assert.Equal(0, first);
        Assert.Equal(content.AsSpan(0x11, range.Length).ToArray(), range);
        Assert.Equal(plain, inflated);
        Assert.Equal(InvalidData, second);
    }

    [DumpFact]
    public void A_package_archive_reads_as_stored_and_its_first_zlib_chunk_inflates_like_system_io_compression()
    {
        var (path, chunkAt, expected) = FindZlibChunk(DumpFactAttribute.Folder!);
        byte[] stored = File.ReadAllBytes(path);
        const int OddOffset = 0x1001;
        int rangeSize = Math.Min(0x3000, stored.Length - OddOffset);
        uint window = (uint)Math.Min(stored.Length - chunkAt, 0x20000);

        using var test = TestRuntime.Create();
        var api = StorageApi.Create(test);
        nint file = api.OpenFile(path);
        nint statuses = api.CreateStatusArray(1);
        nint queue = api.CreateQueue();

        byte[] range = new byte[rangeSize];
        byte[] inflated = new byte[expected.Length];
        int result;

        fixed (byte* rangeOut = range, inflatedOut = inflated)
        {
            StorageApi.EnqueueRead(queue, file, OddOffset, (uint)rangeSize, rangeOut, (uint)rangeSize, None);
            StorageApi.EnqueueRead(queue, file, (ulong)chunkAt, window, inflatedOut, (uint)expected.Length, Zlib);
            StorageApi.EnqueueStatus(queue, statuses, 0);
            StorageApi.Submit(queue);
            result = StorageApi.WaitStatus(statuses, 0);
        }

        StorageApi.Release(queue);
        StorageApi.Release(file);
        StorageApi.Release(statuses);

        Assert.Equal(0, result);
        Assert.Equal(stored.AsSpan(OddOffset, rangeSize).ToArray(), range);
        Assert.Equal(expected, inflated);
    }

    private static byte[] Pattern(int length, int seed)
    {
        byte[] bytes = new byte[length];

        for (int i = 0; i < length; i++)
        {
            bytes[i] = (byte)((i * seed) ^ (i >> 7));
        }

        return bytes;
    }

    private static byte[] Compress(byte[] plain)
    {
        using var output = new MemoryStream();

        using (var zlib = new ZLibStream(output, CompressionLevel.Optimal, true))
        {
            zlib.Write(plain);
        }

        return output.ToArray();
    }

    /// <summary>The smallest archive's first 16-byte aligned zlib stream, with its output from System.IO.Compression.</summary>
    private static (string Path, int Offset, byte[] Inflated) FindZlibChunk(string folder)
    {
        const int ScanLimit = 0x400000;
        const int Window = 0x20000;
        var archives = Directory.EnumerateFiles(folder, "*.rpf", SearchOption.AllDirectories).OrderBy(name => new FileInfo(name).Length);

        foreach (string archive in archives)
        {
            byte[] bytes = File.ReadAllBytes(archive);

            for (int offset = 0; offset + 2 <= Math.Min(bytes.Length, ScanLimit); offset += 0x10)
            {
                if (bytes[offset] != 0x78 || ((bytes[offset] << 8) | bytes[offset + 1]) % 31 != 0)
                {
                    continue;
                }

                byte[]? inflated = TryInflate(bytes.AsSpan(offset, Math.Min(Window, bytes.Length - offset)).ToArray());

                if (inflated is not null && inflated.Length > 0)
                {
                    return (archive, offset, inflated);
                }
            }
        }

        throw new InvalidOperationException("no zlib chunk in the dump's archives");
    }

    private static byte[]? TryInflate(byte[] window)
    {
        try
        {
            using var zlib = new ZLibStream(new MemoryStream(window), CompressionMode.Decompress);
            using var output = new MemoryStream();
            zlib.CopyTo(output);

            return output.ToArray();
        }
        catch (InvalidDataException)
        {
            return null;
        }
    }
}
