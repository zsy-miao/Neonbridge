// Copyright (c) 2026 Five Star Recomp contributors.
// SPDX-License-Identifier: GPL-3.0-or-later

using System.Globalization;
using System.Text;

namespace FiveStar.Shaders.Disassembly.Rdna;

/// <summary>
/// The DPP16 and DPP8 forms of VOP1/VOP2 (AMD "RDNA 2 Instruction Set Architecture", DPP16 and DPP8 formats). DPP16
/// dword: SRC0 [7:0], DPP_CTRL [16:8], FI [18], BOUND_CTRL [19], SRC0_NEG/ABS [20]/[21], SRC1_NEG/ABS [22]/[23],
/// BANK_MASK [27:24], ROW_MASK [31:28]. DPP8 dword: SRC0 [7:0], then eight 3-bit lane selects.
/// </summary>
internal static class DppDecoder
{
    private const int Words = 2;
    private const int ByteWidth = 8;
    private const int ControlLow = 8;
    private const int ControlWidth = 9;
    private const int FetchInactiveBit = 18;
    private const int BoundControlBit = 19;
    private const int Source0NegateBit = 20;
    private const int ModifierLow = 20;
    private const int ModifierWidth = 4;
    private const int Source0AbsoluteBit = 21;
    private const int Source1NegateBit = 22;
    private const int Source1AbsoluteBit = 23;
    private const int BankMaskLow = 24;
    private const int RowMaskLow = 28;
    private const int MaskWidth = 4;
    private const int Lanes = 8;
    private const int LaneSelectWidth = 3;
    private const int QuadPermEnd = 0xFF;
    private const int LaneSelectBits = 2;
    private const int QuadLanes = 4;

    /// <summary>DPP16 row operations by control base: the name and the valid offsets 1-15 or 0-15.</summary>
    private static readonly (int Base, string Name, int First)[] RowControls =
    [
        (0x100, "row_shl", 1), (0x110, "row_shr", 1), (0x120, "row_ror", 1), (0x150, "row_share", 0), (0x160, "row_xmask", 0),
    ];

    private const int RowGroup = 16;
    private const int RowMirror = 0x140;
    private const int RowHalfMirror = 0x141;

    public static Decoded? Decode16(CodeWords code, int at, VectorForm form)
    {
        if (!code.Has(at, Words))
        {
            return null;
        }

        var dpp = code[at + 1];
        var control = Control(Bits.Field(dpp, ControlLow, ControlWidth));

        if (!ModifiersAllowed(form, dpp))
        {
            return null;
        }

        var source0 = Source(form, 0, Bits.Field(dpp, 0, ByteWidth), Bits.Flag(dpp, Source0NegateBit), Bits.Flag(dpp, Source0AbsoluteBit));
        var source1 = Source(form, 1, form.Source1, Bits.Flag(dpp, Source1NegateBit), Bits.Flag(dpp, Source1AbsoluteBit));
        var text = new StringBuilder(Sources.Join(form.Mnemonic("_dpp"), form.Operands(source0!, source1, null)));
        text.Append(' ').Append(control);
        text.Append(CultureInfo.InvariantCulture, $" row_mask:0x{Bits.Field(dpp, RowMaskLow, MaskWidth):x} bank_mask:0x{Bits.Field(dpp, BankMaskLow, MaskWidth):x}");

        if (Bits.Flag(dpp, BoundControlBit))
        {
            text.Append(" bound_ctrl:1");
        }

        if (Bits.Flag(dpp, FetchInactiveBit))
        {
            text.Append(" fi:1");
        }

        return new Decoded(text.ToString(), Words);
    }

    public static Decoded? Decode8(CodeWords code, int at, VectorForm form)
    {
        if (!code.Has(at, Words))
        {
            return null;
        }

        var dpp = code[at + 1];
        var source0 = Source(form, 0, Bits.Field(dpp, 0, ByteWidth), false, false);
        var source1 = Source(form, 1, form.Source1, false, false);
        var lanes = Enumerable.Range(0, Lanes).Select(lane => Bits.Field(dpp, ByteWidth + (lane * LaneSelectWidth), LaneSelectWidth));
        var text = Sources.Join(form.Mnemonic("_dpp"), form.Operands(source0!, source1, null)) + " dpp8:[" + string.Join(',', lanes) + "]";

        if (form.Source0 == OperandText.Dpp8FetchInactive)
        {
            text += " fi:1";
        }

        return new Decoded(text, Words);
    }

    /// <summary>A VGPR source with float modifiers; null for VOP1 source 1.</summary>
    private static string? Source(VectorForm form, int index, int register, bool negate, bool absolute)
    {
        if (index == 1 && !form.HasSource1)
        {
            return null;
        }

        var vgpr = OperandText.Vgpr(register, OperandText.Dwords(form.Info.Sources[index], false));

        return Modifiers.Apply(vgpr, negate, absolute, false);
    }

    /// <summary>Integer operations have no source modifiers; set modifier bits make the dword undecodable.</summary>
    private static bool ModifiersAllowed(VectorForm form, uint dpp)
    {
        if (Modifiers.IsFloat(form.Info.Sources[0]))
        {
            return true;
        }

        return Bits.Field(dpp, ModifierLow, ModifierWidth) == 0;
    }

    private static string Control(int control)
    {
        if (control <= QuadPermEnd)
        {
            var lanes = Enumerable.Range(0, QuadLanes).Select(lane => (control >> (lane * LaneSelectBits)) & (QuadLanes - 1));

            return "quad_perm:[" + string.Join(',', lanes) + "]";
        }

        if (control == RowMirror)
        {
            return "row_mirror";
        }

        if (control == RowHalfMirror)
        {
            return "row_half_mirror";
        }

        foreach (var (start, name, first) in RowControls)
        {
            var offset = control - start;

            if (offset >= first && offset < RowGroup)
            {
                return string.Create(CultureInfo.InvariantCulture, $"{name}:{offset}");
            }
        }

        return "/* Invalid dpp_ctrl value */";
    }
}
