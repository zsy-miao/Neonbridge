// Copyright (c) 2026 Five Star Recomp contributors.
// SPDX-License-Identifier: GPL-3.0-or-later

using System.Globalization;
using System.Text;

namespace FiveStar.Formats.Platform;

/// <summary>
/// The members of a COFF library (<c>.lib</c>, PE/COFF specification "Archive (Library) File Format"): an 8-byte
/// signature, then members, each a 60-byte text header and its data padded to an even size.
/// </summary>
public static class CoffArchive
{
    private const int HeaderSize = 60;
    private const int SizeField = 48;
    private const int SizeFieldLength = 10;

    private static readonly byte[] Signature = "!<arch>\n"u8.ToArray();

    /// <summary>The data of every member, linker and long-name members included.</summary>
    public static IEnumerable<ReadOnlyMemory<byte>> Members(byte[] library)
    {
        if (!library.AsSpan().StartsWith(Signature))
        {
            throw new InvalidDataException("not a COFF library");
        }

        var at = Signature.Length;

        while (at + HeaderSize <= library.Length)
        {
            var sizeText = Encoding.ASCII.GetString(library, at + SizeField, SizeFieldLength).Trim();
            var size = int.Parse(sizeText, NumberStyles.None, CultureInfo.InvariantCulture);
            var data = at + HeaderSize;

            yield return library.AsMemory(data, Math.Min(size, library.Length - data));

            // Members start on even offsets.
            at = data + size + (size & 1);
        }
    }
}
