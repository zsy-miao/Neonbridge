// Copyright (c) 2026 Five Star Recomp contributors.
// SPDX-License-Identifier: GPL-3.0-or-later

namespace FiveStar.Formats.Platform;

/// <summary>The x64 exception directory (<c>.pdata</c>) as a lookup from a code RVA to the start of its function.</summary>
public static class PeFunctionTable
{
    // RUNTIME_FUNCTION (PE/COFF specification, .pdata): BeginAddress, EndAddress, UnwindInfoAddress, each a u32 RVA.
    private const int RuntimeFunctionSize = 12;
    private const int EndAddressOffset = 4;

    // Binary search splits the remaining entries in two.
    private const int Halves = 2;

    /// <summary>The BeginAddress of the RUNTIME_FUNCTION that covers <paramref name="rva"/>, or null when none does.</summary>
    public static uint? FunctionStart(PeImage image, uint rva)
    {
        var (table, size) = image.Directory(PeDirectory.Exception);
        var offset = image.Offset(table);

        if (table == 0 || offset is null)
        {
            return null;
        }

        var low = 0;
        var high = ((int)size / RuntimeFunctionSize) - 1;

        // The entries are sorted by BeginAddress.
        while (low <= high)
        {
            var middle = low + ((high - low) / Halves);
            var entry = (int)offset.Value + (middle * RuntimeFunctionSize);
            var begin = image.U32(entry);

            if (rva < begin)
            {
                high = middle - 1;
            }
            else if (rva >= image.U32(entry + EndAddressOffset))
            {
                low = middle + 1;
            }
            else
            {
                return begin;
            }
        }

        return null;
    }
}
