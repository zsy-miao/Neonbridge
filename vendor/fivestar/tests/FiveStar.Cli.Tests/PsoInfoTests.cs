// Copyright (c) 2026 Five Star Recomp contributors.
// SPDX-License-Identifier: GPL-3.0-or-later

using System.Buffers.Binary;
using System.Text;
using FiveStar.Cli.Archive;
using Xunit;

namespace FiveStar.Cli.Tests;

/// <summary>A PSO file that names no root still reports its root and blocks.</summary>
public sealed class PsoInfoTests
{
    private const int ChunkHeader = 8;
    private const int TagSize = 4;
    private const uint Struct = 0x12345678;

    [Fact]
    public void A_file_without_a_root_reports_none()
    {
        var path = Path.Combine(Path.GetTempPath(), "fivestar-pso-info-" + Guid.NewGuid().ToString("N") + ".bin");
        var output = new StringWriter();

        File.WriteAllBytes(path, NoRootFile());

        try
        {
            Assert.Equal(0, new PsoReports(output).Info(path));
            Assert.Contains("root block none (type none)", output.ToString(), StringComparison.Ordinal);
            Assert.Contains("block 1 type 12345678", output.ToString(), StringComparison.Ordinal);
        }
        finally
        {
            File.Delete(path);
        }
    }

    private static byte[] NoRootFile()
    {
        using var file = new MemoryStream();
        var row = new byte[16];

        BinaryPrimitives.WriteUInt32LittleEndian(row, Struct);
        BinaryPrimitives.WriteUInt32LittleEndian(row.AsSpan(sizeof(uint)), 16);
        row.AsSpan(8).Fill(0xAA);
        Chunk(file, "PSIN", [.. row, .. new byte[8], .. new byte[16]]);

        var header = new byte[0x20];

        BinaryPrimitives.WriteUInt32LittleEndian(header, Struct);
        BinaryPrimitives.WriteUInt32LittleEndian(header.AsSpan(0x18), 16);
        Chunk(file, "LSCH", header);

        return file.ToArray();
    }

    private static void Chunk(Stream stream, string tag, byte[] payload)
    {
        var header = new byte[ChunkHeader];

        Encoding.ASCII.GetBytes(tag, header.AsSpan(0, TagSize));
        BinaryPrimitives.WriteUInt32BigEndian(header.AsSpan(TagSize), (uint)(ChunkHeader + payload.Length));
        stream.Write(header);
        stream.Write(payload);
    }
}
