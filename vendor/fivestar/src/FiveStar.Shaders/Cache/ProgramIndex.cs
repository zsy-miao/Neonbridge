// Copyright (c) 2026 Five Star Recomp contributors.
// SPDX-License-Identifier: GPL-3.0-or-later

using FiveStar.Shaders.Hardware;

namespace FiveStar.Shaders.Cache;

/// <summary>
/// Finds the extracted FSSM programs of a cached shader. Both code blocks start with the same 24 bytes carrying the
/// shader hash; the code after them is compiled again by the driver and differs.
/// </summary>
public sealed class ProgramIndex
{
    /// <summary>Length of the hash prefix of a code block.</summary>
    public const int PrefixSize = 0x18;

    private readonly Dictionary<string, List<string>> _byPrefix = new(StringComparer.Ordinal);
    private readonly Dictionary<string, HardwareStage> _stages = new(StringComparer.Ordinal);

    private ProgramIndex(IEnumerable<ProgramIndexEntry> entries, bool saved)
    {
        Saved = saved;

        foreach (var entry in entries)
        {
            _stages[entry.Name] = entry.Stage;
            _byPrefix.TryAdd(entry.Prefix, []);
            _byPrefix[entry.Prefix].Add(entry.Name);
        }
    }

    /// <summary>The names of the programs indexed.</summary>
    public IEnumerable<string> Names => _stages.Keys;

    /// <summary>Gets the number of programs indexed.</summary>
    public int Count => _stages.Count;

    /// <summary>Gets a value indicating whether the index was read from the folder's <see cref="ProgramIndexFile"/>.</summary>
    public bool Saved { get; }

    /// <summary>An index without programs.</summary>
    public static ProgramIndex Empty()
    {
        return new ProgramIndex([], false);
    }

    /// <summary>
    /// Indexes every <c>.fssm</c> program of a folder by its code block's hash prefix. The index is saved in the folder
    /// (<see cref="ProgramIndexFile"/>) and read back while no program file changed, so only the first load opens the
    /// programs.
    /// </summary>
    public static ProgramIndex Load(string folder)
    {
        var listing = ProgramFolderListing.Of(folder);
        var indexPath = Path.Combine(folder, ProgramIndexFile.FileName);
        var saved = ProgramIndexFile.TryRead(indexPath, listing.Stamp);

        if (saved is not null)
        {
            return new ProgramIndex(saved, true);
        }

        var entries = ReadPrograms(listing.Paths);
        ProgramIndexFile.TryWrite(indexPath, listing.Stamp, entries);

        return new ProgramIndex(entries, false);
    }

    /// <summary>
    /// The index with every program <paramref name="renamed"/> picks under its name plus <paramref name="suffix"/>, so
    /// what is made for it (DXIL, links) stays apart from the program's usual files.
    /// </summary>
    public ProgramIndex Renamed(Func<string, bool> renamed, string suffix)
    {
        var entries = new List<ProgramIndexEntry>();

        foreach (var (prefix, names) in _byPrefix)
        {
            foreach (var name in names)
            {
                string kept = name;

                if (renamed(name))
                {
                    kept = name + suffix;
                }

                entries.Add(new ProgramIndexEntry(kept, _stages[name], prefix));
            }
        }

        return new ProgramIndex(entries, Saved);
    }

    /// <summary>The names (file stems) of the programs whose code starts like <paramref name="code"/>.</summary>
    public IReadOnlyList<string> Find(ReadOnlySpan<byte> code)
    {
        if (code.Length < PrefixSize)
        {
            return [];
        }

        if (_byPrefix.TryGetValue(Convert.ToHexStringLower(code[..PrefixSize]), out var names))
        {
            return names;
        }

        return [];
    }

    /// <summary>The hardware stage of an indexed program.</summary>
    public HardwareStage Stage(string name)
    {
        return _stages[name];
    }

    /// <summary>The stage of <paramref name="name"/>; false for a name the index does not hold (a linked or trimmed form).</summary>
    public bool TryStage(string name, out HardwareStage stage)
    {
        return _stages.TryGetValue(name, out stage);
    }

    private static List<ProgramIndexEntry> ReadPrograms(IReadOnlyList<string> paths)
    {
        var entries = new List<ProgramIndexEntry>(paths.Count);

        foreach (var path in paths)
        {
            var program = File.ReadAllBytes(path);

            if (!FssmHeader.HasMagic(program))
            {
                continue;
            }

            var config = FssmHeader.Decode(program);
            var codeOffset = config.CodeOffset ?? program.Length;

            if (config.Stage is null || codeOffset + PrefixSize > program.Length)
            {
                continue;
            }

            var prefix = Convert.ToHexStringLower(program.AsSpan((int)codeOffset, PrefixSize));
            entries.Add(new ProgramIndexEntry(Path.GetFileNameWithoutExtension(path), config.Stage.Value, prefix));
        }

        return entries;
    }
}
