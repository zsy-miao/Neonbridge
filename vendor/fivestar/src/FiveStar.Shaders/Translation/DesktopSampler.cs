// Copyright (c) 2026 Five Star Recomp contributors.
// SPDX-License-Identifier: GPL-3.0-or-later

namespace FiveStar.Shaders.Translation;

/// <summary>
/// The D3D12_SAMPLER_DESC (13 dwords, d3d12.h) of an RDNA sampler descriptor (S#, SQ_IMG_SAMP words 0 to 3; field layout
/// as Mesa's sid.h for gfx10). The mapping follows the facts of shadPS4's sampler conversion (GPL-2.0+; facts only, no
/// code): a comparison filter wherever a compare instruction uses the S#, else the filter mode's min/max reduction;
/// MaxLOD = MinLOD without a mip filter; anisotropy only with anisotropic bilinear filtering. Not mapped: border colours
/// from the border-colour table (transparent black stands in).
/// </summary>
public static class DesktopSampler
{
    /// <summary>The dwords of a D3D12_SAMPLER_DESC.</summary>
    public const int Dwords = 13;

    private const int ClampY = 3;
    private const int ClampZ = 6;
    private const int AnisoShift = 9;
    private const int CompareShift = 12;
    private const int FilterModeShift = 29;
    private const int MaxLodShift = 12;
    private const int LodBits = 12;
    private const int LodBiasBits = 14;
    private const int MagFilterShift = 20;
    private const int MinFilterShift = 22;
    private const int MipFilterShift = 26;
    private const int BorderTypeShift = 30;
    private const uint TwoBits = 3;
    private const uint ThreeBits = 7;
    private const float LodScale = 256f;
    private const int AnisoBilinear = 3;
    private const int MipLinear = 2;
    private const int MipNone = 0;
    private const int MinShift = 4;
    private const int MagShift = 2;
    private const int MinimumMode = 1;
    private const int MaximumMode = 2;
    private const uint AnisotropicFilter = 0x55;
    private const uint ComparisonFilter = 0x80;
    private const uint MinimumFilter = 0x100;
    private const uint MaximumFilter = 0x180;
    private const int OpaqueBlack = 1;
    private const int OpaqueWhite = 2;
    private const int WordBits = 32;
    private const int FilterWord = 2;
    private const int BorderWord = 3;
    private const int AddressUDword = 1;
    private const int AddressVDword = 2;
    private const int AddressWDword = 3;
    private const int BiasDword = 4;
    private const int AnisotropyDword = 5;
    private const int CompareDword = 6;
    private const int BorderDword = 7;
    private const int AlphaDword = 10;
    private const int MinLodDword = 11;
    private const int MaxLodDword = 12;

    // SQ_TEX_CLAMP (wrap, mirror, clamp last texel, mirror once last texel, clamp half border, mirror once half border,
    // clamp border, mirror once border) as D3D12_TEXTURE_ADDRESS_MODE.
    private static readonly uint[] AddressModes = [1, 2, 3, 5, 3, 5, 4, 5];

    /// <summary>The desktop sampler of S# words a program uses for a depth compare (<paramref name="comparison"/>) or a sample.</summary>
    public static uint[] Describe(IReadOnlyList<uint> words, bool comparison)
    {
        var (w0, w1, w2, w3) = (words[0], words[1], words[FilterWord], words[BorderWord]);
        var anisotropy = 1u << (int)((w0 >> AnisoShift) & ThreeBits);
        var minLod = (w1 & Mask(LodBits)) / LodScale;
        var maxLod = ((w1 >> MaxLodShift) & Mask(LodBits)) / LodScale;
        var mip = (int)((w2 >> MipFilterShift) & TwoBits);

        if (mip == MipNone)
        {
            maxLod = minLod;
        }

        var desc = new uint[Dwords];
        desc[0] = Filter((int)((w2 >> MinFilterShift) & TwoBits), (int)((w2 >> MagFilterShift) & TwoBits), mip, anisotropy) | Reduction(w0, comparison);
        desc[AddressUDword] = AddressModes[w0 & ThreeBits];
        desc[AddressVDword] = AddressModes[(w0 >> ClampY) & ThreeBits];
        desc[AddressWDword] = AddressModes[(w0 >> ClampZ) & ThreeBits];

        desc[BiasDword] = BitConverter.SingleToUInt32Bits(SignExtend(w2 & Mask(LodBiasBits), LodBiasBits) / LodScale);
        desc[AnisotropyDword] = anisotropy;
        desc[CompareDword] = ((w0 >> CompareShift) & ThreeBits) + 1;
        Border(desc, (int)(w3 >> BorderTypeShift));
        desc[MinLodDword] = BitConverter.SingleToUInt32Bits(minLod);
        desc[MaxLodDword] = BitConverter.SingleToUInt32Bits(maxLod);

        return desc;
    }

    private static uint Filter(int minify, int magnify, int mip, uint anisotropy)
    {
        if (anisotropy > 1 && (minify == AnisoBilinear || magnify == AnisoBilinear))
        {
            return AnisotropicFilter;
        }

        var linearMip = 0u;

        if (mip == MipLinear)
        {
            linearMip = 1;
        }

        return ((uint)(minify & 1) << MinShift) | ((uint)(magnify & 1) << MagShift) | linearMip;
    }

    /// <summary>The comparison reduction for a compare, else the filter mode's minimum or maximum (word 0 bits 29-30).</summary>
    private static uint Reduction(uint w0, bool comparison)
    {
        if (comparison)
        {
            return ComparisonFilter;
        }

        var mode = (int)((w0 >> FilterModeShift) & TwoBits);

        if (mode == MinimumMode)
        {
            return MinimumFilter;
        }

        if (mode == MaximumMode)
        {
            return MaximumFilter;
        }

        return 0;
    }

    private static void Border(uint[] desc, int type)
    {
        var one = BitConverter.SingleToUInt32Bits(1f);

        if (type == OpaqueWhite)
        {
            desc.AsSpan(BorderDword, AlphaDword - BorderDword).Fill(one);
        }

        if (type is OpaqueBlack or OpaqueWhite)
        {
            desc[AlphaDword] = one;
        }
    }

    private static uint Mask(int bits)
    {
        return (1u << bits) - 1;
    }

    private static float SignExtend(uint value, int bits)
    {
        var shift = WordBits - bits;

        return (int)(value << shift) >> shift;
    }
}
