// Copyright (c) 2026 Five Star Recomp contributors.
// SPDX-License-Identifier: GPL-3.0-or-later

using System.Buffers.Binary;

namespace FiveStar.Formats.Platform;

/// <summary>
/// The x64 instruction forms of a GDK thunk, recognised by their bytes: <c>lea rcx/rdx, [rip + disp32]</c> for the
/// GUIDs, <c>mov rdx, rcx</c> when both GUIDs are the same, and the vtable load <c>mov rax, [rax + disp]</c> of the
/// called function.
/// </summary>
public static class GdkThunkCode
{
    /// <summary>Length of <c>lea reg, [rip + disp32]</c>.</summary>
    public const int LeaLength = 7;

    /// <summary>Offset of the disp32 inside the lea.</summary>
    public const int LeaDisplacement = 3;

    // Byte positions in REX.W opcode ModRM disp.
    private const int OpcodeByte = 1;
    private const int ModRmByte = 2;
    private const int DisplacementByte = LeaDisplacement;

    private const byte RexW = 0x48;
    private const byte Lea = 0x8D;
    private const byte Mov = 0x8B;
    private const byte MovStore = 0x89;
    private const byte RipRcx = 0x0D;
    private const byte RipRdx = 0x15;
    private const byte RdxFromRcx = 0xD1;
    private const byte RcxIntoRdx = 0xCA;
    private const byte RaxDisp8 = 0x40;
    private const byte RaxDisp32 = 0x80;
    private const int Disp32Length = 7;
    private const int SlotSize = 8;
    private const int FirstApiSlotIndex = 3;

    /// <summary>The register a <c>lea reg, [rip + disp32]</c> at <paramref name="at"/> writes, or null.</summary>
    public static GdkRegister? LeaTarget(ReadOnlySpan<byte> code, int at)
    {
        if (at < 0 || at + LeaLength > code.Length || code[at] != RexW || code[at + OpcodeByte] != Lea)
        {
            return null;
        }

        switch (code[at + ModRmByte])
        {
            case RipRcx:
                return GdkRegister.Rcx;
            case RipRdx:
                return GdkRegister.Rdx;
            default:
                return null;
        }
    }

    /// <summary>True when <c>mov rdx, rcx</c> (either encoding) occurs in the range.</summary>
    public static bool CopiesRcxToRdx(ReadOnlySpan<byte> code, int start, int end)
    {
        for (var at = Math.Max(start, 0); at + ModRmByte < Math.Min(end, code.Length); at++)
        {
            var load = code[at + OpcodeByte] == Mov && code[at + ModRmByte] == RdxFromRcx;
            var store = code[at + OpcodeByte] == MovStore && code[at + ModRmByte] == RcxIntoRdx;

            if (code[at] == RexW && (load || store))
            {
                return true;
            }
        }

        return false;
    }

    /// <summary>
    /// The slot of the first vtable load in the range past the IUnknown slots (0..2: QueryInterface, AddRef, Release),
    /// or null. Argument moves may sit between that load and the call through it.
    /// </summary>
    public static int? FirstApiSlot(ReadOnlySpan<byte> code, int start, int end)
    {
        var limit = Math.Min(end, code.Length) - Disp32Length;

        for (var at = Math.Max(start, 0); at < limit; at++)
        {
            if (code[at] != RexW || code[at + OpcodeByte] != Mov)
            {
                continue;
            }

            int? slot = null;

            if (code[at + ModRmByte] == RaxDisp8)
            {
                slot = code[at + DisplacementByte] / SlotSize;
            }
            else if (code[at + ModRmByte] == RaxDisp32)
            {
                slot = BinaryPrimitives.ReadInt32LittleEndian(code[(at + DisplacementByte)..]) / SlotSize;
            }

            if (slot >= FirstApiSlotIndex)
            {
                return slot;
            }
        }

        return null;
    }
}
