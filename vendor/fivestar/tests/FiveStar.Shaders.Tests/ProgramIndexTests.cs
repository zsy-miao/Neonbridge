// Copyright (c) 2026 Five Star Recomp contributors.
// SPDX-License-Identifier: GPL-3.0-or-later

using System.Buffers.Binary;
using System.Text;
using FiveStar.Shaders.Cache;
using FiveStar.Shaders.Hardware;
using Xunit;

namespace FiveStar.Shaders.Tests;

/// <summary>The program index is saved in the program folder and read back until a program changes.</summary>
public sealed class ProgramIndexTests
{
    private const int CodeOffset = 0x400;
    private const byte Compute = 6;

    [Fact]
    public void A_saved_index_is_read_back_until_a_program_changes()
    {
        var folder = Directory.CreateTempSubdirectory("fivestar-index-").FullName;

        try
        {
            File.WriteAllBytes(Path.Combine(folder, "a.fssm"), Program(0x11));
            File.WriteAllBytes(Path.Combine(folder, "b.fssm"), Program(0x22));

            var built = ProgramIndex.Load(folder);
            var saved = ProgramIndex.Load(folder);

            Assert.False(built.Saved);
            Assert.True(saved.Saved);
            Assert.Equal(["a"], saved.Find(Prefix(0x11)));
            Assert.Equal(HardwareStage.Compute, saved.Stage("b"));

            File.WriteAllBytes(Path.Combine(folder, "b.fssm"), [.. Program(0x33), 0]);
            var rebuilt = ProgramIndex.Load(folder);

            Assert.False(rebuilt.Saved);
            Assert.Empty(rebuilt.Find(Prefix(0x22)));
            Assert.Equal(["b"], rebuilt.Find(Prefix(0x33)));
        }
        finally
        {
            Directory.Delete(folder, recursive: true);
        }
    }

    private static byte[] Prefix(byte fill)
    {
        return Enumerable.Repeat(fill, ProgramIndex.PrefixSize).ToArray();
    }

    // A compute program whose code block (stored offset relative to FSSM+0x10) starts with the hash prefix.
    private static byte[] Program(byte fill)
    {
        var bytes = new byte[0x600];
        Encoding.ASCII.GetBytes(FssmHeader.Magic).CopyTo(bytes, 0);
        BinaryPrimitives.WriteUInt64LittleEndian(bytes.AsSpan(0xC0), CodeOffset - FssmHeader.OffsetBase);
        BinaryPrimitives.WriteUInt64LittleEndian(bytes.AsSpan(0xC8), ProgramIndex.PrefixSize);
        bytes[0x100] = Compute;
        Prefix(fill).CopyTo(bytes, CodeOffset);

        return bytes;
    }
}
