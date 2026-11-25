// Copyright (c) 2026 Five Star Recomp contributors.
// SPDX-License-Identifier: GPL-3.0-or-later

using FiveStar.Shaders.Cache;
using FiveStar.Shaders.Containers;
using FiveStar.Shaders.Hardware;
using Xunit;

namespace FiveStar.Shaders.Tests;

/// <summary>
/// Cache files and chunks read from disk: a name that leaves its folder, a length past the data or a count no file
/// could hold makes the file damaged (absent), never a path out of the folder or an exception.
/// </summary>
public sealed class CacheFileHardeningTests : IDisposable
{
    private const string Prefix = "000102030405060708090a0b0c0d0e0f1011121314151617";
    private readonly DirectoryInfo _folder = Directory.CreateTempSubdirectory("fivestar-cache-hardening-");

    public void Dispose()
    {
        _folder.Delete(recursive: true);
    }

    [Theory]
    [InlineData("..\\outside.fssm")]
    [InlineData("\\\\server\\share\\program.fssm")]
    [InlineData("C:\\Windows\\program.fssm")]
    [InlineData("sub/program.fssm")]
    public void A_program_index_naming_a_path_out_of_its_folder_is_damaged(string name)
    {
        var path = Path.Combine(_folder.FullName, ProgramIndexFile.FileName);
        ProgramIndexFile.TryWrite(path, 7, [new ProgramIndexEntry("good.fssm", HardwareStage.Pixel, Prefix), new ProgramIndexEntry(name, HardwareStage.Pixel, Prefix)]);

        Assert.Null(ProgramIndexFile.TryRead(path, 7));
        Assert.False(PlainFileName.Is(name));
        Assert.Throws<InvalidDataException>(() => PlainFileName.Inside(_folder.FullName, name));
        Assert.Equal(Path.Combine(_folder.FullName, "good.dxil"), PlainFileName.Inside(_folder.FullName, "good.dxil"));
    }

    [Fact]
    public void A_program_index_with_a_count_or_a_name_past_its_end_is_damaged()
    {
        var path = Path.Combine(_folder.FullName, ProgramIndexFile.FileName);
        ProgramIndexFile.TryWrite(path, 7, [new ProgramIndexEntry("good.fssm", HardwareStage.Pixel, Prefix)]);
        var bytes = File.ReadAllBytes(path);
        Assert.NotNull(ProgramIndexFile.TryRead(path, 7));

        // The count (at 16) a billion; then the name's length prefix (at 20) past the end of the file.
        var counted = (byte[])bytes.Clone();
        BitConverter.TryWriteBytes(counted.AsSpan(16), 1_000_000_000);
        File.WriteAllBytes(path, counted);
        Assert.Null(ProgramIndexFile.TryRead(path, 7));

        var named = (byte[])bytes.Clone();
        named[20] = 0x7f;
        File.WriteAllBytes(path, named);
        Assert.Null(ProgramIndexFile.TryRead(path, 7));
    }

    [Fact]
    public void A_journal_record_with_a_negative_or_oversized_length_names_nothing()
    {
        var path = Path.Combine(_folder.FullName, PipelineJournalFile.FileName);

        using (var writer = new BinaryWriter(File.Create(path)))
        {
            writer.Write(PipelineJournalFile.Magic);
            writer.Write(PipelineJournalFile.Version);
            Record(writer, PipelineJournalFile.GraphicsKind, [2, -5]);
            Record(writer, PipelineJournalFile.GraphicsKind, [int.MaxValue]);
            Record(writer, PipelineJournalFile.ComputeKind, [3], [1, 2, 3]);
        }

        var prefixes = PipelineJournalFile.ShaderPrefixes(path);

        Assert.Equal([1, 2, 3], Assert.Single(prefixes));
    }

    [Fact]
    public void A_psv0_chunk_whose_info_size_does_not_fit_is_invalid()
    {
        var chunk = new byte[4 + 24];
        BitConverter.TryWriteBytes(chunk, 24);
        Assert.Equal(24, PipelineStateParser.Parse(chunk).InfoSize);

        BitConverter.TryWriteBytes(chunk, 4096);
        Assert.Throws<InvalidDataException>(() => PipelineStateParser.Parse(chunk));
        BitConverter.TryWriteBytes(chunk, -1);
        Assert.Throws<InvalidDataException>(() => PipelineStateParser.Parse(chunk));
        Assert.Throws<InvalidDataException>(() => PipelineStateParser.Parse(new byte[2]));
    }

    /// <summary>A journal record: its kind, then <paramref name="words"/> and <paramref name="bytes"/>.</summary>
    private static void Record(BinaryWriter writer, byte kind, int[] words, byte[]? bytes = null)
    {
        using var memory = new MemoryStream();

        using (var record = new BinaryWriter(memory, System.Text.Encoding.UTF8, leaveOpen: true))
        {
            record.Write(kind);
            Array.ForEach(words, record.Write);
            record.Write(bytes ?? []);
        }

        writer.Write((int)memory.Length);
        writer.Write(memory.ToArray());
    }
}
