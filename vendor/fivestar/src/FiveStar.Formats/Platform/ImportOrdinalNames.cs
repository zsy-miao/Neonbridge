// Copyright (c) 2026 Five Star Recomp contributors.
// SPDX-License-Identifier: GPL-3.0-or-later

namespace FiveStar.Formats.Platform;

/// <summary>
/// Names of system DLL exports that modules commonly import by ordinal only (OLEAUT32's BSTR and VARIANT helpers,
/// whose ordinals are fixed by the Windows SDK import library).
/// </summary>
public static class ImportOrdinalNames
{
    private static readonly string?[] OleAutomation =
    [
        null, null, "SysAllocString", "SysReAllocString", "SysAllocStringLen", "SysReAllocStringLen", "SysFreeString",
        "SysStringLen", "VariantInit", "VariantClear", "VariantCopy",
    ];

    /// <summary>The function name behind an ordinal import, or null when it is not known.</summary>
    public static string? Find(string dll, ulong ordinal)
    {
        if (!dll.Equals("oleaut32.dll", StringComparison.OrdinalIgnoreCase) || ordinal >= (ulong)OleAutomation.Length)
        {
            return null;
        }

        return OleAutomation[ordinal];
    }
}
