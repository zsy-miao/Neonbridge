// Copyright (c) 2026 Five Star Recomp contributors.
// SPDX-License-Identifier: GPL-3.0-or-later

using System.Globalization;

namespace FiveStar.Shaders.Disassembly.Rdna;

/// <summary>
/// A branch operand: llvm-objdump prints the raw 16-bit offset in decimal and labels the target, which
/// <see cref="ObjdumpParser"/> turns into <c>@T0x&lt;target&gt;</c>. The target is the next instruction plus the signed
/// offset in dwords; one before the program start wraps to a 64-bit value.
/// </summary>
internal static class Branch
{
    private const int BytesPerWord = 4;
    private const int ImmediateWidth = 16;

    public static string Text(int at, int immediate)
    {
        var offset = Bits.Signed((uint)immediate, 0, ImmediateWidth);
        var target = (ulong)((long)(at + 1 + offset) * BytesPerWord);

        return string.Create(CultureInfo.InvariantCulture, $"{immediate} @T0x{target:x}");
    }
}
