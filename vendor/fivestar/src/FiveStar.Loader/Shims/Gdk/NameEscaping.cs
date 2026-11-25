// Copyright (c) 2026 Five Star Recomp contributors.
// SPDX-License-Identifier: GPL-3.0-or-later

using System.Globalization;
using System.Text;

namespace FiveStar.Loader.Shims.Gdk;

/// <summary>
/// File-system-safe form of a GDK name (container, blob, configuration id): ASCII letters, digits, '_' and '-' stay,
/// '.' stays except in first position, every other UTF-8 byte becomes %XX.
/// </summary>
internal static class NameEscaping
{
    private const char Escape = '%';
    private const int HexDigits = 2;
    private const string HexFormat = "X2";

    public static string Encode(string name)
    {
        var builder = new StringBuilder();

        foreach (byte b in Encoding.UTF8.GetBytes(name))
        {
            char c = (char)b;
            bool keep = char.IsAsciiLetterOrDigit(c) || c == '_' || c == '-' || (c == '.' && builder.Length > 0);

            if (keep)
            {
                builder.Append(c);
            }
            else
            {
                builder.Append(Escape).Append(b.ToString(HexFormat, CultureInfo.InvariantCulture));
            }
        }

        return builder.ToString();
    }

    public static string Decode(string name)
    {
        var bytes = new List<byte>();

        for (int i = 0; i < name.Length; i++)
        {
            bool escaped = name[i] == Escape && i + HexDigits < name.Length;

            if (escaped && byte.TryParse(name.AsSpan(i + 1, HexDigits), NumberStyles.HexNumber, CultureInfo.InvariantCulture, out byte value))
            {
                bytes.Add(value);
                i += HexDigits;
            }
            else
            {
                bytes.Add((byte)name[i]);
            }
        }

        return Encoding.UTF8.GetString(bytes.ToArray());
    }
}
