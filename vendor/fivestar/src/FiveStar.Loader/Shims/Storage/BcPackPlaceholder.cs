// Copyright (c) 2026 Five Star Recomp contributors.
// SPDX-License-Identifier: GPL-3.0-or-later

using System.Runtime.CompilerServices;
using System.Runtime.InteropServices;
using FiveStar.Formats.Ttpf.BcPack;

namespace FiveStar.Loader.Shims.Storage;

/// <summary>
/// Neutral blocks for a BCPack page whose blocks cannot be reconstructed: one constant block of the page's BC format
/// repeated. Colour formats read opaque mid grey, BC4 and BC5 read 0.5 per channel (a flat normal for BC5) and BC6H
/// reads black. Block layouts follow the D3D11 block-compression specification.
/// </summary>
internal static class BcPackPlaceholder
{
    private const int SmallBlock = 8;
    private const int LargeBlock = 16;

    // BC1 colour block: two RGB565 endpoints, then 2-bit indices (all 0: every texel is endpoint 0).
    private const ulong Rgb565Grey = (16 << 11) | (32 << 5) | 16;
    private const int SecondColourShift = 16;
    private const ulong Bc1Grey = Rgb565Grey | (Rgb565Grey << SecondColourShift);

    // BC4 channel block: two 8-bit endpoints, then 3-bit indices (all 0).
    private const ulong ChannelHalf = 0x80;
    private const int SecondEndpointShift = 8;
    private const ulong Bc4Half = ChannelHalf | (ChannelHalf << SecondEndpointShift);

    // BC3 alpha block: the BC4 layout with both endpoints opaque.
    private const ulong AlphaOpaque = 0xFF;
    private const ulong Bc3Opaque = AlphaOpaque | (AlphaOpaque << SecondEndpointShift);

    // BC7 mode 6: the mode bit at 6, endpoints R0 R1 G0 G1 B0 B1 A0 A1 of 7 bits from bit 7, P-bits at 63 and 64,
    // 4-bit indices (all 0). With P = 1 the 7-bit value v reads (v << 1) | 1: 64 is grey 129, 127 is alpha 255.
    private const int Bc7Mode6Bit = 6;
    private const int Bc7FirstEndpoint = 7;
    private const int Bc7EndpointBits = 7;
    private const ulong Bc7Grey = 64;
    private const ulong Bc7Opaque = 127;
    private const int Bc7ColourEndpoints = 6;
    private const int Bc7AlphaEndpoints = 2;
    private const int Bc7FirstPBit = 63;
    private const ulong Bc7SecondPBit = 1;

    private static readonly UInt128 Bc7Grey128 = BuildBc7Grey();

    /// <summary>Fills <paramref name="output"/> with placeholder blocks of <paramref name="codec"/>; false for an unknown codec or a partial block.</summary>
    [HotPath]
    public static bool TryFill(int codec, Span<byte> output)
    {
        switch ((BcPackCodec)codec)
        {
            case BcPackCodec.Bc1:
                return Fill(output, Bc1Grey);
            case BcPackCodec.Bc4:
                return Fill(output, Bc4Half);
            case BcPackCodec.Bc3:
                return Fill(output, new UInt128(Bc1Grey, Bc3Opaque));
            case BcPackCodec.Bc5:
                return Fill(output, new UInt128(Bc4Half, Bc4Half));
            case BcPackCodec.Bc6H:
                return Fill(output, UInt128.Zero);
            case BcPackCodec.Bc7:
                return Fill(output, Bc7Grey128);
            default:
                return false;
        }
    }

    [HotPath]
    private static bool Fill(Span<byte> output, ulong block)
    {
        if (output.Length % SmallBlock != 0)
        {
            return false;
        }

        MemoryMarshal.CreateSpan(ref Unsafe.As<byte, ulong>(ref MemoryMarshal.GetReference(output)), output.Length / SmallBlock).Fill(block);

        return true;
    }

    [HotPath]
    private static bool Fill(Span<byte> output, UInt128 block)
    {
        if (output.Length % LargeBlock != 0)
        {
            return false;
        }

        MemoryMarshal.CreateSpan(ref Unsafe.As<byte, UInt128>(ref MemoryMarshal.GetReference(output)), output.Length / LargeBlock).Fill(block);

        return true;
    }

    private static UInt128 BuildBc7Grey()
    {
        ulong low = 1UL << Bc7Mode6Bit;
        int shift = Bc7FirstEndpoint;

        for (int i = 0; i < Bc7ColourEndpoints + Bc7AlphaEndpoints; i++)
        {
            ulong value = Bc7Grey;

            if (i >= Bc7ColourEndpoints)
            {
                value = Bc7Opaque;
            }

            low |= value << shift;
            shift += Bc7EndpointBits;
        }

        low |= 1UL << Bc7FirstPBit;

        return new UInt128(Bc7SecondPBit, low);
    }
}
