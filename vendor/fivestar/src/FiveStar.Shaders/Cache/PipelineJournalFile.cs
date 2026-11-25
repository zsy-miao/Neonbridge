// Copyright (c) 2026 Five Star Recomp contributors.
// SPDX-License-Identifier: GPL-3.0-or-later

namespace FiveStar.Shaders.Cache;

/// <summary>
/// The file of the loader's pipeline journal, <c>pipelines-used.bin</c> (docs/d3d12x-pipelines.md, Warm-up):
/// <c>FSPJ</c>, u32 version, then records of u32 length and payload. A payload starts with its kind (0 graphics with an
/// i32 shader count, 1 compute with one shader) and its shaders, each an i32 length and the code-block prefix that
/// names the program; the graphics state follows. The loader writes it; the translator reads which programs the game
/// used.
/// </summary>
public static class PipelineJournalFile
{
    public const string FileName = "pipelines-used.bin";
    public const uint Magic = 0x4A505346;
    public const uint Version = 1;
    public const byte GraphicsKind = 0;
    public const byte ComputeKind = 1;

    /// <summary>The header's size in bytes.</summary>
    public const int HeaderSize = 2 * sizeof(uint);

    /// <summary>The records of a journal file; null when it is missing or not a journal. A cut-off last record is dropped.</summary>
    public static List<byte[]>? Records(string path)
    {
        if (!File.Exists(path))
        {
            return null;
        }

        using var reader = new BinaryReader(File.Open(path, FileMode.Open, FileAccess.Read, FileShare.ReadWrite));
        var stream = reader.BaseStream;

        if (stream.Length < HeaderSize || reader.ReadUInt32() != Magic || reader.ReadUInt32() != Version)
        {
            return null;
        }

        var records = new List<byte[]>();

        while (stream.Length - stream.Position >= sizeof(int))
        {
            var length = reader.ReadInt32();

            if (length <= 0 || length > stream.Length - stream.Position)
            {
                break;
            }

            records.Add(reader.ReadBytes(length));
        }

        return records;
    }

    /// <summary>The shader prefixes of the journal's pipelines in first-use order, each once; none without a journal.</summary>
    public static List<byte[]> ShaderPrefixes(string path)
    {
        var prefixes = new List<byte[]>();
        var seen = new HashSet<string>(StringComparer.Ordinal);

        foreach (var record in Records(path) ?? [])
        {
            try
            {
                using var reader = new BinaryReader(new MemoryStream(record));
                var count = reader.ReadByte() == ComputeKind ? 1 : reader.ReadInt32();

                // Each shader takes at least its length: a count or length past the record is damage.
                if (count < 0 || count > record.Length / sizeof(int))
                {
                    continue;
                }

                for (var shader = 0; shader < count; shader++)
                {
                    var length = reader.ReadInt32();

                    if (length < 0 || length > reader.BaseStream.Length - reader.BaseStream.Position)
                    {
                        break;
                    }

                    var prefix = reader.ReadBytes(length);

                    if (seen.Add(Convert.ToHexString(prefix)))
                    {
                        prefixes.Add(prefix);
                    }
                }
            }
            catch (Exception exception) when (exception is EndOfStreamException or ArgumentException or FormatException)
            {
                // A damaged record names nothing.
            }
        }

        return prefixes;
    }
}
