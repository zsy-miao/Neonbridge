// Copyright (c) 2026 Five Star Recomp contributors.
// SPDX-License-Identifier: GPL-3.0-or-later

using System.Globalization;

namespace FiveStar.Shaders.Disassembly.Rdna;

/// <summary>
/// The <c>hwreg(...)</c> operand of s_getreg/s_setreg: register id [5:0], bit offset [10:6], size - 1 [15:11]; register
/// names from the AMD "RDNA 2 Instruction Set Architecture" reference guide (hardware registers of S_GETREG_B32).
/// </summary>
internal static class HardwareRegister
{
    private const int IdWidth = 6;
    private const int OffsetLow = 6;
    private const int OffsetWidth = 5;
    private const int SizeLow = 11;
    private const int SizeWidth = 5;
    private const int FullSize = 32;

    private static readonly Dictionary<int, string> Names = new()
    {
        [1] = "HW_REG_MODE", [2] = "HW_REG_STATUS", [3] = "HW_REG_TRAPSTS",
        [5] = "HW_REG_GPR_ALLOC", [6] = "HW_REG_LDS_ALLOC", [7] = "HW_REG_IB_STS", [15] = "HW_REG_SH_MEM_BASES",
        [16] = "HW_REG_TBA_LO", [17] = "HW_REG_TBA_HI", [18] = "HW_REG_TMA_LO", [19] = "HW_REG_TMA_HI",
        [20] = "HW_REG_FLAT_SCR_LO", [21] = "HW_REG_FLAT_SCR_HI", [22] = "HW_REG_XNACK_MASK", [23] = "HW_REG_HW_ID1",
        [24] = "HW_REG_HW_ID2", [25] = "HW_REG_POPS_PACKER",
    };

    public static string Text(int immediate)
    {
        var id = Bits.Field((uint)immediate, 0, IdWidth);
        var offset = Bits.Field((uint)immediate, OffsetLow, OffsetWidth);
        var size = Bits.Field((uint)immediate, SizeLow, SizeWidth) + 1;
        var name = Names.GetValueOrDefault(id) ?? id.ToString(CultureInfo.InvariantCulture);

        if (offset == 0 && size == FullSize)
        {
            return $"hwreg({name})";
        }

        return string.Create(CultureInfo.InvariantCulture, $"hwreg({name}, {offset}, {size})");
    }
}
