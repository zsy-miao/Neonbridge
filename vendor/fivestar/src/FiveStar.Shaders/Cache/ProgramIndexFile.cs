// Copyright (c) 2026 Five Star Recomp contributors.
// SPDX-License-Identifier: GPL-3.0-or-later

using FiveStar.Shaders.Hardware;

namespace FiveStar.Shaders.Cache;

/// <summary>
/// The saved <see cref="ProgramIndex"/> of a program folder (<see cref="FileName"/> inside it), so that indexing reads
/// one file instead of every program. It holds the <see cref="ProgramFolderListing.Stamp"/> it was built from and is
/// ignored when the folder no longer matches.
/// </summary>
/// <remarks>
/// Layout, little-endian: u32 magic <c>FSPX</c>, u32 version, u64 stamp, i32 count, then per program a
/// length-prefixed UTF-8 name (<see cref="BinaryWriter.Write(string)"/>), a u8 stage and the 24-byte hash prefix.
/// </remarks>
public static class ProgramIndexFile
{
    /// <summary>Name of the index file inside the program folder.</summary>
    public const string FileName = "programs.fspx";

    private const uint Magic = 0x58505346; // "FSPX"
    private const uint Version = 1;
    private const string TemporarySuffix = ".tmp";
    private const int MinimumEntryBytes = 2 + 1 + ProgramIndex.PrefixSize;

    /// <summary>The entries saved in <paramref name="path"/> for <paramref name="stamp"/>, or null when absent, stale or damaged.</summary>
    public static IReadOnlyList<ProgramIndexEntry>? TryRead(string path, ulong stamp)
    {
        try
        {
            using var reader = new BinaryReader(File.OpenRead(path));

            if (reader.ReadUInt32() != Magic || reader.ReadUInt32() != Version || reader.ReadUInt64() != stamp)
            {
                return null;
            }

            return ReadEntries(reader);
        }
        catch (Exception exception) when (exception is IOException or UnauthorizedAccessException or ArgumentException or FormatException)
        {
            // Damaged (EndOfStreamException is an IOException): as if absent.
            return null;
        }
    }

    /// <summary>Saves <paramref name="entries"/> for <paramref name="stamp"/>; a folder that cannot be written keeps no index.</summary>
    public static void TryWrite(string path, ulong stamp, IReadOnlyList<ProgramIndexEntry> entries)
    {
        // A unique name per writer: processes that index the same folder at once each replace the file whole.
        var temporary = path + "." + Guid.NewGuid().ToString("N") + TemporarySuffix;

        try
        {
            using (var writer = new BinaryWriter(File.Create(temporary)))
            {
                writer.Write(Magic);
                writer.Write(Version);
                writer.Write(stamp);
                writer.Write(entries.Count);

                foreach (var entry in entries)
                {
                    writer.Write(entry.Name);
                    writer.Write((byte)entry.Stage);
                    writer.Write(Convert.FromHexString(entry.Prefix));
                }
            }

            File.Move(temporary, path, overwrite: true);
        }
        catch (Exception exception) when (exception is IOException or UnauthorizedAccessException)
        {
            File.Delete(temporary);
        }
    }

    private static List<ProgramIndexEntry>? ReadEntries(BinaryReader reader)
    {
        int count = reader.ReadInt32();
        long remaining = reader.BaseStream.Length - reader.BaseStream.Position;

        // Every entry takes at least its name's length byte, a name byte, the stage and the prefix.
        if (count < 0 || count > remaining / MinimumEntryBytes)
        {
            return null;
        }

        var entries = new List<ProgramIndexEntry>(count);

        for (int i = 0; i < count; i++)
        {
            var name = reader.ReadString();
            var stage = (HardwareStage)reader.ReadByte();
            var prefix = reader.ReadBytes(ProgramIndex.PrefixSize);

            // A name names a file in the program folder: one that is not a plain file name (a path, "..", a UNC
            // name) makes the whole index damaged.
            if (!Enum.IsDefined(stage) || prefix.Length != ProgramIndex.PrefixSize || !PlainFileName.Is(name))
            {
                return null;
            }

            entries.Add(new ProgramIndexEntry(name, stage, Convert.ToHexStringLower(prefix)));
        }

        return entries;
    }
}
