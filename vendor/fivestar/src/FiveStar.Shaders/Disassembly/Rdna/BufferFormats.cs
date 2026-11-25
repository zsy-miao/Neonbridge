// Copyright (c) 2026 Five Star Recomp contributors.
// SPDX-License-Identifier: GPL-3.0-or-later

using System.Globalization;

namespace FiveStar.Shaders.Disassembly.Rdna;

/// <summary>
/// The MTBUF format field: the unified buffer format of the AMD "RDNA 2 Instruction Set Architecture" reference guide
/// (buffer data formats). Format 1 is the default and prints nothing; values without a name print as numbers.
/// </summary>
internal static class BufferFormats
{
    private const int DefaultFormat = 1;

    private static readonly string[] Layouts =
    [
        "INVALID", "8_UNORM", "8_SNORM", "8_USCALED", "8_SSCALED", "8_UINT", "8_SINT", "16_UNORM", "16_SNORM", "16_USCALED",
        "16_SSCALED", "16_UINT", "16_SINT", "16_FLOAT", "8_8_UNORM", "8_8_SNORM", "8_8_USCALED", "8_8_SSCALED", "8_8_UINT",
        "8_8_SINT", "32_UINT", "32_SINT", "32_FLOAT", "16_16_UNORM", "16_16_SNORM", "16_16_USCALED", "16_16_SSCALED",
        "16_16_UINT", "16_16_SINT", "16_16_FLOAT", "10_11_11_UNORM", "10_11_11_SNORM", "10_11_11_USCALED",
        "10_11_11_SSCALED", "10_11_11_UINT", "10_11_11_SINT", "10_11_11_FLOAT", "11_11_10_UNORM", "11_11_10_SNORM",
        "11_11_10_USCALED", "11_11_10_SSCALED", "11_11_10_UINT", "11_11_10_SINT", "11_11_10_FLOAT", "10_10_10_2_UNORM",
        "10_10_10_2_SNORM", "10_10_10_2_USCALED", "10_10_10_2_SSCALED", "10_10_10_2_UINT", "10_10_10_2_SINT",
        "2_10_10_10_UNORM", "2_10_10_10_SNORM", "2_10_10_10_USCALED", "2_10_10_10_SSCALED", "2_10_10_10_UINT",
        "2_10_10_10_SINT", "8_8_8_8_UNORM", "8_8_8_8_SNORM", "8_8_8_8_USCALED", "8_8_8_8_SSCALED", "8_8_8_8_UINT",
        "8_8_8_8_SINT", "32_32_UINT", "32_32_SINT", "32_32_FLOAT", "16_16_16_16_UNORM", "16_16_16_16_SNORM",
        "16_16_16_16_USCALED", "16_16_16_16_SSCALED", "16_16_16_16_UINT", "16_16_16_16_SINT", "16_16_16_16_FLOAT",
        "32_32_32_UINT", "32_32_32_SINT", "32_32_32_FLOAT", "32_32_32_32_UINT", "32_32_32_32_SINT", "32_32_32_32_FLOAT",
    ];

    /// <summary>" format:[BUF_FMT_...]", " format:N" or nothing for the default.</summary>
    public static string Text(int format)
    {
        if (format == DefaultFormat)
        {
            return string.Empty;
        }

        if (format < Layouts.Length)
        {
            return " format:[BUF_FMT_" + Layouts[format] + "]";
        }

        return string.Create(CultureInfo.InvariantCulture, $" format:{format}");
    }
}
