// Copyright (c) 2026 Five Star Recomp contributors.
// SPDX-License-Identifier: GPL-3.0-or-later

namespace FiveStar.Loader.Shims.D3D12X.Binding;

/// <summary>
/// The V# fields a buffer view's descriptor record carries (<see cref="BufferRecords"/>). Dwords 0 and 1 (bits 0-15)
/// hold the view's console GPU address, <see cref="Base"/>, as the console's descriptor does (where <see cref="BufferRecords.Based"/> finds that safe): programs read V#s from
/// tables and build pointers from their base (MS_VS_Transform_PositionOnly s_loads a V# and then loads through its
/// base + 0x40), and translated programs reach such a V# through the address map. A view with no console address (a
/// resource the desktop placed) keeps the desktop heap index in dword 0 with base_hi 0. All fields 0 for a texture view.
/// </summary>
/// <param name="Stride">Bytes an indexed access (idxen) multiplies its index by; 0 for a raw view.</param>
/// <param name="Records">Dword 2: num_records, elements of the stride (bytes when the stride is 0).</param>
/// <param name="Format">Dword 3 as the hardware has it (<see cref="BufferFormatCodes"/>).</param>
internal readonly record struct BufferRecord(uint Stride, uint Records, uint Format)
{
    /// <summary>STRIDE: dword 1 bits 16-29; bits 0-15 hold base_hi, 0 when dword 0 is a heap index.</summary>
    private const int StrideShift = 16;
    private const uint StrideMask = 0x3FFF;
    private const int HighShift = 32;
    private const ulong HighMask = 0xFFFF;

    /// <summary>Bytes of an element of a raw view, which counts its first element in dwords.</summary>
    private const uint RawElement = 4;

    /// <summary>The fields of a descriptor that is no buffer view.</summary>
    public static BufferRecord None => default;

    /// <summary>The view's console GPU address (its first element); 0 when it has none.</summary>
    public ulong Base { get; init; }

    /// <summary>Dword 1 of the record: the stride and base_hi.</summary>
    public uint Word1 => ((Stride & StrideMask) << StrideShift) | (uint)((Base >> HighShift) & HighMask);

    /// <summary>Whether the record is a buffer view's.</summary>
    public bool IsBuffer => Records != 0 || Format != 0 || Stride != 0;

    /// <summary>Dword 0 of the record: the base's low dword, or <paramref name="desktop"/> for a view without one.</summary>
    public uint Word0(uint desktop)
    {
        if (Base == 0)
        {
            return desktop;
        }

        return (uint)Base;
    }

    /// <summary>
    /// The record with the console address of element <paramref name="firstElement"/> of a buffer at
    /// <paramref name="address"/> as its base; as it is for a texture view, no address, or <c>--buffer-records-heap-index</c>.
    /// </summary>
    public BufferRecord At(ulong address, ulong firstElement)
    {
        if (address == 0 || !IsBuffer || BufferRecords.HeapIndexOnly)
        {
            return this;
        }

        ulong element = Stride;

        if (element == 0)
        {
            element = RawElement;
        }

        return this with { Base = address + (firstElement * element) };
    }
}
