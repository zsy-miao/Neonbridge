// Copyright (c) 2026 Five Star Recomp contributors.
// SPDX-License-Identifier: GPL-3.0-or-later

using System.Globalization;
using System.Text;

namespace FiveStar.Shaders.Disassembly.Rdna;

/// <summary>
/// The counters of s_waitcnt (VM_CNT [3:0] and [15:14], EXP_CNT [6:4], LGKM_CNT [13:8]) and the fields of
/// s_waitcnt_depctr, as the AMD "RDNA 2 Instruction Set Architecture" reference guide lays them out. A counter at its
/// maximum (no wait) is left out unless all are.
/// </summary>
internal static class WaitCounts
{
    private const int VmLowWidth = 4;
    private const int VmHighLow = 14;
    private const int VmHighWidth = 2;
    private const int ExpLow = 4;
    private const int ExpWidth = 3;
    private const int LgkmLow = 8;
    private const int LgkmWidth = 6;
    private const int VmMaximum = 63;
    private const int ExpMaximum = 7;
    private const int LgkmMaximum = 63;

    /// <summary>s_waitcnt_depctr fields in print order: name, low bit, width.</summary>
    private static readonly (string Name, int Low, int Width)[] DependencyFields =
    [
        ("depctr_sa_sdst", 0, 1),
        ("depctr_va_vdst", 12, 4),
        ("depctr_va_sdst", 9, 3),
        ("depctr_va_ssrc", 8, 1),
        ("depctr_va_vcc", 1, 1),
        ("depctr_vm_vsrc", 2, 3),
    ];

    public static string Text(int immediate)
    {
        var word = (uint)immediate;
        var vm = Bits.Field(word, 0, VmLowWidth) | (Bits.Field(word, VmHighLow, VmHighWidth) << VmLowWidth);
        var exp = Bits.Field(word, ExpLow, ExpWidth);
        var lgkm = Bits.Field(word, LgkmLow, LgkmWidth);
        var all = vm == VmMaximum && exp == ExpMaximum && lgkm == LgkmMaximum;
        var parts = new List<string>();

        if (all || vm != VmMaximum)
        {
            parts.Add(Counter("vmcnt", vm));
        }

        if (all || exp != ExpMaximum)
        {
            parts.Add(Counter("expcnt", exp));
        }

        if (all || lgkm != LgkmMaximum)
        {
            parts.Add(Counter("lgkmcnt", lgkm));
        }

        return string.Join(' ', parts);
    }

    /// <summary>The fields that differ from their maximum, all fields when none does, or hex when unnamed bits are set.</summary>
    public static string DependencyText(int immediate)
    {
        var word = (uint)immediate;
        var known = 0u;
        var anyWait = false;

        foreach (var (_, low, width) in DependencyFields)
        {
            var mask = ((1u << width) - 1) << low;
            known |= mask;
            anyWait |= (word & mask) != mask;
        }

        if ((word & ~known) != 0)
        {
            return OperandText.Hex(word);
        }

        var text = new StringBuilder();

        foreach (var (name, low, width) in DependencyFields)
        {
            var value = Bits.Field(word, low, width);

            if (anyWait && value == (1 << width) - 1)
            {
                continue;
            }

            text.Append(CultureInfo.InvariantCulture, $"{name}({value}) ");
        }

        return text.ToString().TrimEnd();
    }

    private static string Counter(string name, int value)
    {
        return string.Create(CultureInfo.InvariantCulture, $"{name}({value})");
    }
}
