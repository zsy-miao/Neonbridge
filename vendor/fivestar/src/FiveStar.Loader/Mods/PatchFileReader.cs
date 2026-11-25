// Copyright (c) 2026 Five Star Recomp contributors.
// SPDX-License-Identifier: GPL-3.0-or-later

using System.Globalization;

namespace FiveStar.Loader.Mods;

/// <summary>
/// Reads patch files: a <c>[name]</c> line starts a patch, <c>key = value</c> lines fill it, <c>#</c> starts a comment.
/// Keys: module, sha256, enabled, description, pattern, matches, offset, bytes, jump. Errors cite file and line.
/// </summary>
internal static class PatchFileReader
{
    /// <summary>The extension of the patch files read from a folder.</summary>
    public const string Extension = ".fspatch";

    private const char CommentMarker = '#';
    private const char SectionStart = '[';
    private const char SectionEnd = ']';
    private const char Separator = '=';
    private const string HexPrefix = "0x";
    private const string NegativeHexPrefix = "-0x";
    private const int Sha256HexLength = 64;

    private static readonly string[] Keys = ["module", "sha256", "enabled", "description", "pattern", "matches", "offset", "bytes", "jump"];

    /// <summary>Every patch of each path: a file, or a folder whose patch files are read in name order.</summary>
    public static List<CodePatch> ReadPaths(IEnumerable<string> paths)
    {
        var patches = new List<CodePatch>();

        foreach (string path in paths)
        {
            if (Directory.Exists(path))
            {
                var files = Directory.EnumerateFiles(path, "*" + Extension).Order(StringComparer.OrdinalIgnoreCase);
                patches.AddRange(files.SelectMany(Read));

                continue;
            }

            patches.AddRange(Read(path));
        }

        return patches;
    }

    public static List<CodePatch> Read(string path)
    {
        return Parse(File.ReadAllLines(path), Path.GetFullPath(path));
    }

    /// <summary>Parses the lines of one file; <paramref name="source"/> names it in errors and outcomes.</summary>
    public static List<CodePatch> Parse(IReadOnlyList<string> lines, string source)
    {
        var patches = new List<CodePatch>();
        Dictionary<string, string>? fields = null;
        string name = string.Empty;
        int header = 0;

        for (int i = 0; i < lines.Count; i++)
        {
            string line = StripComment(lines[i]);

            if (line.Length == 0)
            {
                continue;
            }

            if (line[0] == SectionStart)
            {
                AddPatch(patches, fields, name, $"{source}:{header}");
                (name, fields, header) = (SectionName(line, source, i + 1), [], i + 1);

                continue;
            }

            if (fields is null)
            {
                throw Error(source, i + 1, "a key outside a [patch] section");
            }

            AddField(fields, line, source, i + 1);
        }

        AddPatch(patches, fields, name, $"{source}:{header}");

        return patches;
    }

    private static string StripComment(string line)
    {
        int comment = line.IndexOf(CommentMarker, StringComparison.Ordinal);

        if (comment >= 0)
        {
            line = line[..comment];
        }

        return line.Trim();
    }

    private static string SectionName(string line, string source, int number)
    {
        if (line[^1] != SectionEnd || line.Length < "[x]".Length)
        {
            throw Error(source, number, $"'{line}' is not a [name] line");
        }

        return line[1..^1].Trim();
    }

    private static void AddField(Dictionary<string, string> fields, string line, string source, int number)
    {
        int separator = line.IndexOf(Separator, StringComparison.Ordinal);

        if (separator <= 0)
        {
            throw Error(source, number, $"'{line}' is not key = value");
        }

        string key = line[..separator].Trim().ToLowerInvariant();

        if (!Keys.Contains(key))
        {
            throw Error(source, number, $"unknown key '{key}'");
        }

        if (!fields.TryAdd(key, line[(separator + 1)..].Trim()))
        {
            throw Error(source, number, $"'{key}' given twice");
        }
    }

    private static void AddPatch(List<CodePatch> patches, Dictionary<string, string>? fields, string name, string where)
    {
        if (fields is null)
        {
            return;
        }

        try
        {
            patches.Add(Build(name, fields, where));
        }
        catch (FormatException e)
        {
            throw new InvalidDataException($"{where}: [{name}]: {e.Message}", e);
        }
    }

    private static CodePatch Build(string name, Dictionary<string, string> fields, string where)
    {
        string module = Required(fields, "module");
        var pattern = BytePattern.Parse(Required(fields, "pattern"));
        string? bytesText = fields.GetValueOrDefault("bytes");
        string? jump = fields.GetValueOrDefault("jump");

        if ((bytesText is null) == (jump is null))
        {
            throw new FormatException("give exactly one of 'bytes' and 'jump'");
        }

        byte[]? bytes = null;

        if (bytesText is not null)
        {
            bytes = ParseBytes(bytesText);
        }

        if (jump is not null && !jump.Contains('!', StringComparison.Ordinal))
        {
            throw new FormatException("'jump' takes module!symbol");
        }

        string? sha256 = Sha256(fields.GetValueOrDefault("sha256"));
        bool enabled = bool.Parse(fields.GetValueOrDefault("enabled") ?? bool.TrueString);
        string description = fields.GetValueOrDefault("description") ?? string.Empty;
        int matches = int.Parse(fields.GetValueOrDefault("matches") ?? "1", CultureInfo.InvariantCulture);
        int offset = ParseOffset(fields.GetValueOrDefault("offset"));

        if (matches < 1)
        {
            throw new FormatException("'matches' must be at least 1");
        }

        return new CodePatch(name, module, sha256, enabled, description, pattern, matches, offset, bytes, jump, where);
    }

    private static string Required(Dictionary<string, string> fields, string key)
    {
        if (!fields.TryGetValue(key, out string? value) || value.Length == 0)
        {
            throw new FormatException($"'{key}' is required");
        }

        return value;
    }

    private static byte[] ParseBytes(string text)
    {
        string[] tokens = text.Split(' ', StringSplitOptions.RemoveEmptyEntries);

        if (tokens.Length == 0)
        {
            throw new FormatException("'bytes' is empty");
        }

        return [.. tokens.Select(t => byte.Parse(t, NumberStyles.AllowHexSpecifier, CultureInfo.InvariantCulture))];
    }

    private static string? Sha256(string? text)
    {
        if (text is null)
        {
            return null;
        }

        if (text.Length != Sha256HexLength || !text.All(char.IsAsciiHexDigit))
        {
            throw new FormatException("'sha256' takes 64 hex digits");
        }

        return text.ToUpperInvariant();
    }

    private static int ParseOffset(string? text)
    {
        if (text is null)
        {
            return 0;
        }

        if (text.StartsWith(NegativeHexPrefix, StringComparison.OrdinalIgnoreCase))
        {
            return -int.Parse(text[NegativeHexPrefix.Length..], NumberStyles.AllowHexSpecifier, CultureInfo.InvariantCulture);
        }

        if (text.StartsWith(HexPrefix, StringComparison.OrdinalIgnoreCase))
        {
            return int.Parse(text[HexPrefix.Length..], NumberStyles.AllowHexSpecifier, CultureInfo.InvariantCulture);
        }

        return int.Parse(text, NumberStyles.AllowLeadingSign, CultureInfo.InvariantCulture);
    }

    private static InvalidDataException Error(string source, int line, string message)
    {
        return new InvalidDataException($"{source}:{line}: {message}");
    }
}
