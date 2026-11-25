// Copyright (c) 2026 Five Star Recomp contributors.
// SPDX-License-Identifier: GPL-3.0-or-later

namespace FiveStar.Loader.Cpu;

/// <summary>
/// The low-quadword operations of EXTRQ and INSERTQ (AMD64 Architecture Programmer's Manual, volume 4, EXTRQ and
/// INSERTQ). A field that runs past bit 63 is undefined there; these give the bits that fit.
/// </summary>
internal static class BitField
{
    /// <summary>The 6-bit length and index fields; a length of 0 encodes 64.</summary>
    private const int FieldMask = 0x3F;

    /// <summary>The register forms' field byte positions: length in bits 5:0, index in bits 13:8 of the control quadword.</summary>
    private const int IndexShift = 8;

    /// <summary>The field of <paramref name="value"/> at <paramref name="field"/>, zero-extended (EXTRQ).</summary>
    [HotPath]
    public static ulong Extract(ulong value, BitFieldSpec field)
    {
        return (value >> Index(field)) & Mask(field);
    }

    /// <summary>
    /// <paramref name="destination"/> with the field at <paramref name="field"/> replaced by the low bits of
    /// <paramref name="source"/> (INSERTQ).
    /// </summary>
    [HotPath]
    public static ulong Insert(ulong destination, ulong source, BitFieldSpec field)
    {
        int index = Index(field);
        ulong mask = Mask(field);

        return (destination & ~(mask << index)) | ((source & mask) << index);
    }

    /// <summary>The register forms' control: length in bits 5:0 and index in bits 13:8 of a quadword.</summary>
    [HotPath]
    public static BitFieldSpec FromControl(ulong control)
    {
        return new BitFieldSpec((int)(control & FieldMask), (int)((control >> IndexShift) & FieldMask));
    }

    private static int Index(BitFieldSpec field)
    {
        return field.Index & FieldMask;
    }

    private static ulong Mask(BitFieldSpec field)
    {
        int length = field.Length & FieldMask;

        if (length == 0)
        {
            return ulong.MaxValue;
        }

        return (1UL << length) - 1;
    }
}
