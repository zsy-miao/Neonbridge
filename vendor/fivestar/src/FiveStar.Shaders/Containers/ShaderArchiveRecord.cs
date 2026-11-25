// Copyright (c) 2026 Five Star Recomp contributors.
// SPDX-License-Identifier: GPL-3.0-or-later

namespace FiveStar.Shaders.Containers;

/// <summary>
/// One shader record of an SGD5 table: where it sits in the archive, its name and kind tag, the sizes from its
/// header and the decompressed DXBC container.
/// </summary>
public sealed record ShaderArchiveRecord(
    string Region,
    long RecordOffset,
    long TagOffset,
    int Tag,
    string Name,
    int CompressedSize,
    int DecompressedSize,
    uint Constant,
    byte[] Container,
    IReadOnlyList<string> Chunks)
{
    private const int MaxSafeNameLength = 150;
    private const char Replacement = '~';

    /// <summary>The DXBC content hash as lower-case hex.</summary>
    public string HashHex => Convert.ToHexStringLower(Container, DxbcContainer.HashOffset, DxbcContainer.HashSize);

    /// <summary>
    /// The file name used for this program: region, tag offset (8 hex digits), tag (2 hex digits) and the name
    /// with every character outside [A-Za-z0-9_.-] replaced by '~', cut at 150 characters.
    /// </summary>
    public string FileStem
    {
        get
        {
            var safe = new string(Name.Select(Safe).ToArray());

            if (safe.Length > MaxSafeNameLength)
            {
                safe = safe[..MaxSafeNameLength];
            }

            return $"{Region}_{TagOffset:x8}_t{Tag:x2}_{safe}";
        }
    }

    private static char Safe(char c)
    {
        var keep = char.IsAsciiLetterOrDigit(c) || c == '_' || c == '.' || c == '-';
        var result = keep ? c : Replacement;

        return result;
    }
}
